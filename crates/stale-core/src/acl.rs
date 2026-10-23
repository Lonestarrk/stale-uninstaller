//! Locking folders down to Administrators and SYSTEM: owner and a protected DACL holding exactly
//! two inheritable full-control ACEs, replacing whatever was there (explicit ACEs included).

use crate::pathutil::wide;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, LocalFree, ERROR_INVALID_OWNER, ERROR_SUCCESS, HANDLE, HLOCAL, LUID};
use windows::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    GetNamedSecurityInfoW, GetSecurityInfo, SetNamedSecurityInfoW, SetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    AddAce, AdjustTokenPrivileges, GetAce, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
    GetSecurityDescriptorOwner, InitializeAcl, LookupPrivilegeValueW, ACE_HEADER, ACE_REVISION, INHERITED_ACE,
    SE_DACL_PROTECTED, ACL, ACL_REVISION, DACL_SECURITY_INFORMATION, LUID_AND_ATTRIBUTES, OWNER_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, SE_PRIVILEGE_ENABLED,
    SE_BACKUP_NAME, SE_RESTORE_NAME, SE_TAKE_OWNERSHIP_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    UNPROTECTED_DACL_SECURITY_INFORMATION,
};
use windows::Win32::Storage::FileSystem::CreateDirectoryW;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Owner Administrators; protected DACL: full control for Administrators and SYSTEM, inherited by
/// subfolders and files.
pub const LOCKED_SDDL: &str = "O:BAD:P(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)";
/// The same DACL without an owner, for creating a folder when the token cannot name Administrators
/// as owner (unelevated); the creator then stays owner.
const LOCKED_SDDL_NO_OWNER: &str = "D:P(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)";

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

/// Serializes privilege changes: each lock-down enables the privileges and restores them after.
static PRIVILEGES: Mutex<()> = Mutex::new(());

/// Privileges enabled on the process token while this guard lives; dropping it puts back their
/// previous state. Unheld privileges are ignored. [`OwnerPrivileges::enable`] enables
/// SeRestorePrivilege and SeTakeOwnershipPrivilege (so an elevated process can always make
/// Administrators the owner); [`backup_privilege`] enables SeBackupPrivilege.
pub(crate) struct OwnerPrivileges {
    _lock: MutexGuard<'static, ()>,
    token: HANDLE,
    previous: Vec<TOKEN_PRIVILEGES>,
}

impl OwnerPrivileges {
    fn enable() -> OwnerPrivileges {
        OwnerPrivileges::enable_named(&[SE_RESTORE_NAME, SE_TAKE_OWNERSHIP_NAME])
    }

    fn enable_named(names: &[PCWSTR]) -> OwnerPrivileges {
        let lock = PRIVILEGES.lock().unwrap_or_else(|e| e.into_inner());
        let mut guard = OwnerPrivileges { _lock: lock, token: HANDLE::default(), previous: Vec::new() };
        // SAFETY: the pseudo handle needs no closing; `token` is closed in `drop`.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut guard.token) }.is_err() {
            guard.token = HANDLE::default();
            return guard;
        }
        for &name in names {
            let mut luid = LUID::default();
            // SAFETY: `name` is a static NUL-terminated string; `luid` is a valid local.
            if unsafe { LookupPrivilegeValueW(PCWSTR::null(), name, &mut luid) }.is_err() {
                continue;
            }
            let tp = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
            };
            let mut previous = TOKEN_PRIVILEGES::default();
            let mut len = 0u32;
            // SAFETY: `token` is open with TOKEN_ADJUST_PRIVILEGES; `tp` is a valid one-entry list and
            // `previous` a buffer of the size passed, which receives the earlier state.
            let adjusted = unsafe {
                AdjustTokenPrivileges(
                    guard.token,
                    false,
                    Some(&tp),
                    std::mem::size_of::<TOKEN_PRIVILEGES>() as u32,
                    Some(&mut previous),
                    Some(&mut len),
                )
            };
            // An unheld privilege yields PrivilegeCount 0: nothing to restore.
            if adjusted.is_ok() && previous.PrivilegeCount > 0 {
                guard.previous.push(previous);
            }
        }
        guard
    }
}

