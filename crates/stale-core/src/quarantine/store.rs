//! Quarantine manifests on disk: `<manifest dir>\<id>\manifest.json`, with the `.reg` exports and
//! task XML copies of that quarantine next to it.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::{QuarantineId, QuarantineManifest};

const MANIFEST_FILE: &str = "manifest.json";

/// `quarantine_manifest_dir()\<id>\manifest.json`.
pub fn manifest_path(id: &QuarantineId) -> PathBuf {
    crate::paths::quarantine_manifest_dir().join(&id.0).join(MANIFEST_FILE)
}

/// Whether `id` is safe to use as a single folder name (letters, digits and `-` only).
pub(crate) fn valid_id(id: &QuarantineId) -> bool {
    !id.0.is_empty() && id.0.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Writes `dir\<id>\manifest.json` (via a temporary file, so a crash never leaves half a manifest).
pub fn save_manifest(dir: &Path, m: &QuarantineManifest) -> Result<(), String> {
    if !valid_id(&m.id) {
        return Err(format!("invalid quarantine id {:?}", m.id.0));
    }
    let folder = dir.join(&m.id.0);
    std::fs::create_dir_all(&folder).map_err(|e| format!("create {}: {e}", folder.display()))?;
    let json = serde_json::to_string_pretty(m).map_err(|e| e.to_string())?;
    let tmp = folder.join("manifest.json.tmp");
    let file = folder.join(MANIFEST_FILE);
    let write = |tmp: &Path| -> std::io::Result<()> {
        let mut f = std::fs::File::create(tmp)?;
        f.write_all(json.as_bytes())?;
        f.sync_all()
    };
    write(&tmp).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &file).map_err(|e| format!("write {}: {e}", file.display()))
}

/// Every readable manifest below `dir`, newest first; unreadable or mismatching ones are skipped.
pub fn list_manifests(dir: &Path) -> Vec<QuarantineManifest> {
    list_trusted_manifests(dir, &|_| Some(true), &|_| false)
}

/// Like [`list_manifests`], but only `<id>` folders that are not reparse points (`is_reparse`) and
/// for which `trusted` returns `Some(true)` (in production [`owned_by_admins`]), so a manifest or a
/// junction planted by an unprivileged user is ignored.
pub fn list_trusted_manifests(
    dir: &Path,
    trusted: &dyn Fn(&Path) -> Option<bool>,
    is_reparse: &dyn Fn(&Path) -> bool,
) -> Vec<QuarantineManifest> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<QuarantineManifest> = rd
        .flatten()
        .filter(|e| !is_reparse(&e.path()) && trusted(&e.path()) == Some(true))
        .filter_map(|e| {
            let text = std::fs::read_to_string(e.path().join(MANIFEST_FILE)).ok()?;
            let m: QuarantineManifest = serde_json::from_str(&text).ok()?;
            (valid_id(&m.id) && e.file_name().to_str() == Some(m.id.0.as_str())).then_some(m)
        })
        .collect();
    out.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.0.cmp(&a.id.0)));
    out
}

