//! Well-known folders used by Stale.

use std::path::PathBuf;

fn env_or(var: &str, fallback: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| fallback.to_string())
}

/// `%ProgramData%\Stale`, created on demand (creation errors are ignored).
pub fn data_dir() -> PathBuf {
    let dir = PathBuf::from(env_or("ProgramData", r"C:\ProgramData")).join("Stale");
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