/// SeBackupPrivilege enabled while the guard lives: with `FILE_FLAG_BACKUP_SEMANTICS`, an open for
/// reading then passes even a DACL that excludes Administrators. Never nest it with another
/// guard of this module (they share one lock).
pub(crate) fn backup_privilege() -> OwnerPrivileges {
    OwnerPrivileges::enable_named(&[SE_BACKUP_NAME])
}

impl Drop for OwnerPrivileges {
    fn drop(&mut self) {
        if self.token.is_invalid() {
            return;
        }
        for previous in &self.previous {
            // SAFETY: `token` is open with TOKEN_ADJUST_PRIVILEGES; `previous` came from the call
            // that changed the privilege.
            let _ = unsafe { AdjustTokenPrivileges(self.token, false, Some(previous), 0, None, None) };
        }
        // SAFETY: `token` was opened in `enable`.
        let _ = unsafe { CloseHandle(self.token) };
    }
}

/// Sets owner = Administrators and the locked, protected DACL on an existing file or folder.
fn apply_locked(path: &Path) -> Result<(), String> {
    set_security(path, true)
}

/// The locked, protected DACL, plus owner = Administrators when `with_owner`.
fn set_security(path: &Path, with_owner: bool) -> Result<(), String> {
    let _privileges = OwnerPrivileges::enable();
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

/// Creates the folder `path` (its parent must exist) with the locked descriptor already in place:
/// owner Administrators when the token allows it (else the creator), the two-ACE DACL. Used for
/// folders the lock-down sweep must recognise as ours whatever the "default owner" policy says.
pub fn create_locked(path: &Path) -> Result<(), String> {
    let _privileges = OwnerPrivileges::enable();
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

/// FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES: data access, so the missing delete sharing is enforced
/// (an open with attribute access only takes no part in sharing checks).
const PIN_ACCESS: u32 = 0x1 | 0x80;
const FILE_SHARE_READ_WRITE: u32 = 0x1 | 0x2;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// Opens the folder `path` itself (never a link's target) without delete sharing, so while the
/// handle is open neither it nor a folder above it can be renamed, replaced or deleted. Refuses a
/// reparse point or a non-folder, judged on the opened handle.
fn pin_folder(path: &Path) -> Result<std::fs::File, String> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    let file = std::fs::OpenOptions::new()
        .access_mode(PIN_ACCESS)
        .share_mode(FILE_SHARE_READ_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    let md = file.metadata().map_err(|e| format!("read {}: {e}", path.display()))?;
    if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(format!("{}: {}", crate::quarantine::codes::IS_REPARSE, path.display()));
    }
    if !md.is_dir() {
        return Err(format!("{} is not a folder", path.display()));
    }
    Ok(file)
}

/// `Err` of [`secure_dir`] when `path` turned into a reparse point while it was being locked.
pub const REPARSE_AFTER_LOCK: &str = "reparse-after-lock";

/// What [`secure_dir`] asks of the system, so tests can substitute it.
struct DirOps<'a> {
    is_reparse: &'a dyn Fn(&Path) -> bool,
    owned_by_admins: &'a dyn Fn(&Path) -> Option<bool>,
    apply: &'a dyn Fn(&Path) -> Result<(), String>,
}

