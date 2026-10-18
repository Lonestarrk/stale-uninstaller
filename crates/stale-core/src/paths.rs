//! Well-known folders used by Stale Uninstaller.

use std::path::PathBuf;

fn env_or(var: &str, fallback: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| fallback.to_string())
}

/// Test-only: when set (and not empty), [`data_dir`] is this folder instead of
/// `%ProgramData%\Stale Uninstaller`, so the headless end-to-end example (`examples/e2e.rs`) can
/// run unelevated. The application never sets it.
pub const E2E_DATA_DIR_ENV: &str = "STALE_E2E_DATA_DIR";

/// `override_dir` when it is a non-empty value, else `<program_data>\Stale Uninstaller`.
fn data_dir_from(override_dir: Option<std::ffi::OsString>, program_data: &str) -> PathBuf {
    match override_dir {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(program_data).join("Stale Uninstaller"),
    }
}

fn data_dir_path() -> PathBuf {
    data_dir_from(std::env::var_os(E2E_DATA_DIR_ENV), &env_or("ProgramData", r"C:\ProgramData"))
}

/// `%ProgramData%\Stale Uninstaller` (or the test-only [`E2E_DATA_DIR_ENV`] folder), created on demand (creation errors are ignored).
pub fn data_dir() -> PathBuf {
    let dir = data_dir_path();
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Locks `%ProgramData%\Stale Uninstaller` down to Administrators and SYSTEM (owner and the whole
/// DACL replaced, a link or reparse point refused, untrusted children renamed aside; created
/// locked when missing) via [`crate::acl::secure_dir`], so settings and manifests below it are
/// trusted. Returns the warnings about children moved aside.
pub fn secure_data_dir() -> Result<Vec<String>, String> {
    crate::acl::secure_dir(&data_dir_path(), false)
}

/// Creates `dir` locked ([`crate::acl::create_locked`], so the data folder's lock-down sweep
/// recognises it as ours), falling back to a plain folder; errors are ignored.
fn create_ours(dir: &std::path::Path) {
    if std::fs::symlink_metadata(dir).is_err() && crate::acl::create_locked(dir).is_err() {
        let _ = std::fs::create_dir_all(dir);
    }
}

/// `data_dir()\important.json`: the keep list.
pub fn keep_list_path() -> PathBuf {
    data_dir().join("important.json")
}

/// `data_dir()\logs`, created on demand (creation errors are ignored).
pub fn logs_dir() -> PathBuf {
    let dir = data_dir().join("logs");
    create_ours(&dir);
    dir
}

/// `%SystemDrive%\Users`.
pub fn user_profiles_root() -> PathBuf {
    PathBuf::from(format!(r"{}\Users", env_or("SystemDrive", "C:")))
}

/// `data_dir()\quarantine`: one manifest per quarantine. Created on demand (creation errors are ignored).
pub fn quarantine_manifest_dir() -> PathBuf {
    let dir = data_dir().join("quarantine");
    create_ours(&dir);
    dir
}

/// `<drive>:\Stale Uninstaller\quarantine`: where files moved from that drive are stored, so a move
/// stays a same-volume rename. Not created here; `FileMover::ensure_payload_root` creates and locks it down.
pub fn quarantine_payload_root(drive: char) -> PathBuf {
    PathBuf::from(format!(r"{}:\Stale Uninstaller\quarantine", drive.to_ascii_uppercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_override_is_ignored_when_unset_or_empty() {
        let expected = PathBuf::from(r"C:\ProgramData\Stale Uninstaller");
        assert_eq!(data_dir_from(None, r"C:\ProgramData"), expected);
        assert_eq!(data_dir_from(Some("".into()), r"C:\ProgramData"), expected);
        assert_eq!(data_dir_from(Some(r"C:	mp\e2e".into()), r"C:\ProgramData"), PathBuf::from(r"C:	mp\e2e"));
    }
}