/// Whether `path` is owned by BUILTIN\Administrators or LocalSystem; `None` when the owner
/// cannot be read (e.g. the path does not exist). A reparse point is never trusted: the owner
/// lookup would follow it to its target.
pub fn owned_by_admins(path: &Path) -> Option<bool> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LocalFree, ERROR_SUCCESS, HLOCAL};
    use windows::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows::Win32::Security::{
        IsWellKnownSid, WinBuiltinAdministratorsSid, WinLocalSystemSid, OWNER_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR, PSID,
    };

    std::fs::symlink_metadata(path).ok()?;
    if super::fsmove::is_reparse_point(path) {
        return Some(false);
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut owner = PSID::default();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `wide` is NUL-terminated and outlives the call; `owner` points into `sd`, which the
    // call allocates and which is freed below with `LocalFree` after the last use of `owner`.
    unsafe {
        let err = GetNamedSecurityInfoW(
            PCWSTR(wide.as_ptr()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            None,
            None,
            &mut sd,
        );
        if err != ERROR_SUCCESS {
            return None;
        }
        let ok = !owner.is_invalid()
            && (IsWellKnownSid(owner, WinBuiltinAdministratorsSid).as_bool()
                || IsWellKnownSid(owner, WinLocalSystemSid).as_bool());
        LocalFree(Some(HLOCAL(sd.0)));
        Some(ok)
    }
}

/// `"<yyyymmdd-hhmmss>-<4 hex>"`; the suffix is random.
pub fn new_id(now: DateTime<Utc>) -> QuarantineId {
    let random = RandomState::new().hash_one(now.timestamp_nanos_opt().unwrap_or_default()) & 0xffff;
    QuarantineId(format!("{}-{:04x}", now.format("%Y%m%d-%H%M%S"), random))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quarantine::QuarantineId;

    fn manifest(id: &str, created: DateTime<Utc>) -> QuarantineManifest {
        QuarantineManifest {
            id: QuarantineId(id.to_string()),
            program_name: id.to_string(),
            created,
            moved: vec![],
            keys: vec![],
            values: vec![],
            services: vec![],
            tasks: vec![],
            failures: vec![],
            notes: vec![],
        }
    }

    #[test]
    fn list_trusted_manifests_keeps_only_trusted_folders() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let (a, b, c) = (manifest("a-1", now), manifest("b-2", now), manifest("c-3", now));
        for m in [&a, &b, &c] {
            save_manifest(dir.path(), m).unwrap();
        }
        let (trusted_a, unknown_c) = (dir.path().join("a-1"), dir.path().join("c-3"));
        let trusted = move |p: &Path| {
            if p == trusted_a {
                Some(true)
            } else if p == unknown_c {
                None
            } else {
                Some(false)
            }
        };
        assert_eq!(list_trusted_manifests(dir.path(), &trusted, &|_| false), vec![a]);
        assert_eq!(list_manifests(dir.path()).len(), 3, "the unfiltered listing is unchanged");
    }

    #[test]
    fn list_trusted_manifests_skips_reparse_entries() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let (a, b) = (manifest("a-1", now), manifest("b-2", now));
        save_manifest(dir.path(), &a).unwrap();
        save_manifest(dir.path(), &b).unwrap();
        let link = dir.path().join("b-2");
        let checked = std::cell::RefCell::new(Vec::new());
        let trusted = |p: &Path| {
            checked.borrow_mut().push(p.to_path_buf());
            Some(true)
        };
        assert_eq!(list_trusted_manifests(dir.path(), &trusted, &|p| p == link), vec![a]);
        assert!(!checked.borrow().contains(&link), "a reparse entry is refused before its owner is read");
    }

    /// `mklink /J link target`; false when it could not be created.
    fn junction(link: &Path, target: &Path) -> bool {
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .is_ok_and(|o| o.status.success())
    }

    /// Run with `cargo test -p stale-core -- --ignored junction`.
    #[test]
    #[ignore]
    fn junction_to_trusted_folder_is_not_trusted_live() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let m = manifest("j-1", Utc::now());
        save_manifest(other.path(), &m).unwrap();
        assert!(junction(&dir.path().join("j-1"), &other.path().join("j-1")), "mklink /J failed");
        let real = crate::quarantine::fsmove::is_reparse_point;
        assert_eq!(list_trusted_manifests(dir.path(), &|_| Some(true), &real), vec![], "junction entry skipped");
        assert_eq!(list_manifests(other.path()), vec![m], "the target itself is listable");

        let programdata = std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".to_string());
        let to_system = dir.path().join("to-programdata");
        assert!(junction(&to_system, Path::new(&programdata)), "mklink /J failed");
        assert_eq!(owned_by_admins(Path::new(&programdata)), Some(true));
        assert_eq!(owned_by_admins(&to_system), Some(false), "a junction to a SYSTEM-owned folder is not trusted");
    }

    /// Run with `cargo test -p stale-core -- --ignored owned_by_admins`.
    #[test]
    #[ignore]
    fn owned_by_admins_live() {
        let programdata = std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".to_string());
        assert_eq!(owned_by_admins(Path::new(&programdata)), Some(true), "ProgramData is owned by SYSTEM");
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(owned_by_admins(dir.path()), Some(false), "an unelevated user's temp folder");
        assert_eq!(owned_by_admins(&dir.path().join("missing")), None);
    }
}