/// Locks `path` down to Administrators and SYSTEM, replacing the whole DACL and the owner.
///
/// A missing folder is created with that descriptor ([`create_locked`]); an existing one gets it
/// through `SetNamedSecurityInfoW`. A link or reparse point is refused. The folder is held open
/// without delete sharing for the whole call (so it cannot be swapped for a junction meanwhile),
/// and is checked again after the descriptor change: a reparse point then is
/// `Err("reparse-after-lock")` and nothing below it is touched. When setting the descriptor fails,
/// that is an error unless the folder was created by us (`created_by_us`, or just now), since its
/// DACL is then already right.
///
/// Then each direct child (folder, file, link or reparse point) that is a reparse point or not
/// owned by Administrators/SYSTEM is renamed aside to `<name>.untrusted-<timestamp>` (one warning
/// each in the returned list) — re-checked against a fresh listing of `path` just before; names
/// already holding `.untrusted-` are left alone; links and reparse points are never followed. A
/// planted `settings.json` or `important.json` is never adopted: once it is aside the next load
/// finds no file and uses the defaults, and a handle the planter kept reaches only the moved file.
/// When the rename fails the file stays, and loading ignores it (its owner is not trusted, see
/// [`crate::trusted::read_trusted`]). Admin-owned files are left alone. Trusted child folders get
/// the same descriptor (one level: the DACL is inheritable below).
pub fn secure_dir(path: &Path, created_by_us: bool) -> Result<Vec<String>, String> {
    use crate::quarantine::fsmove::is_reparse_point;
    use crate::quarantine::store::owned_by_admins;
    let ops = DirOps {
        is_reparse: &is_reparse_point,
        owned_by_admins: &owned_by_admins,
        apply: &apply_locked,
    };
    secure_dir_with(path, created_by_us, &ops)
}

fn secure_dir_with(path: &Path, created_by_us: bool, ops: &DirOps) -> Result<Vec<String>, String> {
    let mut created_by_us = created_by_us;
    match std::fs::symlink_metadata(path) {
        Ok(_) if (ops.is_reparse)(path) => {
            return Err(format!("{}: {}", crate::quarantine::codes::IS_REPARSE, path.display()))
        }
        Ok(md) if !md.is_dir() => return Err(format!("{} is not a folder", path.display())),
        Ok(_) => {}
        Err(_) => {
            create_locked(path)?;
            created_by_us = true;
        }
    }
    let _pinned = pin_folder(path)?;
    let mut warnings = Vec::new();
    if let Err(e) = (ops.apply)(path) {
        if !created_by_us {
            return Err(e);
        }
        warnings.push(e);
    }
    if (ops.is_reparse)(path) {
        return Err(REPARSE_AFTER_LOCK.to_string());
    }
    let Ok(rd) = std::fs::read_dir(path) else { return Ok(warnings) };
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S").to_string();
    let untrusted = |child: &Path| (ops.is_reparse)(child) || (ops.owned_by_admins)(child) != Some(true);
    for entry in rd.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().contains(".untrusted-") {
            continue;
        }
        let child = path.join(&name);
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        if untrusted(&child) {
            // Only what a fresh listing of the (pinned) folder still shows, still untrusted.
            let listed = std::fs::read_dir(path).into_iter().flatten().flatten().any(|e| e.file_name() == name);
            if !listed || !untrusted(&child) {
                continue;
            }
            let reparse = (ops.is_reparse)(&child);
            let mut aside = name.clone();
            aside.push(format!(".untrusted-{stamp}"));
            let aside = path.join(aside);
            let why = if reparse { "link or reparse point" } else { "not owned by Administrators or SYSTEM" };
            match std::fs::rename(&child, &aside) {
                Ok(()) => warnings.push(format!("{} ({why}) moved aside to {}", child.display(), aside.display())),
                // Left in place: loading never trusts it (owner or reparse check on its handle).
                Err(e) => warnings.push(format!("{} ({why}) could not be moved aside, ignored: {e}", child.display())),
            }
        } else if is_dir {
            if let Err(e) = (ops.apply)(&child) {
                warnings.push(e);
            }
        }
    }
    Ok(warnings)
}

/// FILE_READ_ATTRIBUTES | READ_CONTROL | WRITE_DAC.
const REINHERIT_ACCESS: u32 = 0x80 | 0x2_0000 | 0x4_0000;

/// Makes BUILTIN\Administrators the owner of the open `file` (which needs WRITE_OWNER), through
/// that handle. Works elevated; unelevated the owner change is refused.
pub(crate) fn set_owner_admins(file: &std::fs::File) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;
    let _privileges = OwnerPrivileges::enable();
    let sd = Sd::from_sddl("O:BA")?;
    let (owner, _) = sd.parts()?;
    // SAFETY: `file` is open with WRITE_OWNER and lives for the call; `owner` points into `sd`,
    // alive for the call.
    let err = unsafe {
        SetSecurityInfo(HANDLE(file.as_raw_handle()), SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION, Some(owner), None, None, None)
    };
    if err == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(windows::core::Error::from(err.to_hresult()).message())
    }
}

