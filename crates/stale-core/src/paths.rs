//! Well-known folders used by Stale Uninstaller.

use std::path::PathBuf;

fn env_or(var: &str, fallback: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| fallback.to_string())
}

fn data_dir_path() -> PathBuf {
    PathBuf::from(env_or("ProgramData", r"C:\ProgramData")).join("Stale Uninstaller")
}

/// `%ProgramData%\Stale Uninstaller`, created on demand (creation errors are ignored).
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
