//! Where a program came from: who signed its main exe, its version info, its install date and
//! the channel that installed it (spec §8). Nothing here touches the network.

use std::path::Path;

use chrono::NaiveDate;

use crate::model::Program;
use crate::registry::RegistryReader;

#[derive(Clone, Debug, PartialEq)]
pub enum Signature {
    Valid { signer: String },
    /// `reason` is a short code (see [`signature_from_status`]) the GUI translates.
    Invalid { signer: Option<String>, reason: String },
    Unsigned,
    /// The exe is missing or could not be read (or the program has no main exe).
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallSource {
    Winget,
    MicrosoftStore,
    Steam,
    Epic,
    Gog,
    EaApp,
    Ubisoft,
    Driver,
    Oem,
    Msi,
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Provenance {
    pub signature: Signature,
    pub description: Option<String>,
    pub company: Option<String>,
    pub install_date: Option<NaiveDate>,
    pub source: InstallSource,
}

/// Reads an exe's signature and version info.
pub trait ProvenanceProbe: Send + Sync {
    fn signature(&self, exe: &Path) -> Signature;
    /// `(FileDescription, CompanyName)`.
    fn version_info(&self, exe: &Path) -> (Option<String>, Option<String>);
}

/// Holds `InstallDate` (REG_DWORD, unix seconds) of the Windows installation. Feature updates
/// reset it, so the OEM rule built on it is a heuristic.
pub const OS_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

/// Publishers whose programs come preinstalled on their machines.
const OEM_PUBLISHERS: [&str; 10] = ["asus", "asustek", "dell", "hp", "lenovo", "acer", "msi", "micro-star", "gigabyte", "razer"];
/// Hardware vendors; their programs are drivers only when named like one (see [`is_driver`]).
const DRIVER_PUBLISHERS: [&str; 6] = ["nvidia", "amd", "advanced micro devices", "intel", "realtek", "creative"];
/// Lowercased name fragments that make any program a driver package.
const DRIVER_WORDS: [&str; 4] = ["driver", "drivrutin", "treiber", "pilote"];
/// Lowercased name fragments that make a hardware vendor's program a driver package.
const VENDOR_DRIVER_WORDS: [&str; 14] = [
    "chipset",
    "audio",
    "graphics",
    "grafik",
    "ethernet",
    "wireless",
    "wi-fi",
    "bluetooth",
    "management engine",
    "serial io",
    "rapid storage",
    "physx",
    "control panel",
    "usb 3",
];
/// How close (days) to the OS install date an OEM program must be installed.
const OEM_WINDOW_DAYS: i64 = 30;

/// Folder fragments (lowercase, backslash on both sides) that name a store or launcher library.
const LIBRARY_FOLDERS: [(&str, InstallSource); 8] = [
    (r"\steamapps\common\", InstallSource::Steam),
    (r"\epic games\", InstallSource::Epic),
    (r"\gog galaxy\games\", InstallSource::Gog),
    (r"\gog games\", InstallSource::Gog),
    (r"\ea games\", InstallSource::EaApp),
    (r"\origin games\", InstallSource::EaApp),
    (r"\ubisoft game launcher\games\", InstallSource::Ubisoft),
    (r"\windowsapps\", InstallSource::MicrosoftStore),
];

/// Lowercased with backslashes and a trailing backslash, so a fragment matches whole segments.
fn folder_form(p: &Path) -> String {
    let mut s = p.to_string_lossy().to_lowercase().replace('/', "\\");
    if !s.ends_with('\\') {
        s.push('\\');
    }
    s
}

/// True when `publisher` is `name` or starts with it followed by a non-alphanumeric character
/// ("HP Inc." is HP, "HPX Software" is not).
fn publisher_is(publisher: &str, names: &[&str]) -> bool {
    let publisher = publisher.trim().to_lowercase();
    names.iter().any(|n| {
        publisher.strip_prefix(n).is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_alphanumeric()))
    })
}

fn reg_string(reg: &dyn RegistryReader, key: &crate::model::RegKeyPath, name: &str) -> Option<String> {
    use crate::registry::RegValue;
    reg.values(key).ok()?.into_iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).and_then(|(_, v)| match v {
        RegValue::Str(s) | RegValue::ExpandStr(s) => Some(s),
        _ => None,
    })
}

/// The day Windows was installed (UTC), from [`OS_KEY`].
pub fn os_install_date(reg: &dyn RegistryReader) -> Option<NaiveDate> {
    use crate::registry::{key, RegValue};
    let values = reg.values(&key(crate::model::Hive::LocalMachine, OS_KEY)).ok()?;
    let secs = values.into_iter().find(|(n, _)| n.eq_ignore_ascii_case("InstallDate")).and_then(|(_, v)| match v {
        RegValue::Dword(d) => Some(i64::from(d)),
        RegValue::Qword(q) => i64::try_from(q).ok(),
        _ => None,
    })?;
    chrono::DateTime::from_timestamp(secs, 0).map(|t| t.date_naive())
}

/// The channel that installed `p`; the first matching rule wins: a store or launcher library
/// folder (Steam, Epic, GOG, EA, Ubisoft, Microsoft Store), `WindowsInstaller=1`, a winget
/// `InstallSource`, an OEM publisher installed near the OS install date, a driver package.
pub fn install_source(p: &Program, reg: &dyn RegistryReader) -> InstallSource {
    let folders: Vec<String> = p
        .install_location
        .iter()
        .chain(p.display_icon.iter())
        .map(|f| folder_form(f))
        .collect();
    for (fragment, source) in LIBRARY_FOLDERS {
        if folders.iter().any(|f| f.contains(fragment)) {
            return source;
        }
    }
    if p.is_msi {
        return InstallSource::Msi;
    }
    if reg_string(reg, &p.key, "InstallSource").is_some_and(|s| s.to_lowercase().contains("winget")) {
        return InstallSource::Winget;
    }
    let publisher = p.publisher.as_deref().unwrap_or_default();
    if publisher_is(publisher, &OEM_PUBLISHERS) {
        let near = match (p.install_date, os_install_date(reg)) {
            (Some(installed), Some(os)) => (installed - os).num_days().abs() <= OEM_WINDOW_DAYS,
            _ => false,
        };
        if near {
            return InstallSource::Oem;
        }
    }
    if is_driver(&p.name, publisher) {
        return InstallSource::Driver;
    }
    InstallSource::Unknown
}

/// A driver package: named like one in any language we know, or a hardware vendor's program
/// named after a device class ("NVIDIA PhysX", "Realtek Ethernet …"). AMD Software (the
/// Adrenalin driver UI) counts; vendor tools such as GeForce Experience do not.
fn is_driver(name: &str, publisher: &str) -> bool {
    let name = name.to_lowercase();
    if DRIVER_WORDS.iter().any(|w| name.contains(w)) {
        return true;
    }
    publisher_is(publisher, &DRIVER_PUBLISHERS)
        && (VENDOR_DRIVER_WORDS.iter().any(|w| name.contains(w)) || name.starts_with("amd software"))
}

/// Everything the details panel shows about where `p` came from. Without a main exe the
/// signature is [`Signature::Unknown`] and there is no version info.
pub fn provenance(p: &Program, main_exe: Option<&Path>, probe: &dyn ProvenanceProbe, reg: &dyn RegistryReader) -> Provenance {
    let (signature, (description, company)) = match main_exe {
        Some(exe) => (probe.signature(exe), probe.version_info(exe)),
        None => (Signature::Unknown, (None, None)),
    };
    Provenance { signature, description, company, install_date: p.install_date, source: install_source(p, reg) }
}

pub const TRUST_E_PROVIDER_UNKNOWN: i32 = 0x800B_0001_u32 as i32;
pub const TRUST_E_SUBJECT_FORM_UNKNOWN: i32 = 0x800B_0003_u32 as i32;
pub const TRUST_E_NOSIGNATURE: i32 = 0x800B_0100_u32 as i32;
pub const CERT_E_EXPIRED: i32 = 0x800B_0101_u32 as i32;
pub const CERT_E_UNTRUSTEDROOT: i32 = 0x800B_0109_u32 as i32;
pub const CERT_E_CHAINING: i32 = 0x800B_010A_u32 as i32;
pub const CERT_E_REVOKED: i32 = 0x800B_010C_u32 as i32;
pub const TRUST_E_EXPLICIT_DISTRUST: i32 = 0x800B_0111_u32 as i32;
pub const TRUST_E_BAD_DIGEST: i32 = 0x8009_6010_u32 as i32;
pub const TRUST_E_NO_SIGNER_CERT: i32 = 0x8009_6002_u32 as i32;
pub const TRUST_E_CERT_SIGNATURE: i32 = 0x8009_6004_u32 as i32;
pub const CRYPT_E_FILE_ERROR: i32 = 0x8009_2003_u32 as i32;

/// Maps a `WinVerifyTrust` status to a [`Signature`]. Invalid reasons are short codes:
/// `bad-digest`, `expired`, `untrusted-root`, `revoked`, `distrusted`, `bad-certificate`, or
/// `error:0x<hresult>`. Win32 error HRESULTs and `CRYPT_E_FILE_ERROR` (the file could not be
/// read) are `Unknown`.
pub fn signature_from_status(status: i32, signer: Option<String>) -> Signature {
    let reason = match status {
        0 => return Signature::Valid { signer: signer.unwrap_or_default() },
        TRUST_E_NOSIGNATURE | TRUST_E_SUBJECT_FORM_UNKNOWN | TRUST_E_PROVIDER_UNKNOWN => return Signature::Unsigned,
        CRYPT_E_FILE_ERROR => return Signature::Unknown,
        s if (s as u32) & 0xFFFF_0000 == 0x8007_0000 => return Signature::Unknown,
        TRUST_E_BAD_DIGEST => "bad-digest".to_string(),
        CERT_E_EXPIRED => "expired".to_string(),
        CERT_E_UNTRUSTEDROOT | CERT_E_CHAINING => "untrusted-root".to_string(),
        CERT_E_REVOKED => "revoked".to_string(),
        TRUST_E_EXPLICIT_DISTRUST => "distrusted".to_string(),
        TRUST_E_NO_SIGNER_CERT | TRUST_E_CERT_SIGNATURE => "bad-certificate".to_string(),
        s => format!("error:0x{:08X}", s as u32),
    };
    Signature::Invalid { signer, reason }
}

/// The real probe: `WinVerifyTrust` on the embedded signature (no UI, no revocation check, no
/// network retrieval), the signer's display name from the embedded PKCS#7 and the exe's
/// version resource. A file without an embedded signature is checked against the installed
/// system catalogs (local only), where most in-box Windows files are signed.
pub struct WinProbe;

impl ProvenanceProbe for WinProbe {
    fn signature(&self, exe: &Path) -> Signature {
        if !exe.is_file() {
            return Signature::Unknown;
        }
        let status = win::verify_trust(exe);
        let unsigned = matches!(signature_from_status(status, None), Signature::Unsigned);
        if unsigned {
            // No embedded signature: the file may be listed in a system catalog instead.
            if let Some((status, catalog)) = win::verify_catalog(exe) {
                return signature_from_status(status, win::signer_name(&catalog));
            }
        }
        signature_from_status(status, win::signer_name(exe))
    }

    fn version_info(&self, exe: &Path) -> (Option<String>, Option<String>) {
        if !exe.is_file() {
            return (None, None);
        }
        win::version_strings(exe)
    }
}

mod win {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HANDLE, HWND, INVALID_HANDLE_VALUE};
    use windows::Win32::Security::Cryptography::{
        Catalog::{
            CryptCATAdminAcquireContext2, CryptCATAdminCalcHashFromFileHandle2, CryptCATAdminEnumCatalogFromHash,
            CryptCATAdminReleaseCatalogContext, CryptCATAdminReleaseContext, CryptCATCatalogInfoFromContext, CATALOG_INFO,
        },
        CertCloseStore, CertFindCertificateInStore, CertFreeCertificateContext, CertGetNameStringW, CryptMsgClose,
        CryptMsgGetParam, CryptQueryObject, BCRYPT_SHA256_ALGORITHM, CERT_FIND_SUBJECT_CERT, CERT_INFO,
        CERT_NAME_SIMPLE_DISPLAY_TYPE, CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED, CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED, CERT_QUERY_CONTENT_TYPE_FLAGS,
        CERT_QUERY_FORMAT_FLAG_ALL, CERT_QUERY_OBJECT_FILE,
        CMSG_SIGNER_INFO, CMSG_SIGNER_INFO_PARAM, HCERTSTORE, PKCS_7_ASN_ENCODING, X509_ASN_ENCODING,
    };
    use windows::Win32::Security::WinTrust::{
        WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_CATALOG_INFO, WINTRUST_DATA, WINTRUST_DATA_0,
        WINTRUST_DATA_UNION_CHOICE, WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_CATALOG, WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE,
        WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };
    use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};

    fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
        s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
    }

    /// Runs `WinVerifyTrust` with revocation checks off, no UI and no network retrieval, and
    /// always closes the state handle.
    fn run_verify(choice: WINTRUST_DATA_UNION_CHOICE, subject: WINTRUST_DATA_0) -> i32 {
        let mut data = WINTRUST_DATA {
            cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
            dwUIChoice: WTD_UI_NONE,
            fdwRevocationChecks: WTD_REVOKE_NONE,
            dwUnionChoice: choice,
            Anonymous: subject,
            dwStateAction: WTD_STATEACTION_VERIFY,
            dwProvFlags: WTD_REVOCATION_CHECK_NONE | WTD_CACHE_ONLY_URL_RETRIEVAL,
            ..Default::default()
        };
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        // INVALID_HANDLE_VALUE as the window: no interactive UI at all.
        let hwnd = HWND(INVALID_HANDLE_VALUE.0);
        // SAFETY: the caller keeps the subject struct (and its buffers) alive across this call;
        // the second call releases the state data the first one allocated.
        unsafe {
            let status = WinVerifyTrust(hwnd, &mut action, &mut data as *mut WINTRUST_DATA as *mut c_void);
            data.dwStateAction = WTD_STATEACTION_CLOSE;
            WinVerifyTrust(hwnd, &mut action, &mut data as *mut WINTRUST_DATA as *mut c_void);
            status
        }
    }

    /// `WinVerifyTrust` status of the embedded signature.
    pub fn verify_trust(exe: &Path) -> i32 {
        let path = wide(exe);
        let mut file = WINTRUST_FILE_INFO {
            cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: PCWSTR(path.as_ptr()),
            hFile: HANDLE::default(),
            pgKnownSubject: std::ptr::null_mut(),
        };
        run_verify(WTD_CHOICE_FILE, WINTRUST_DATA_0 { pFile: &mut file })
    }

    /// Releases a catalog admin context (and a catalog context in it) on drop.
    struct CatAdmin {
        admin: isize,
        cat: isize,
    }

    impl Drop for CatAdmin {
        fn drop(&mut self) {
            // SAFETY: both handles come from the CryptCATAdmin calls in `verify_catalog`.
            unsafe {
                if self.cat != 0 {
                    let _ = CryptCATAdminReleaseCatalogContext(self.admin, self.cat, 0);
                }
                let _ = CryptCATAdminReleaseContext(self.admin, 0);
            }
        }
    }

    /// For a file signed through a system catalog (no embedded signature, like most in-box
    /// Windows files): the `WinVerifyTrust` status of its catalog membership and the catalog
    /// file. `None` when no installed catalog lists the file. Only the local catalog database
    /// is read.
    pub fn verify_catalog(exe: &Path) -> Option<(i32, std::path::PathBuf)> {
        let file = std::fs::File::open(exe).ok()?;
        let handle = HANDLE(file.as_raw_handle());
        // SHA-256 catalogs first, then the SHA-1 ones older catalogs use.
        for algorithm in [BCRYPT_SHA256_ALGORITHM, PCWSTR::null()] {
            if let Some(found) = verify_catalog_with(exe, handle, algorithm) {
                return Some(found);
            }
        }
        None
    }

    fn verify_catalog_with(exe: &Path, handle: HANDLE, algorithm: PCWSTR) -> Option<(i32, std::path::PathBuf)> {
        let mut admin = 0isize;
        // SAFETY: out-pointers are valid locals; `CatAdmin` releases what is acquired; the
        // hash, tag and path buffers outlive the `WinVerifyTrust` call.
        unsafe {
            CryptCATAdminAcquireContext2(&mut admin, None, algorithm, None, None).ok()?;
            let mut guard = CatAdmin { admin, cat: 0 };
            let mut len = 0u32;
            CryptCATAdminCalcHashFromFileHandle2(admin, handle, &mut len, None, None).ok()?;
            let mut hash = vec![0u8; len as usize];
            CryptCATAdminCalcHashFromFileHandle2(admin, handle, &mut len, Some(hash.as_mut_ptr()), None).ok()?;
            hash.truncate(len as usize);
            guard.cat = CryptCATAdminEnumCatalogFromHash(admin, &hash, None, None);
            if guard.cat == 0 {
                return None;
            }
            let mut info = CATALOG_INFO { cbStruct: std::mem::size_of::<CATALOG_INFO>() as u32, ..Default::default() };
            CryptCATCatalogInfoFromContext(guard.cat, &mut info, 0).ok()?;
            let end = info.wszCatalogFile.iter().position(|&c| c == 0).unwrap_or(info.wszCatalogFile.len());
            let catalog = std::path::PathBuf::from(String::from_utf16_lossy(&info.wszCatalogFile[..end]));
            let tag = wide(hash.iter().map(|b| format!("{b:02X}")).collect::<String>());
            let member = wide(exe);
            let cat_path = wide(&catalog);
            let mut cat_info = WINTRUST_CATALOG_INFO {
                cbStruct: std::mem::size_of::<WINTRUST_CATALOG_INFO>() as u32,
                dwCatalogVersion: 0,
                pcwszCatalogFilePath: PCWSTR(cat_path.as_ptr()),
                pcwszMemberTag: PCWSTR(tag.as_ptr()),
                pcwszMemberFilePath: PCWSTR(member.as_ptr()),
                hMemberFile: handle,
                pbCalculatedFileHash: hash.as_mut_ptr(),
                cbCalculatedFileHash: len,
                pcCatalogContext: std::ptr::null_mut(),
                hCatAdmin: admin,
            };
            let status = run_verify(WTD_CHOICE_CATALOG, WINTRUST_DATA_0 { pCatalog: &mut cat_info });
            Some((status, catalog))
        }
    }

    /// Display name of the signer certificate of the signature in `file` (an exe's embedded
    /// signature, or a catalog file), whether or not it verifies.
    pub fn signer_name(file: &Path) -> Option<String> {
        let path = wide(file);
        let mut store = HCERTSTORE::default();
        let mut msg: *mut c_void = std::ptr::null_mut();
        // SAFETY: out-pointers are valid locals; `store` and `msg` are released below.
        unsafe {
            CryptQueryObject(
                CERT_QUERY_OBJECT_FILE,
                path.as_ptr() as *const c_void,
                // An exe's embedded signature, or a catalog read as the PKCS#7 message it is
                // (as a CTL it would come back without a message handle).
                CERT_QUERY_CONTENT_TYPE_FLAGS(CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED.0 | CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED.0),
                CERT_QUERY_FORMAT_FLAG_ALL,
                0,
                None,
                None,
                None,
                Some(&mut store),
                Some(&mut msg),
                None,
            )
            .ok()?;
        }
        let name = signer_from(store, msg);
        // SAFETY: both handles came from the successful `CryptQueryObject` above.
        unsafe {
            let _ = CryptMsgClose(Some(msg));
            let _ = CertCloseStore(Some(store), 0);
        }
        name
    }

    fn signer_from(store: HCERTSTORE, msg: *mut c_void) -> Option<String> {
        if msg.is_null() || store.is_invalid() {
            return None;
        }
        let mut size = 0u32;
        // SAFETY: size query, then a buffer of that size (u64-aligned for the struct).
        unsafe {
            CryptMsgGetParam(msg, CMSG_SIGNER_INFO_PARAM, 0, None, &mut size).ok()?;
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            CryptMsgGetParam(msg, CMSG_SIGNER_INFO_PARAM, 0, Some(buf.as_mut_ptr() as *mut c_void), &mut size).ok()?;
            let info = &*(buf.as_ptr() as *const CMSG_SIGNER_INFO);
            let find = CERT_INFO { Issuer: info.Issuer, SerialNumber: info.SerialNumber, ..Default::default() };
            let cert = CertFindCertificateInStore(
                store,
                X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
                0,
                CERT_FIND_SUBJECT_CERT,
                Some(&find as *const CERT_INFO as *const c_void),
                None,
            );
            if cert.is_null() {
                return None;
            }
            let len = CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, None);
            let mut name = vec![0u16; len as usize];
            let written = CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, Some(&mut name));
            let _ = CertFreeCertificateContext(Some(cert));
            let text = String::from_utf16_lossy(&name[..(written as usize).saturating_sub(1)]);
            let text = text.trim().to_string();
            (!text.is_empty()).then_some(text)
        }
    }

    /// True when `bytes` bytes from `ptr` lie inside `block`.
    pub(super) fn in_block(block: &[u8], ptr: *const c_void, bytes: usize) -> bool {
        let (start, end) = (block.as_ptr() as usize, block.as_ptr() as usize + block.len());
        let p = ptr as usize;
        !ptr.is_null() && p >= start && p.checked_add(bytes).is_some_and(|e| e <= end)
    }

    /// `(FileDescription, CompanyName)` from the version resource, in its first translation.
    pub fn version_strings(exe: &Path) -> (Option<String>, Option<String>) {
        let path = wide(exe);
        // SAFETY: the buffer has the size the first call reported; `VerQueryValueW` returns
        // pointers into it, read while it lives.
        unsafe {
            let size = GetFileVersionInfoSizeW(PCWSTR(path.as_ptr()), None);
            if size == 0 {
                return (None, None);
            }
            let mut block = vec![0u8; size as usize];
            if GetFileVersionInfoW(PCWSTR(path.as_ptr()), None, size, block.as_mut_ptr() as *mut c_void).is_err() {
                return (None, None);
            }
            // `len` is in bytes for `\VarFileInfo\Translation` and in UTF-16 units for strings.
            let query = |sub: &str, unit: usize| -> Option<(*mut c_void, u32)> {
                let sub = wide(sub);
                let mut ptr: *mut c_void = std::ptr::null_mut();
                let mut len = 0u32;
                if !VerQueryValueW(block.as_ptr() as *const c_void, PCWSTR(sub.as_ptr()), &mut ptr, &mut len).as_bool() {
                    return None;
                }
                in_block(&block, ptr, len as usize * unit).then_some((ptr, len)).filter(|(_, l)| *l > 0)
            };
            let mut langs: Vec<String> = Vec::new();
            if let Some((ptr, len)) = query(r"\VarFileInfo\Translation", 1) {
                let pairs = std::slice::from_raw_parts(ptr as *const u16, (len as usize) / 2);
                langs.extend(pairs.as_chunks::<2>().0.iter().map(|[lang, cp]| format!("{lang:04x}{cp:04x}")));
            }
            // Common fallbacks when the translation table is missing or wrong.
            langs.extend(["040904b0", "040904e4", "04090000"].map(String::from));
            let read = |field: &str| -> Option<String> {
                langs.iter().find_map(|lang| {
                    let (ptr, len) = query(&format!(r"\StringFileInfo\{lang}\{field}"), 2)?;
                    let chars = std::slice::from_raw_parts(ptr as *const u16, len as usize);
                    let end = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
                    let text = String::from_utf16_lossy(&chars[..end]).trim().to_string();
                    (!text.is_empty()).then_some(text)
                })
            };
            (read("FileDescription"), read("CompanyName"))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::model::{Hive, ProgramId, RegKeyPath, Scope};
    use crate::registry::{key, FakeRegistry, RegValue};

    const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

    fn program(leaf: &str, name: &str, publisher: Option<&str>, install: Option<&str>) -> Program {
        let path = format!(r"{UNINSTALL}\{leaf}");
        Program {
            account: None,
            id: ProgramId(format!(r"LocalMachine\{path}")),
            key: RegKeyPath { hive: Hive::LocalMachine, path },
            name: name.into(),
            publisher: publisher.map(Into::into),
            version: None,
            install_location: install.map(PathBuf::from),
            install_date: None,
            uninstall_string: None,
            quiet_uninstall_string: None,
            display_icon: None,
            is_msi: false,
            estimated_size_bytes: None,
            drive: Some('C'),
            hidden: false,
            scope: Scope::Machine,
        }
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// Sets the OS install date (unix seconds, as Windows stores it).
    fn os_installed(reg: &mut FakeRegistry, day: NaiveDate) {
        let secs = day.and_hms_opt(12, 0, 0).unwrap().and_utc().timestamp() as u32;
        reg.set(key(Hive::LocalMachine, OS_KEY), "InstallDate", RegValue::Dword(secs));
    }

    struct FakeProbe {
        signature: Signature,
    }

    impl ProvenanceProbe for FakeProbe {
        fn signature(&self, _exe: &Path) -> Signature {
            self.signature.clone()
        }
        fn version_info(&self, _exe: &Path) -> (Option<String>, Option<String>) {
            (Some("Foo App".into()), Some("Foo Corp".into()))
        }
    }

    #[test]
    fn steam_library_path_is_steam() {
        let reg = FakeRegistry::new();
        let mut p = program("Steam App 570", "Dota 2", Some("Valve"), Some(r"D:\SteamLibrary\steamapps\common\dota 2 beta"));
        assert_eq!(install_source(&p, &reg), InstallSource::Steam);
        // First match wins: a Steam game with an MSI entry is still Steam.
        p.is_msi = true;
        assert_eq!(install_source(&p, &reg), InstallSource::Steam);
        let steam_itself = program("Steam", "Steam", Some("Valve Corporation"), Some(r"C:\Program Files (x86)\Steam"));
        assert_eq!(install_source(&steam_itself, &reg), InstallSource::Unknown);
    }

    #[test]
    fn store_and_launcher_folders_name_their_source() {
        let reg = FakeRegistry::new();
        let cases = [
            (r"C:\Program Files\Epic Games\Fortnite", InstallSource::Epic),
            (r"C:\Program Files (x86)\GOG Galaxy\Games\Witcher 3", InstallSource::Gog),
            (r"C:\GOG Games\Gwent", InstallSource::Gog),
            (r"C:\Program Files\EA Games\Battlefield", InstallSource::EaApp),
            (r"C:\Program Files (x86)\Origin Games\Sims 4", InstallSource::EaApp),
            (r"C:\Program Files (x86)\Ubisoft\Ubisoft Game Launcher\games\Anno 1800", InstallSource::Ubisoft),
            (r"C:\Program Files\WindowsApps\Microsoft.Foo_1.0_x64__8wekyb3d8bbwe", InstallSource::MicrosoftStore),
            (r"C:\Program Files\Epic Gamesville", InstallSource::Unknown),
        ];
        for (dir, want) in cases {
            assert_eq!(install_source(&program("x", "X", None, Some(dir)), &reg), want, "{dir}");
        }
    }

    #[test]
    fn msi_flag_is_msi() {
        let reg = FakeRegistry::new();
        let mut p = program("{GUID}", "Foo", Some("Foo Corp"), Some(r"C:\Program Files\Foo"));
        p.is_msi = true;
        assert_eq!(install_source(&p, &reg), InstallSource::Msi);
    }

    #[test]
    fn winget_install_source_is_winget() {
        let mut reg = FakeRegistry::new();
        let p = program("Foo", "Foo", Some("Foo Corp"), Some(r"C:\Program Files\Foo"));
        reg.set(p.key.clone(), "InstallSource", RegValue::Str(r"C:\Users\x\AppData\Local\Temp\WinGet\Foo.Foo.1.2".into()));
        assert_eq!(install_source(&p, &reg), InstallSource::Winget);
        let other = program("Bar", "Bar", None, Some(r"C:\Program Files\Bar"));
        reg.set(other.key.clone(), "InstallSource", RegValue::Str(r"C:\Downloads".into()));
        assert_eq!(install_source(&other, &reg), InstallSource::Unknown);
    }

    #[test]
    fn oem_publisher_near_os_install_is_oem() {
        let mut reg = FakeRegistry::new();
        os_installed(&mut reg, date(2025, 3, 1));
        let mut p = program("Armoury", "Armoury Crate", Some("ASUSTeK COMPUTER INC."), Some(r"C:\Program Files\ASUS\Armoury"));
        p.install_date = Some(date(2025, 3, 10));
        assert_eq!(install_source(&p, &reg), InstallSource::Oem);
        p.install_date = Some(date(2025, 2, 20));
        assert_eq!(install_source(&p, &reg), InstallSource::Oem, "OEM images install before OOBE");
        // Installed later by the user: not preinstalled.
        p.install_date = Some(date(2025, 6, 1));
        assert_eq!(install_source(&p, &reg), InstallSource::Unknown);
        // No date: no claim.
        p.install_date = None;
        assert_eq!(install_source(&p, &reg), InstallSource::Unknown);
        // A publisher that only starts with an OEM name is not one.
        let mut hpx = program("x", "X", Some("HPX Software"), None);
        hpx.install_date = Some(date(2025, 3, 2));
        assert_eq!(install_source(&hpx, &reg), InstallSource::Unknown);
        let mut hp = program("y", "Y", Some("HP Inc."), None);
        hp.install_date = Some(date(2025, 3, 2));
        assert_eq!(install_source(&hp, &reg), InstallSource::Oem);
        // Without an OS install date the rule never fires.
        assert_eq!(install_source(&hp, &FakeRegistry::new()), InstallSource::Unknown);
    }

    #[test]
    fn drivers_by_name_or_vendor() {
        let reg = FakeRegistry::new();
        assert_eq!(install_source(&program("a", "Foo Audio Driver", Some("Foo"), None), &reg), InstallSource::Driver);
        assert_eq!(install_source(&program("b", "Skrivare drivrutin", None, None), &reg), InstallSource::Driver);
        assert_eq!(install_source(&program("c", "NVIDIA Graphics", Some("NVIDIA Corporation"), None), &reg), InstallSource::Driver);
        assert_eq!(
            install_source(&program("d", "Chipset", Some("Advanced Micro Devices, Inc."), None), &reg),
            InstallSource::Driver
        );
        assert_eq!(install_source(&program("e", "Foo", Some("Intellicorp"), None), &reg), InstallSource::Unknown);
        let nvidia = Some("NVIDIA Corporation");
        let cases = [
            ("NVIDIA Graphics Driver 551.52", nvidia, InstallSource::Driver),
            ("NVIDIA PhysX systemprogramvara", nvidia, InstallSource::Driver),
            ("Drucker Treiber", None, InstallSource::Driver),
            ("Pilote d'imprimante", None, InstallSource::Driver),
            ("AMD Software", Some("Advanced Micro Devices, Inc."), InstallSource::Driver),
            ("Intel(R) Serial IO", Some("Intel Corporation"), InstallSource::Driver),
            ("Realtek Ethernet Controller", Some("Realtek"), InstallSource::Driver),
            // Vendor tools that are not drivers.
            ("NVIDIA GeForce Experience 3.25", nvidia, InstallSource::Unknown),
            ("NVIDIA NodeJS", nvidia, InstallSource::Unknown),
            ("AMD Product Verification Tool", Some("Advanced Micro Devices, Inc."), InstallSource::Unknown),
            ("Intel Unison", Some("Intel Corporation"), InstallSource::Unknown),
            // A driver-like word from a non-vendor publisher is no driver.
            ("Foo Audio Recorder", Some("Foo"), InstallSource::Unknown),
        ];
        for (name, publisher, want) in cases {
            assert_eq!(install_source(&program("x", name, publisher, None), &reg), want, "{name}");
        }
    }

    #[test]
    fn provenance_reads_the_main_exe() {
        let reg = FakeRegistry::new();
        let mut p = program("Foo", "Foo", Some("Foo Corp"), Some(r"C:\Program Files\Foo"));
        p.install_date = Some(date(2024, 5, 6));
        let probe = FakeProbe { signature: Signature::Valid { signer: "Foo Corp".into() } };
        let got = provenance(&p, Some(Path::new(r"C:\Program Files\Foo\foo.exe")), &probe, &reg);
        assert_eq!(
            got,
            Provenance {
                signature: Signature::Valid { signer: "Foo Corp".into() },
                description: Some("Foo App".into()),
                company: Some("Foo Corp".into()),
                install_date: Some(date(2024, 5, 6)),
                source: InstallSource::Unknown,
            }
        );
    }

    #[test]
    fn no_main_exe_still_has_date_and_source() {
        let reg = FakeRegistry::new();
        let mut p = program("Foo", "Foo", None, None);
        p.is_msi = true;
        p.install_date = Some(date(2024, 5, 6));
        let probe = FakeProbe { signature: Signature::Unsigned };
        let got = provenance(&p, None, &probe, &reg);
        assert_eq!(got.signature, Signature::Unknown);
        assert_eq!((got.description, got.company), (None, None));
        assert_eq!(got.install_date, Some(date(2024, 5, 6)));
        assert_eq!(got.source, InstallSource::Msi);
    }

    #[test]
    fn trust_status_maps_to_signature() {
        let signer = || Some("X".to_string());
        assert_eq!(signature_from_status(0, signer()), Signature::Valid { signer: "X".into() });
        for unsigned in [TRUST_E_NOSIGNATURE, TRUST_E_SUBJECT_FORM_UNKNOWN, TRUST_E_PROVIDER_UNKNOWN] {
            assert_eq!(signature_from_status(unsigned, None), Signature::Unsigned);
        }
        assert_eq!(
            signature_from_status(TRUST_E_BAD_DIGEST, signer()),
            Signature::Invalid { signer: signer(), reason: "bad-digest".into() }
        );
        assert_eq!(signature_from_status(CERT_E_EXPIRED, None), Signature::Invalid { signer: None, reason: "expired".into() });
        assert_eq!(
            signature_from_status(0x800B_0109_u32 as i32, signer()),
            Signature::Invalid { signer: signer(), reason: "untrusted-root".into() }
        );
        assert_eq!(
            signature_from_status(0x8009_9999_u32 as i32, None),
            Signature::Invalid { signer: None, reason: "error:0x80099999".into() }
        );
        // A file that cannot be read (Win32 error HRESULTs) is unknown, not invalid.
        assert_eq!(signature_from_status(0x8007_0002_u32 as i32, None), Signature::Unknown);
        assert_eq!(signature_from_status(0x8009_2003_u32 as i32, None), Signature::Unknown, "CRYPT_E_FILE_ERROR");
    }

    #[test]
    fn version_values_must_lie_inside_the_block() {
        let block = [0u8; 16];
        let at = |i: usize| block[i..].as_ptr() as *const std::ffi::c_void;
        assert!(win::in_block(&block, at(0), 16));
        assert!(win::in_block(&block, at(8), 8));
        assert!(!win::in_block(&block, at(8), 9));
        assert!(!win::in_block(&block, std::ptr::null(), 0));
        let outside = [0u8; 4];
        assert!(!win::in_block(&block, outside.as_ptr() as *const std::ffi::c_void, 1));
    }

    #[test]
    #[ignore = "reads the real notepad.exe"]
    fn win_probe_reads_notepad() {
        let exe = Path::new(r"C:\Windows\System32\notepad.exe");
        let sig = WinProbe.signature(exe);
        let (description, company) = WinProbe.version_info(exe);
        println!("{sig:?} {description:?} {company:?}");
        match sig {
            Signature::Valid { signer } => assert!(signer.contains("Microsoft"), "{signer}"),
            other => panic!("notepad.exe: {other:?}"),
        }
        assert!(description.is_some_and(|d| !d.is_empty()));
    }
}
