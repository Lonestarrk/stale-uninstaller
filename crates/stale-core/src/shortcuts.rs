use std::collections::HashSet;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::core::{Interface, PCWSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM_READ,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink, SLGP_RAWPATH};

use crate::pathutil::{expand_env, norm};

const START_MENU_PROGRAMS: &str = r"Microsoft\Windows\Start Menu\Programs";

/// The all-users and current-user Start Menu `Programs` folders.
pub fn start_menu_dirs() -> Vec<PathBuf> {
    ["ProgramData", "APPDATA"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|base| PathBuf::from(base).join(START_MENU_PROGRAMS))
        .collect()
}

/// The all-users and current-user Startup folders (shortcuts there run at logon).
pub fn startup_dirs() -> Vec<PathBuf> {
    start_menu_dirs().into_iter().map(|d| d.join("Startup")).collect()
}

/// Keeps COM initialized on the calling thread for its lifetime. If the thread already uses
/// another apartment model (`RPC_E_CHANGED_MODE`) COM is usable as-is and nothing is undone.
struct ComGuard {
    initialized: bool,
}

impl ComGuard {
    fn init() -> Self {
        // SAFETY: plain COM initialization; balanced by `CoUninitialize` in `drop` only on success.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        ComGuard { initialized: hr.is_ok() }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.initialized {
            // SAFETY: matches the successful `CoInitializeEx` in `init` on this same thread.
            unsafe { CoUninitialize() }
        }
    }
}

/// An `.exe` a shortcut starts, with its command-line arguments (Squirrel apps point their
/// shortcuts at `Update.exe --processStart App.exe`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutTarget {
    pub path: PathBuf,
    pub arguments: String,
    /// The `.lnk` file this target was read from (empty when not from a file).
    pub lnk: PathBuf,
}

impl ShortcutTarget {
    /// A target without arguments.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        ShortcutTarget { path: path.into(), arguments: String::new(), lnk: PathBuf::new() }
    }
}

fn from_wide(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// The shortcut's target exactly as stored (may contain `%VAR%`; empty if it has none) and
/// its arguments.
fn raw_target(lnk: &Path) -> windows::core::Result<(String, String)> {
    let wide: Vec<u16> = lnk.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut path = [0u16; 1024];
    let mut args = [0u16; 2048];
    // SAFETY: COM is initialized by the caller; `wide` is NUL-terminated and outlives `Load`;
    // `GetPath`/`GetArguments` write at most `buf.len()` UTF-16 units; `GetPath` accepts a
    // null find-data pointer.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.cast::<IPersistFile>()?.Load(PCWSTR(wide.as_ptr()), STGM_READ)?;
        link.GetPath(&mut path, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)?;
        // Arguments only refine matching; a link without readable ones still has a target.
        let _ = link.GetArguments(&mut args);
    }
    Ok((from_wide(&path), from_wide(&args)))
}

/// Resolves a `.lnk` to its target; `None` unless the target is an `.exe`. The target does
/// not have to exist.
pub fn resolve_lnk(lnk: &Path) -> Option<ShortcutTarget> {
    let _com = ComGuard::init();
    let (path, arguments) = raw_target(lnk).ok()?;
    let path = PathBuf::from(expand_env(&path));
    let is_exe = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    is_exe.then(|| ShortcutTarget { path, arguments: arguments.trim().to_string(), lnk: lnk.to_path_buf() })
}

/// Every `.exe` target of the `*.lnk` files under `dirs` (recursive), deduped by path
/// (case-insensitively) and arguments.
pub fn shortcut_targets(dirs: &[PathBuf]) -> Vec<ShortcutTarget> {
    // Held across the walk so each `resolve_lnk` only bumps COM's init count.
    let _com = ComGuard::init();
    let mut lnks = Vec::new();
    for dir in dirs {
        collect_lnks(dir, &mut lnks);
    }
    let mut seen = HashSet::new();
    lnks.iter()
        .filter_map(|l| resolve_lnk(l))
        .filter(|t| seen.insert((norm(&t.path), t.arguments.to_lowercase())))
        .collect()
}

/// Recursively gathers `*.lnk` files; unreadable folders are skipped, symlinks not followed.
fn collect_lnks(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else { continue };
        let path = entry.path();
        if file_type.is_dir() {
            collect_lnks(&path, out);
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk")) {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_menu_dirs_are_the_two_programs_folders() {
        let _env = crate::env_lock();
        let dirs = start_menu_dirs();
        let suffix = Path::new("Microsoft").join("Windows").join("Start Menu").join("Programs");
        assert_eq!(dirs.len(), 2);
        assert_eq!(dirs[0], PathBuf::from(std::env::var("ProgramData").unwrap()).join(&suffix));
        assert_eq!(dirs[1], PathBuf::from(std::env::var("APPDATA").unwrap()).join(&suffix));
    }

    #[test]
    fn shortcut_targets_skips_missing_dirs_and_non_lnk_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("readme.txt"), "x").unwrap();
        let dirs = [dir.path().to_path_buf(), dir.path().join("missing")];
        assert!(shortcut_targets(&dirs).is_empty());
    }

    #[test]
    fn resolve_lnk_rejects_non_shortcut_files() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("broken.lnk");
        std::fs::write(&fake, "not a shell link").unwrap();
        assert_eq!(resolve_lnk(&fake), None);
        assert_eq!(resolve_lnk(&dir.path().join("missing.lnk")), None);
    }

    #[test]
    #[ignore]
    fn shortcuts_live_resolves_at_least_one_exe() {
        let targets = shortcut_targets(&start_menu_dirs());
        println!("{} exe targets, e.g. {:?}", targets.len(), targets.iter().take(5).collect::<Vec<_>>());
        assert!(!targets.is_empty());
        assert!(targets.iter().all(|t| t.path.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))));
    }
}
