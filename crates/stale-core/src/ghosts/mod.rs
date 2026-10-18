//! Ghost programs ("Spökprogram"): programs that are gone from the uninstall list but left
//! traces. Collectors gather [`Evidence`]; each one lives in its own module.

pub mod folders;

use crate::leftovers::names::{child_matches_in_container, name_matches, name_matches_first_word_only, normalize_name};
use crate::leftovers::{program_locations, system_root_norm};
use crate::model::{Program, RegKeyPath};
use crate::pathutil::{norm, normed_under};
use crate::usage::UsageSource;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One trace a vanished program left behind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Evidence {
    Folder { path: PathBuf, size: Option<u64>, modified: Option<DateTime<Utc>>, root_kind: RootKind },
    RegKeyMissingPath { key: RegKeyPath, value_name: String, path: PathBuf },
    RegKeyOrphan { key: RegKeyPath },
    /// `location` is the key path, `.lnk` path, service name or task name holding the reference.
    DanglingRef { kind: RefKind, location: String, target: PathBuf, value_name: Option<String> },
    UsageTrace { exe: PathBuf, last_run: DateTime<Utc>, source: UsageSource },
}

/// Which kind of program root an orphan folder was found in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RootKind {
    ProgramFiles,
    ProgramData,
    /// `AppData\Roaming` or `AppData\Local` of a profile.
    AppData,
    /// `AppData\Local\Programs` of a profile.
    LocalPrograms,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefKind { Run, AppPaths, ClassesServer, ShellCommand, Service, ScheduledTask, Shortcut }

/// System and vendor folder names that never count as a ghost, compared case-insensitively. A
/// trailing `*` is a prefix match; dot-folders (`.cache`, `.config`, ...) are skipped as well.
/// Vendor containers such as `Google` are in [`CONTAINER_FOLDERS`] instead: their children are
/// examined.
pub const SKIP_FOLDERS: &[&str] = &[
    "Microsoft",
    "Microsoft Shared",
    "Microsoft OneDrive",
    "Common Files",
    "Windows*",
    // Covered by `Windows*`; listed so the brief's names stay searchable.
    "WindowsApps",
    "Windows Defender*",
    "Windows NT",
    "Windows Mail",
    "Windows Media Player",
    "Windows Photo Viewer",
    "Windows Portable Devices",
    "Windows Security",
    "WindowsPowerShell",
    "Packages",
    "Temp",
    "Internet Explorer",
    "ModifiableWindowsApps",
    "dotnet",
    "MSBuild",
    "Reference Assemblies",
    "Uninstall Information",
    "Package Cache",
    "Intel",
    "NVIDIA Corporation",
    "AMD",
    "Realtek",
    "Dell",
    "HP",
    "Lenovo",
    "ASUS",
    "Common",
    "Crashpad",
    "D3DSCache",
    "ConnectedDevicesPlatform",
    "Comms",
    "Publishers",
    "SquirrelTemp",
    "Programs",
    "CEF",
    "pip",
    "npm",
    "npm-cache",
    "nuget",
];

/// Publisher folders that hold one folder per product; the container itself is never a ghost,
/// its children are examined instead.
pub const CONTAINER_FOLDERS: &[&str] = &["Google", "Mozilla", "Adobe", "JetBrains", "Epic Games", "Unity", "Autodesk"];

/// True when `name` is in [`SKIP_FOLDERS`] or is a dot-folder.
pub fn is_skipped_folder(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.starts_with('.')
        || SKIP_FOLDERS.iter().any(|s| match s.strip_suffix('*') {
            Some(prefix) => lower.starts_with(&prefix.to_lowercase()),
            None => lower == s.to_lowercase(),
        })
}

/// True when `name` is in [`CONTAINER_FOLDERS`].
pub fn is_container_folder(name: &str) -> bool {
    CONTAINER_FOLDERS.iter().any(|c| c.eq_ignore_ascii_case(name))
}

/// What the installed programs (visible and hidden) own: their names and folders.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InstalledIndex {
    /// [`normalize_name`]d display names.
    names: Vec<String>,
    /// [`norm`]ed install folders, display-icon folders and uninstaller folders.
    locations: Vec<String>,
}

