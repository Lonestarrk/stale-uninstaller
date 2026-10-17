//! Well-known folders used by Stale Uninstaller.

use std::path::PathBuf;

fn env_or(var: &str, fallback: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| fallback.to_string())
}

/// `%ProgramData%\Stale Uninstaller`, created on demand (creation errors are ignored).
/// A folder left by early builds (`%ProgramData%\Stale`) is renamed on first use.
pub fn data_dir() -> PathBuf {
    let base = PathBuf::from(env_or("ProgramData", r"C:\ProgramData"));
    let dir = base.join("Stale Uninstaller");
    let legacy = base.join("Stale");
    if !dir.exists() && legacy.is_dir() {
        let _ = std::fs::rename(&legacy, &dir);
    }
    let _ = std::fs::create_dir_all(&dir);
    dir
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
