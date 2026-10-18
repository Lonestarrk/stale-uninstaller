//! Locking folders down to Administrators and SYSTEM: owner and a protected DACL holding exactly
//! two inheritable full-control ACEs, replacing whatever was there (explicit ACEs included).

use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::Once;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, LocalFree, ERROR_INVALID_OWNER, ERROR_SUCCESS, HANDLE, HLOCAL, LUID};
use windows::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    GetNamedSecurityInfoW, SetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    AdjustTokenPrivileges, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner, LookupPrivilegeValueW, ACL,
    DACL_SECURITY_INFORMATION, LUID_AND_ATTRIBUTES, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, SE_PRIVILEGE_ENABLED, SE_RESTORE_NAME, SE_TAKE_OWNERSHIP_NAME,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::Storage::FileSystem::CreateDirectoryW;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Owner Administrators; protected DACL: full control for Administrators and SYSTEM, inherited by
/// subfolders and files.
pub const LOCKED_SDDL: &str = "O:BAD:P(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)";
/// The same DACL without an owner, for creating a folder when the token cannot name Administrators
/// as owner (unelevated); the creator then stays owner.
const LOCKED_SDDL_NO_OWNER: &str = "D:P(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)";

fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/// A security descriptor parsed from SDDL, freed with `LocalFree` on drop.
struct Sd(PSECURITY_DESCRIPTOR);

impl Sd {
    fn from_sddl(sddl: &str) -> Result<Sd, String> {
        let text = wide(sddl);
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `text` is NUL-terminated and outlives the call; `sd` receives a LocalAlloc'd
        // descriptor that `Drop` frees.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(text.as_ptr()), SDDL_REVISION_1, &mut sd, None)
        }
        .map_err(|e| format!("parse security descriptor: {e}"))?;
        Ok(Sd(sd))
    }

    /// The owner SID and the DACL; both point into `self`.
    fn parts(&self) -> Result<(PSID, *const ACL), String> {
        let mut owner = PSID::default();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let (mut defaulted, mut present) = (Default::default(), Default::default());
        // SAFETY: `self.0` is a valid descriptor; the out-pointers are valid locals.
        unsafe {
            GetSecurityDescriptorOwner(self.0, &mut owner, &mut defaulted).map_err(|e| e.to_string())?;
            GetSecurityDescriptorDacl(self.0, &mut present, &mut dacl, &mut defaulted).map_err(|e| e.to_string())?;
        }
        Ok((owner, dacl))
    }
}

impl Drop for Sd {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW (LocalAlloc).
        unsafe { LocalFree(Some(HLOCAL(self.0 .0))) };
    }
}

/// Enables SeRestorePrivilege and SeTakeOwnershipPrivilege on the process token (once), so an
/// elevated process can always make Administrators the owner. Unheld privileges are ignored.
fn enable_owner_privileges() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let mut token = HANDLE::default();
        // SAFETY: the pseudo handle needs no closing; `token` is closed below.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) }.is_err() {
            return;
        }
        for name in [SE_RESTORE_NAME, SE_TAKE_OWNERSHIP_NAME] {
            let mut luid = LUID::default();
            // SAFETY: `name` is a static NUL-terminated string; `luid` is a valid local.
            if unsafe { LookupPrivilegeValueW(PCWSTR::null(), name, &mut luid) }.is_err() {
                continue;
            }
            let tp = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
            };
            // SAFETY: `token` is open with TOKEN_ADJUST_PRIVILEGES; `tp` is a valid one-entry list.
            let _ = unsafe { AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None) };
        }
        // SAFETY: `token` was opened above.
        let _ = unsafe { CloseHandle(token) };
    });
}

/// Sets owner = Administrators and the locked, protected DACL on an existing file or folder.
fn apply_locked(path: &Path) -> Result<(), String> {
    set_security(path, true)
}