impl InstalledIndex {
    /// Indexes every program, hidden ones included: a hidden component still owns its folders.
    pub fn from_programs(programs: &[Program]) -> InstalledIndex {
        let system_root = system_root_norm();
        let mut index = InstalledIndex::default();
        for p in programs {
            let name = normalize_name(&p.name);
            if !name.is_empty() && !index.names.contains(&name) {
                index.names.push(name);
            }
            for loc in program_locations(p, &system_root) {
                if !index.locations.contains(&loc) {
                    index.locations.push(loc);
                }
            }
        }
        index
    }

    /// [`name_matches`] against any installed name.
    pub fn matches_name(&self, name: &str) -> bool {
        self.names.iter().any(|n| name_matches(name, n))
    }

    /// [`name_matches_first_word_only`] against any installed name (`Google` for `Google Chrome`).
    pub fn matches_first_word(&self, name: &str) -> bool {
        self.names.iter().any(|n| name_matches_first_word_only(name, n))
    }

    /// True when `p` equals an installed location or lies under one.
    pub fn covers_path(&self, p: &Path) -> bool {
        let n = norm(p);
        self.locations.iter().any(|loc| *loc == n || normed_under(&n, loc))
    }

    /// True when an installed location lies strictly under `p`.
    pub(crate) fn holds_location(&self, p: &Path) -> bool {
        let n = norm(p);
        self.locations.iter().any(|loc| normed_under(loc, &n))
    }

    /// [`child_matches_in_container`] against any installed name (`Mozilla\Firefox` for
    /// `Mozilla Firefox`).
    pub(crate) fn matches_in_container(&self, child: &str, container: &str) -> bool {
        self.names.iter().any(|n| child_matches_in_container(child, container, n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Hive, ProgramId, Scope};

    fn program(name: &str, install: Option<&str>, hidden: bool) -> Program {
        Program {
            id: ProgramId(name.into()),
            key: RegKeyPath { hive: Hive::LocalMachine, path: format!(r"Software\{name}") },
            name: name.into(),
            publisher: None,
            version: None,
            install_location: install.map(PathBuf::from),
            install_date: None,
            uninstall_string: None,
            quiet_uninstall_string: None,
            display_icon: None,
            is_msi: false,
            estimated_size_bytes: None,
            drive: None,
            hidden,
            scope: Scope::Machine,
        }
    }

    #[test]
    fn installed_index_names_and_paths() {
        let idx = InstalledIndex::from_programs(&[
            program("Google Chrome", Some(r"C:\Program Files\Google\Chrome\Application\"), false),
            program("Hidden Part", Some(r"D:\Hidden"), true),
            program("In Program Files", Some(r"C:\Program Files"), false),
        ]);
        assert!(idx.matches_name("google chrome") && idx.matches_name("GoogleChrome") && idx.matches_name("Hidden Part"));
        assert!(!idx.matches_name("Chrome"));
        assert!(idx.matches_first_word("Google") && !idx.matches_first_word("Google Chrome"));
        assert!(idx.covers_path(Path::new(r"c:\program files\google\chrome\application")));
        assert!(idx.covers_path(Path::new(r"C:\Program Files\Google\Chrome\Application\x\y")));
        assert!(idx.covers_path(Path::new(r"D:\Hidden\sub")));
        assert!(!idx.covers_path(Path::new(r"D:\HiddenOther")));
        assert!(!idx.covers_path(Path::new(r"C:\Program Files\Google\Chrome")));
        // A shared root as install location claims nothing.
        assert!(!idx.covers_path(Path::new(r"C:\Program Files\Anything")));
        assert!(idx.holds_location(Path::new(r"C:\Program Files\Google")));
        assert!(!idx.holds_location(Path::new(r"D:\Hidden")));
    }

    #[test]
    fn skip_and_container_names() {
        for n in ["microsoft", "Windows Kits", "WINDOWSAPPS", ".config", "NuGet", "Crashpad"] {
            assert!(is_skipped_folder(n), "{n}");
        }
        for n in ["Google", "Old Tool", "Microsoftish"] {
            assert!(!is_skipped_folder(n), "{n}");
        }
        assert!(is_container_folder("epic games") && !is_container_folder("Epic"));
    }
}