/// `Err` of [`reinherit`] when `path` cannot be opened or is a link or reparse point.
pub const REPARSE_OR_OPEN_FAILED: &str = "reparse-or-open-failed";

/// Makes `path` inherit its parent's ACL again: a protected DACL is replaced by an unprotected one
/// holding the same explicit (non-inherited) ACEs, so inheritance is back on (the inherited ACEs
/// are recomputed, also below `path`) and the ACEs given to it on purpose survive. A DACL that is
/// not protected already inherits and is left untouched. The owner is left as it is.
///
/// `path` itself (never a link's target) is opened without delete sharing and the DACL is set
/// through that handle, so a junction swapped in cannot redirect the change; a link, reparse point
/// or an object that cannot be opened is `Err("reparse-or-open-failed")`.
pub fn reinherit(path: &Path) -> Result<(), String> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    let opened = std::fs::OpenOptions::new()
        .access_mode(REINHERIT_ACCESS)
        .share_mode(FILE_SHARE_READ_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .and_then(|f| f.metadata().map(|md| (f, md)));
    let file = match opened {
        Ok((f, md)) if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0 => f,
        Ok(_) => {
            log::warn!("reinherit {}: link or reparse point", path.display());
            return Err(REPARSE_OR_OPEN_FAILED.to_string());
        }
        Err(e) => {
            log::warn!("reinherit {}: {e}", path.display());
            return Err(REPARSE_OR_OPEN_FAILED.to_string());
        }
    };
    let handle = HANDLE(file.as_raw_handle());
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut raw_sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `file` is open with READ_CONTROL; `raw_sd` receives a LocalAlloc'd descriptor that
    // `Sd`'s drop frees, and `dacl` points into it.
    let err = unsafe {
        GetSecurityInfo(handle, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, None, None, Some(&mut dacl), None, Some(&mut raw_sd))
    };
    if err != ERROR_SUCCESS {
        return Err(format!("read ACL of {}: {}", path.display(), windows::core::Error::from(err.to_hresult()).message()));
    }
    let sd = Sd(raw_sd);
    let (mut control, mut revision) = (0u16, 0u32);
    // SAFETY: `sd.0` is the valid descriptor just read; the out-pointers are valid locals.
    unsafe { GetSecurityDescriptorControl(sd.0, &mut control, &mut revision) }
        .map_err(|e| format!("read ACL of {}: {}", path.display(), e.message()))?;
    if control & SE_DACL_PROTECTED.0 == 0 {
        return Ok(());
    }
    let mut buf = explicit_aces_acl(dacl)?;
    // SAFETY: `file` is open with WRITE_DAC and READ_CONTROL and lives for the call; `buf` holds an
    // initialized ACL.
    let err = unsafe {
        SetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | UNPROTECTED_DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(buf.as_mut_ptr() as *const ACL),
            None,
        )
    };
    drop(sd);
    if err == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("reset ACL of {}: {}", path.display(), windows::core::Error::from(err.to_hresult()).message()))
    }
}