/// The locked, protected DACL, plus owner = Administrators when `with_owner`.
fn set_security(path: &Path, with_owner: bool) -> Result<(), String> {
    enable_owner_privileges();
    let sd = Sd::from_sddl(LOCKED_SDDL)?;
    let (owner, dacl) = sd.parts()?;
    let name = wide(path);
    // SAFETY: `name` is NUL-terminated; `owner` and `dacl` point into `sd`, alive for the call.
    let err = unsafe {
        SetNamedSecurityInfoW(
            PCWSTR(name.as_ptr()),
            SE_FILE_OBJECT,
            if with_owner { OWNER_SECURITY_INFORMATION } else { Default::default() }
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            with_owner.then_some(owner),
            None,
            Some(dacl),
            None,
        )
    };
    if err == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("set security on {}: {}", path.display(), windows::core::Error::from(err.to_hresult()).message()))
    }
}

/// Creates `path` (its parent must exist) with the locked descriptor already in place.
fn create_locked(path: &Path) -> Result<(), String> {
    enable_owner_privileges();
    let name = wide(path);
    let mut last = String::new();
    for sddl in [LOCKED_SDDL, LOCKED_SDDL_NO_OWNER] {
        let sd = Sd::from_sddl(sddl)?;
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0 .0,
            bInheritHandle: false.into(),
        };
        // SAFETY: `name` is NUL-terminated; `sa` and the descriptor it points to outlive the call.
        match unsafe { CreateDirectoryW(PCWSTR(name.as_ptr()), Some(&sa)) } {
            Ok(()) => return Ok(()),
            Err(e) if e.code() == ERROR_INVALID_OWNER.to_hresult() => last = e.message(),
            Err(e) => return Err(format!("create {}: {}", path.display(), e.message())),
        }
    }
    Err(format!("create {}: {last}", path.display()))
}

/// Locks `path` down to Administrators and SYSTEM, replacing the whole DACL and the owner.
///
/// A missing folder is created with that descriptor (`CreateDirectoryW`); an existing one gets it
/// through `SetNamedSecurityInfoW`. A link or reparse point is refused. When setting the descriptor
/// fails, that is an error unless the folder was created by us (`created_by_us`, or just now),
/// since its DACL is then already right.
///
/// Then each direct child that is a reparse point or not owned by Administrators/SYSTEM is renamed
/// aside to `<name>.untrusted-<timestamp>` (one warning each in the returned list); trusted child
/// folders get the same descriptor (one level: the DACL is inheritable below that).
pub fn secure_dir(path: &Path, created_by_us: bool) -> Result<Vec<String>, String> {
    use crate::quarantine::fsmove::is_reparse_point;
    use crate::quarantine::store::owned_by_admins;

    let mut created_by_us = created_by_us;
    match std::fs::symlink_metadata(path) {
        Ok(_) if is_reparse_point(path) => return Err(format!("{} is a link or reparse point", path.display())),
        Ok(md) if !md.is_dir() => return Err(format!("{} is not a folder", path.display())),
        Ok(_) => {}
        Err(_) => {
            create_locked(path)?;
            created_by_us = true;
        }
    }
    let mut warnings = Vec::new();
    if let Err(e) = apply_locked(path) {
        if !created_by_us {
            return Err(e);
        }
        warnings.push(e);
    }
    let Ok(rd) = std::fs::read_dir(path) else { return Ok(warnings) };
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S").to_string();
    for entry in rd.flatten() {
        let child = entry.path();
        let reparse = is_reparse_point(&child);
        if reparse || owned_by_admins(&child) != Some(true) {
            let mut aside = entry.file_name();
            aside.push(format!(".untrusted-{stamp}"));
            let aside = path.join(aside);
            let why = if reparse { "link or reparse point" } else { "not owned by Administrators or SYSTEM" };
            match std::fs::rename(&child, &aside) {
                Ok(()) => warnings.push(format!("{} ({why}) moved aside to {}", child.display(), aside.display())),
                Err(e) => warnings.push(format!("{} ({why}) could not be moved aside: {e}", child.display())),
            }
        } else if entry.file_type().is_ok_and(|t| t.is_dir()) {
            if let Err(e) = apply_locked(&child) {
                warnings.push(e);
            }
        }
    }
    Ok(warnings)
}

