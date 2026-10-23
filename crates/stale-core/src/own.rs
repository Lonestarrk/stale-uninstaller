//! Stale Uninstaller's own folders, exe and registry key: they are never a ghost's evidence or a
//! leftover, so the program cannot offer to remove itself or its quarantine.

use crate::ghosts::Evidence;
use crate::leftovers::LeftoverKind;
use crate::model::RegKeyPath;
use crate::pathutil::norm;
use std::path::{Path, PathBuf};

const KEY_PATH: &str = r"software\stale uninstaller";

/// The folders that are ours, normalized ([`norm`]). Built once per scan from the live machine
/// ([`OwnPaths::live`]) or from fake paths in tests ([`OwnPaths::from_folders`]).
#[derive(Clone, Debug, Default)]
pub struct OwnPaths {
    folders: Vec<String>,
}

impl OwnPaths {
    /// The data folder, every drive's `<drive>:\Stale Uninstaller` (quarantine payload roots),
    /// the running exe's folder and `%ProgramFiles%\Stale Uninstaller`. Only paths are computed;
    /// nothing is read from disk but the environment.
    pub fn live() -> OwnPaths {
        let mut folders: Vec<PathBuf> = vec![crate::paths::data_dir()];
        folders.extend(('A'..='Z').map(|d| PathBuf::from(format!(r"{d}:\Stale Uninstaller"))));
        if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
            folders.push(dir);
        }
        for var in ["ProgramFiles", "ProgramW6432"] {
            if let Some(pf) = std::env::var_os(var) {
                folders.push(Path::new(&pf).join("Stale Uninstaller"));
            }
        }
        OwnPaths::from_folders(folders)
    }

    pub fn from_folders<I: IntoIterator<Item = PathBuf>>(folders: I) -> OwnPaths {
        OwnPaths { folders: folders.into_iter().map(|f| norm(&f)).filter(|f| !f.is_empty()).collect() }
    }

    /// True for one of our folders, anything below one, or a file named `StaleUninstaller.exe`.
    pub fn is_own_path(&self, p: &Path) -> bool {
        let n = norm(p);
        n.rsplit('\\').next().is_some_and(|f| f.eq_ignore_ascii_case("StaleUninstaller.exe"))
            || self.folders.iter().any(|f| n == *f || n.strip_prefix(f.as_str()).is_some_and(|r| r.starts_with('\\')))
    }

    /// True for `Software\Stale Uninstaller` and below, also under `Wow6432Node`.
    pub fn is_own_key(&self, key: &RegKeyPath) -> bool {
        let p = key.path.to_lowercase();
        let p = p.strip_prefix(r"software\wow6432node\").map(|r| format!(r"software\{r}")).unwrap_or(p);
        p == KEY_PATH || p.strip_prefix(KEY_PATH).is_some_and(|r| r.starts_with('\\'))
    }

    /// True when a leftover names one of ours.
    pub fn is_own_leftover(&self, kind: &LeftoverKind) -> bool {
        match kind {
            LeftoverKind::Dir { path, .. } | LeftoverKind::File { path, .. } => self.is_own_path(path),
            LeftoverKind::RegKey { key } | LeftoverKind::RegValue { key, .. } => self.is_own_key(key),
            LeftoverKind::Shortcut { target, .. } => self.is_own_path(target),
            LeftoverKind::Service { image, .. } => self.is_own_path(image),
            LeftoverKind::ScheduledTask { command, .. } => self.is_own_path(command),
        }
    }

    /// True when ghost evidence names one of ours.
    pub fn is_own_evidence(&self, e: &Evidence) -> bool {
        match e {
            Evidence::Folder { path, .. } => self.is_own_path(path),
            Evidence::RegKeyMissingPath { key, path, .. } => self.is_own_key(key) || self.is_own_path(path),
            Evidence::RegKeyOrphan { key, .. } => self.is_own_key(key),
            Evidence::DanglingRef { target, .. } => self.is_own_path(target),
            Evidence::UsageTrace { exe, .. } => self.is_own_path(exe),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Hive;

    #[test]
    fn paths_and_keys_are_matched_exactly() {
        let own = OwnPaths::from_folders([PathBuf::from(r"C:\ProgramData\Stale Uninstaller")]);
        assert!(own.is_own_path(Path::new(r"c:\programdata\stale uninstaller\quarantine\x")));
        assert!(own.is_own_path(Path::new(r"D:\Tools\StaleUninstaller.exe")));
        assert!(!own.is_own_path(Path::new(r"C:\ProgramData\Stale Uninstaller Pro")));
        let k = |p: &str| RegKeyPath { hive: Hive::CurrentUser, path: p.into() };
        assert!(own.is_own_key(&k(r"Software\Stale Uninstaller")));
        assert!(own.is_own_key(&k(r"Software\WOW6432Node\Stale Uninstaller\x")));
        assert!(!own.is_own_key(&k(r"Software\Stale Uninstaller Pro")));
    }
}