/// A new ACL (in a DWORD-aligned buffer) holding the ACEs of `dacl` that are not inherited, in
/// their order; an empty ACL for a null `dacl`.
fn explicit_aces_acl(dacl: *const ACL) -> Result<Vec<u32>, String> {
    let mut kept: Vec<(*const std::ffi::c_void, u16)> = Vec::new();
    let mut acl_revision = ACL_REVISION;
    if !dacl.is_null() {
        // SAFETY: `dacl` is a valid ACL from GetSecurityInfo, alive while the caller holds its descriptor.
        let (count, rev) = unsafe { ((*dacl).AceCount, (*dacl).AclRevision) };
        acl_revision = ACE_REVISION(u32::from(rev));
        for i in 0..u32::from(count) {
            let mut ace: *mut std::ffi::c_void = std::ptr::null_mut();
            // SAFETY: `i` is below the ACE count; `ace` receives a pointer into `dacl`.
            unsafe { GetAce(dacl, i, &mut ace) }.map_err(|e| format!("read ACE: {}", e.message()))?;
            // SAFETY: every ACE starts with an ACE_HEADER.
            let header = unsafe { std::ptr::read_unaligned(ace as *const ACE_HEADER) };
            if u32::from(header.AceFlags) & INHERITED_ACE.0 == 0 {
                kept.push((ace as *const _, header.AceSize));
            }
        }
    }
    let size = std::mem::size_of::<ACL>() + kept.iter().map(|(_, s)| usize::from(*s)).sum::<usize>();
    let mut buf = vec![0u32; size.div_ceil(4)];
    let acl = buf.as_mut_ptr() as *mut ACL;
    // SAFETY: `buf` is DWORD-aligned and at least `size` bytes long, the length passed.
    unsafe { InitializeAcl(acl, (buf.len() * 4) as u32, acl_revision) }
        .map_err(|e| format!("initialize ACL: {}", e.message()))?;
    for (ace, len) in kept {
        // SAFETY: `acl` was initialized with room for every kept ACE; `ace` points at `len` bytes.
        unsafe { AddAce(acl, acl_revision, u32::MAX, ace, u32::from(len)) }.map_err(|e| format!("copy ACE: {}", e.message()))?;
    }
    Ok(buf)
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

    /// The live sweep's operations except ownership, which `owned` answers.
    fn sweep_ops<'a>(owned: &'a dyn Fn(&Path) -> Option<bool>) -> DirOps<'a> {
        DirOps { is_reparse: &|_| false, owned_by_admins: owned, apply: &|_| Ok(()) }
    }

    /// `is_reparse` that answers false for the first `n` questions about `path`, then true.
    fn reparse_after(path: &Path, n: usize) -> impl Fn(&Path) -> bool + '_ {
        let asked = std::cell::Cell::new(0);
        move |p: &Path| {
            if p != path {
                return false;
            }
            asked.set(asked.get() + 1);
            asked.get() > n
        }
    }

    #[test]
    fn reparse_after_lock_stops_before_the_sweep() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("root");
        std::fs::create_dir_all(dir.join("planted")).unwrap();
        let is_reparse = reparse_after(&dir, 1);
        let applied = std::cell::RefCell::new(Vec::new());
        let apply = |p: &Path| {
            applied.borrow_mut().push(p.to_path_buf());
            Ok(())
        };
        let ops = DirOps { is_reparse: &is_reparse, owned_by_admins: &|_| Some(false), apply: &apply };
        assert_eq!(secure_dir_with(&dir, false, &ops), Err(REPARSE_AFTER_LOCK.to_string()));
        assert!(dir.join("planted").is_dir(), "nothing renamed");
        assert_eq!(*applied.borrow(), vec![dir.clone()], "no child touched");
    }

    #[test]
    fn sweep_moves_untrusted_children_aside_once() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("root");
        for child in ["ours", "planted", "old.untrusted-20261018120000"] {
            std::fs::create_dir_all(dir.join(child)).unwrap();
        }
        std::fs::write(dir.join("ours.txt"), b"x").unwrap();
        let ours = dir.join("ours");
        let ours_txt = dir.join("ours.txt");
        let owned = |p: &Path| Some(p == ours || p == ours_txt);
        let applied = std::cell::RefCell::new(Vec::new());
        let apply = |p: &Path| {
            applied.borrow_mut().push(p.to_path_buf());
            Ok(())
        };
        let ops = DirOps { is_reparse: &|_| false, owned_by_admins: &owned, apply: &apply };
        let warnings = secure_dir_with(&dir, false, &ops).unwrap();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let mut names: Vec<String> =
            std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names.len(), 4, "{names:?}");
        assert!(names.contains(&"ours".to_string()) && names.contains(&"old.untrusted-20261018120000".to_string()));
        assert!(names.iter().any(|n| n.starts_with("planted.untrusted-")), "{names:?}");
        assert!(names.contains(&"ours.txt".to_string()), "admin-owned files stay: {names:?}");
        assert_eq!(*applied.borrow(), vec![dir.clone(), ours.clone()]);

        // A second run finds nothing new to move aside.
        assert_eq!(secure_dir_with(&dir, false, &ops).unwrap(), Vec::<String>::new());
    }

    /// A planted settings file is moved aside with its content: the next load uses the defaults,
    /// and writes through a handle the planter kept never reach the name.
    #[test]
    fn planted_settings_are_moved_aside_not_adopted() {
        use std::io::{Seek, Write};
        use std::os::windows::fs::OpenOptionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("root");
        std::fs::create_dir_all(&dir).unwrap();
        let settings = dir.join("settings.json");
        std::fs::write(&settings, br#"{"quarantine_days":1}"#).unwrap();
        // The planter keeps a write handle that shares everything.
        let mut planter = std::fs::OpenOptions::new().read(true).write(true).share_mode(0x7).open(&settings).unwrap();
        let ops = sweep_ops(&|_| Some(false));
        let warnings = secure_dir_with(&dir, false, &ops).unwrap();
        assert!(warnings.iter().any(|w| w.contains("settings.json") && w.contains("moved aside")), "{warnings:?}");
        assert!(!settings.exists(), "the name is free");
        // Even a load that trusted any owner gets the defaults now.
        let loaded = crate::settings::Settings::load_from_with(&settings, &crate::trusted::tests::trust_all);
        assert_eq!(loaded, crate::settings::Settings::default());
        planter.seek(std::io::SeekFrom::Start(0)).unwrap();
        planter.write_all(br#"{"quarantine_days":2}"#).unwrap();
        planter.flush().unwrap();
        assert!(!settings.exists(), "the old handle does not reach the name");
    }

    #[test]
    fn planted_keep_list_is_moved_aside_not_adopted() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("root");
        std::fs::create_dir_all(&dir).unwrap();
        let keep = dir.join("important.json");
        let other = dir.join("notes.txt");
        let admin = dir.join("admin.json");
        std::fs::write(&keep, br#"{"names":["planted"]}"#).unwrap();
        std::fs::write(&other, b"x").unwrap();
        std::fs::write(&admin, b"{}").unwrap();
        let owned = |p: &Path| Some(p == admin);
        let warnings = secure_dir_with(&dir, false, &sweep_ops(&owned)).unwrap();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(!keep.exists() && !other.exists(), "every non-admin plain file is moved aside");
        assert!(admin.is_file(), "an admin-owned file stays");
        let aside: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".untrusted-"))
            .collect();
        assert_eq!(aside.len(), 2, "{aside:?}");
        let keep_aside = dir.join(aside.iter().find(|n| n.starts_with("important.json")).unwrap());
        assert_eq!(std::fs::read(keep_aside).unwrap(), br#"{"names":["planted"]}"#, "moved, content untouched");
        let loaded = crate::keep::KeepList::load_from_with(&keep, &crate::trusted::tests::trust_all);
        assert_eq!(loaded, crate::keep::KeepList::default());
    }

    #[test]
    fn unreplaceable_settings_file_is_ignored() {
        use std::os::windows::fs::OpenOptionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("root");
        std::fs::create_dir_all(&dir).unwrap();
        let settings = dir.join("settings.json");
        let keep = dir.join("important.json");
        std::fs::write(&settings, br#"{"show_hidden":true,"quarantine_days":1}"#).unwrap();
        std::fs::write(&keep, br#"{"names":["planted"]}"#).unwrap();
        // Held open without delete sharing: neither can be renamed aside.
        let hold = |p: &Path| std::fs::OpenOptions::new().read(true).share_mode(0x1).open(p).unwrap();
        let (_s, _k) = (hold(&settings), hold(&keep));
        let ops = DirOps {
            is_reparse: &|_| false,
            owned_by_admins: &|_| Some(false),
            apply: &|_| Ok(()),
        };
        let warnings = secure_dir_with(&dir, false, &ops).unwrap();
        assert!(warnings.iter().any(|w| w.contains("settings.json") && w.contains("ignored")), "{warnings:?}");
        assert!(settings.is_file() && keep.is_file(), "still in place");
        // Every later load judges the owner itself (the same answer the sweep got): defaults.
        let untrusted = crate::trusted::tests::trust_none;
        assert_eq!(crate::settings::Settings::load_from_with(&settings, &untrusted), crate::settings::Settings::default());
        assert_eq!(crate::keep::KeepList::load_from_with(&keep, &untrusted), crate::keep::KeepList::default());
        assert_eq!(crate::keep::KeepList::load_from_with(&keep, &untrusted), crate::keep::KeepList::default(), "and again");
    }

    #[test]
    fn sweep_moves_reparse_files_aside() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("root");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("link.dll"), b"x").unwrap();
        std::fs::write(dir.join("plain.dll"), b"x").unwrap();
        let link = dir.join("link.dll");
        let is_reparse = |p: &Path| p == link;
        let ops = DirOps { is_reparse: &is_reparse, owned_by_admins: &|_| Some(true), apply: &|_| Ok(()) };
        let warnings = secure_dir_with(&dir, false, &ops).unwrap();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(!link.exists(), "the link-looking file is moved aside");
        assert!(dir.join("plain.dll").is_file(), "the plain file stays");
    }

    #[test]
    fn reinherit_refuses_a_link() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let link = tmp.path().join("link");
        assert!(crate::testutil::junction(&link, &target), "mklink /J failed");
        assert_eq!(reinherit(&link), Err(REPARSE_OR_OPEN_FAILED.to_string()));
        assert_eq!(reinherit(&tmp.path().join("missing")), Err(REPARSE_OR_OPEN_FAILED.to_string()));
        std::fs::remove_dir(&link).unwrap();
    }

    #[test]
    fn reinherit_keeps_explicit_aces_and_unprotects() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("restored");
        std::fs::create_dir(&dir).unwrap();
        // An already unprotected (inheriting) DACL is left exactly as it is.
        let before = sddl_of(&dir).unwrap();
        reinherit(&dir).unwrap();
        assert_eq!(sddl_of(&dir).unwrap(), before);
        // A protected DACL with an explicit Users:Modify ACE, plus full control for the owner so
        // an unelevated test can open it too.
        let sd = Sd::from_sddl("D:P(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;FA;;;OW)(A;OICI;0x1301bf;;;BU)").unwrap();
        let (_, locked_dacl) = sd.parts().unwrap();
        let name = wide(&dir);
        // SAFETY: `name` is NUL-terminated; `locked_dacl` points into `sd`, alive for the call.
        let err = unsafe {
            SetNamedSecurityInfoW(
                PCWSTR(name.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(locked_dacl),
                None,
            )
        };
        assert_eq!(err, ERROR_SUCCESS);
        let locked = sddl_of(&dir).unwrap();
        assert!(locked.contains("D:P"), "{locked}");
        reinherit(&dir).unwrap();
        let sddl = sddl_of(&dir).unwrap();
        let dacl = &sddl[sddl.find("D:").unwrap()..];
        assert!(!dacl.starts_with("D:P"), "unprotected: {dacl}");
        assert!(dacl.contains("ID;"), "inherited ACEs: {dacl}");
        assert!(dacl.contains("(A;OICI;0x1301bf;;;BU)"), "the explicit Users:Modify ACE survives: {dacl}");
        assert!(dacl.contains("(A;OICI;FA;;;BA)"), "explicit ACEs are kept: {dacl}");
    }

    #[test]
    fn pinned_folder_cannot_be_swapped() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("root");
        std::fs::create_dir_all(dir.join("child")).unwrap();
        let pinned = pin_folder(&dir).unwrap();
        assert!(std::fs::rename(&dir, tmp.path().join("moved")).is_err(), "the folder itself stays put");
        assert!(std::fs::rename(tmp.path(), tmp.path().with_extension("moved")).is_err(), "and so does its parent");
        std::fs::rename(dir.join("child"), dir.join("child2")).unwrap();
        drop(pinned);
        std::fs::rename(&dir, tmp.path().join("moved")).unwrap();
        assert!(pin_folder(&tmp.path().join("moved").join("child2").join("missing")).is_err());
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
        println!("icacls:\n{}", crate::proc::decode_oem(&listing.stdout));
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
