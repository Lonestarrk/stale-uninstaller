//! Well-known folders used by Stale Uninstaller.

use std::path::PathBuf;

fn env_or(var: &str, fallback: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| fallback.to_string())
}

/// The Windows folder from `GetSystemWindowsDirectoryW`, never from the environment (which the
/// user controls); `%SystemRoot%` and then `C:\Windows` only when the call fails.
pub fn windows_dir() -> PathBuf {
    use windows::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;
    let mut buf = [0u16; 512];
    // SAFETY: `buf` is a valid, writable buffer of the length the wrapper passes.
    let len = unsafe { GetSystemWindowsDirectoryW(Some(&mut buf)) } as usize;
    if len > 0 && len < buf.len() {
        return PathBuf::from(String::from_utf16_lossy(&buf[..len]));
    }
    PathBuf::from(env_or("SystemRoot", r"C:\Windows"))
}

/// `System32` from `SHGetKnownFolderPath(FOLDERID_System)`; `<windows_dir>\System32` when that fails.
pub fn system32_dir() -> PathBuf {
    use windows::Win32::UI::Shell::FOLDERID_System;
    crate::knownfolders::resolve_id(&FOLDERID_System).unwrap_or_else(|| windows_dir().join("System32"))
}

/// `ProgramData` from `SHGetKnownFolderPath(FOLDERID_ProgramData)`; `%ProgramData%` and then
/// `C:\ProgramData` only when that fails.
pub fn program_data_dir() -> PathBuf {
    use windows::Win32::UI::Shell::FOLDERID_ProgramData;
    crate::knownfolders::resolve_id(&FOLDERID_ProgramData).unwrap_or_else(|| PathBuf::from(env_or("ProgramData", r"C:\ProgramData")))
}

/// Test-only, compiled only with the `e2e-overrides` feature (which the GUI never enables): when
/// set (and not empty), [`data_dir`] is this folder instead of `%ProgramData%\Stale Uninstaller`,
/// so the headless end-to-end example (`examples/e2e.rs`) can run unelevated. Without the feature
/// the variable is never read.
#[cfg(feature = "e2e-overrides")]
pub const E2E_DATA_DIR_ENV: &str = "STALE_E2E_DATA_DIR";

/// `override_dir` when it is a non-empty value, else `<program_data>\Stale Uninstaller`.
#[cfg(feature = "e2e-overrides")]
fn data_dir_from(override_dir: Option<std::ffi::OsString>, program_data: &str) -> PathBuf {
    match override_dir {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(program_data).join("Stale Uninstaller"),
    }
}

#[cfg(feature = "e2e-overrides")]
fn data_dir_path() -> PathBuf {
    data_dir_from(std::env::var_os(E2E_DATA_DIR_ENV), &program_data_dir().to_string_lossy())
}

#[cfg(not(feature = "e2e-overrides"))]
fn data_dir_path() -> PathBuf {
    program_data_dir().join("Stale Uninstaller")
}

/// `<ProgramData>\Stale Uninstaller` ([`program_data_dir`]; or, with the `e2e-overrides` feature, the test-only
/// `STALE_E2E_DATA_DIR` folder), created on demand (creation errors are ignored).
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

/// `System32\Tasks` ([`system32_dir`]), where scheduled task files live.
pub fn tasks_root() -> PathBuf {
    system32_dir().join("Tasks")
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

    #[cfg(not(feature = "e2e-overrides"))]
    #[test]
    fn data_dir_override_env_is_ignored_without_the_feature() {
        let expected = program_data_dir().join("Stale Uninstaller");
        std::env::set_var("STALE_E2E_DATA_DIR", r"C:\e2e-should-be-ignored");
        let got = data_dir_path();
        std::env::remove_var("STALE_E2E_DATA_DIR");
        assert_eq!(got, expected);
    }

    #[test]
    fn system_folders_come_from_the_system() {
        let windows = windows_dir();
        assert!(windows.join("System32").join("kernel32.dll").is_file(), "{}", windows.display());
        assert!(system32_dir().join("kernel32.dll").is_file(), "{}", system32_dir().display());
        assert!(program_data_dir().is_dir(), "{}", program_data_dir().display());
        assert!(tasks_root().to_string_lossy().to_lowercase().ends_with(r"system32\tasks"), "{}", tasks_root().display());
    }

    #[cfg(feature = "e2e-overrides")]
    #[test]
    fn data_dir_override_is_ignored_when_unset_or_empty() {
        let expected = PathBuf::from(r"C:\ProgramData\Stale Uninstaller");
        assert_eq!(data_dir_from(None, r"C:\ProgramData"), expected);
        assert_eq!(data_dir_from(Some("".into()), r"C:\ProgramData"), expected);
        assert_eq!(data_dir_from(Some(r"C:\tmp\e2e".into()), r"C:\ProgramData"), PathBuf::from(r"C:\tmp\e2e"));
    }
}
