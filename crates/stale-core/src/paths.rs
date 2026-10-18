//! Well-known folders used by Stale Uninstaller.

use std::path::PathBuf;

fn env_or(var: &str, fallback: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| fallback.to_string())
}

/// `%ProgramData%\Stale Uninstaller`, created on demand (creation errors are ignored).
pub fn data_dir() -> PathBuf {
    let dir = PathBuf::from(env_or("ProgramData", r"C:\ProgramData")).join("Stale Uninstaller");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Locks `data_dir()` down to Administrators and SYSTEM (owner and inherited ACL, refusing a link
/// or reparse point), like a quarantine payload root: settings and manifests below it are trusted.
pub fn secure_data_dir() -> Result<(), String> {
    use crate::quarantine::fsmove::{FileMover, RealFileMover};
    RealFileMover.ensure_payload_root(&data_dir())
}

/// `data_dir()\logs`, created on demand (creation errors are ignored).
pub fn logs_dir() -> PathBuf {
    let dir = data_dir().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// `%SystemDrive%\Users`.
pub fn user_profiles_root() -> PathBuf {
    PathBuf::from(format!(r"{}\Users", env_or("SystemDrive", "C:")))
}

/// `data_dir()\quarantine`: one manifest per quarantine. Created on demand (creation errors are ignored).
pub fn quarantine_manifest_dir() -> PathBuf {
    let dir = data_dir().join("quarantine");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// `<drive>:\Stale Uninstaller\quarantine`: where files moved from that drive are stored, so a move
/// stays a same-volume rename. Not created here; `FileMover::ensure_payload_root` creates and locks it down.
pub fn quarantine_payload_root(drive: char) -> PathBuf {
    PathBuf::from(format!(r"{}:\Stale Uninstaller\quarantine", drive.to_ascii_uppercase()))
}