/// The owner and DACL of `path` as SDDL (e.g. `O:BAD:PAI(A;OICI;FA;;;BA)…`); `None` when unreadable.
pub fn sddl_of(path: &Path) -> Option<String> {
    let name = wide(path);
    let mut sd = PSECURITY_DESCRIPTOR::default();
    let info = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    // SAFETY: `name` is NUL-terminated; `sd` is allocated by the call and freed below.
    unsafe {
        let err = GetNamedSecurityInfoW(PCWSTR(name.as_ptr()), SE_FILE_OBJECT, info, None, None, None, None, &mut sd);
        if err != ERROR_SUCCESS {
            return None;
        }
        let mut text = PWSTR::null();
        let ok = ConvertSecurityDescriptorToStringSecurityDescriptorW(sd, SDDL_REVISION_1, info, &mut text, None).is_ok();
        let out = if ok { text.to_string().ok() } else { None };
        if !text.is_null() {
            LocalFree(Some(HLOCAL(text.0.cast())));
        }
        LocalFree(Some(HLOCAL(sd.0)));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locked_sddl_parses_with_owner_and_dacl() {
        let sd = Sd::from_sddl(LOCKED_SDDL).unwrap();
        let (owner, dacl) = sd.parts().unwrap();
        assert!(!owner.is_invalid());
        assert!(!dacl.is_null());
    }

    #[test]
    fn secure_dir_refuses_files_and_links() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("f.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(secure_dir(&file, false).unwrap_err().contains("not a folder"));
    }

    /// Run with `cargo test -p stale-core -- --ignored secure_dir_removes_preplanted_everyone_ace --nocapture`.
    /// Making Administrators the owner needs an elevated process; unelevated, the owner change is
    /// refused (ERROR_INVALID_OWNER), so the test then checks the DACL replacement alone and says so.
    #[test]
    #[ignore]
    fn secure_dir_removes_preplanted_everyone_ace() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Stale Uninstaller");
        std::fs::create_dir(&dir).unwrap();
        let icacls = crate::proc::system32_tool("icacls.exe");
        crate::proc::run_hidden(&icacls, &[dir.as_os_str(), "/grant".as_ref(), "*S-1-1-0:(OI)(CI)F".as_ref()]).unwrap();
        assert!(sddl_of(&dir).unwrap().contains(";;;WD)"), "Everyone ACE planted: {:?}", sddl_of(&dir));

        let result = secure_dir(&dir, false);
        println!("secure_dir: {result:?}");
        let elevated = result.is_ok();
        if !elevated {
            println!("UNELEVATED: owner = Administrators refused; checking the DACL replacement alone");
            set_security(&dir, false).unwrap();
        }
        let listing = std::process::Command::new(&icacls).arg(&dir).output().unwrap();
        println!("icacls:
{}", String::from_utf8_lossy(&listing.stdout));
        let sddl = sddl_of(&dir).unwrap();
        println!("sddl: {sddl}");
        let dacl = &sddl[sddl.find("D:").unwrap()..];
        assert!(dacl.starts_with("D:P"), "protected: {dacl}");
        assert_eq!(dacl.matches('(').count(), 2, "exactly two ACEs: {dacl}");
        assert!(dacl.contains("(A;OICI;FA;;;BA)") && dacl.contains("(A;OICI;FA;;;SY)"), "only Administrators and SYSTEM: {dacl}");
        if elevated {
            assert_eq!(crate::quarantine::store::owned_by_admins(&dir), Some(true));
        } else {
            // Still the owner: give the folder its inherited ACL back so the temp dir can be removed.
            crate::proc::run_hidden(&icacls, &[dir.as_os_str(), "/reset".as_ref()]).unwrap();
        }
    }
}
