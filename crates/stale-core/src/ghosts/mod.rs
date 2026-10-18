//! Ghost programs ("Spökprogram"): programs that are gone from the uninstall list but left
//! traces. Collectors gather [`Evidence`]; each one lives in its own module.

pub mod folders;
pub mod group;
pub mod refs;
pub mod registry;
pub mod scan;

pub use group::{ghost_name_of, group, to_leftovers, Assessment, Ghost, NameRoots};
pub use scan::{merge_programs, scan_ghosts, GhostSources};

use crate::leftovers::names::{child_matches_in_container, name_matches, name_matches_exact, name_matches_first_word_only, normalize_name};
use crate::leftovers::{program_locations, system_root_norm};
use crate::model::{Hive, Program, RegKeyPath};
use crate::pathutil::{norm, normed_under};
use crate::usage::UsageSource;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One trace a vanished program left behind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Evidence {
    /// `holds_user_data`: a direct child is named like user data (`saves`, `profiles`, ...).
    Folder {
        path: PathBuf,
        size: Option<u64>,
        modified: Option<DateTime<Utc>>,
        root_kind: RootKind,
        #[serde(default)]
        holds_user_data: bool,
    },
    /// `shared`: the key also holds an installed program's subkey, so removing it is never sure.
    RegKeyMissingPath {
        key: RegKeyPath,
        value_name: String,
        path: PathBuf,
        #[serde(default)]
        shared: bool,
    },
    /// `shared` as for [`Evidence::RegKeyMissingPath`].
    RegKeyOrphan {
        key: RegKeyPath,
        #[serde(default)]
        shared: bool,
    },
    /// `location` is the key path, `.lnk` path, service name or task name holding the reference.
    /// `partly_live`: the holder also references targets that still exist (a scheduled task with
    /// another command that is there), so removing it is never sure.
    DanglingRef {
        kind: RefKind,
        location: String,
        target: PathBuf,
        value_name: Option<String>,
        #[serde(default)]
        partly_live: bool,
    },
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
    // Microsoft products are out of scope; false negatives on them are accepted.
    "Microsoft*",
    // Covered by `Microsoft*`; listed so the brief's names stay searchable.
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
    "Squirrel*",
    // Covered by `Squirrel*`.
    "SquirrelTemp",
    "SquirrelMachineInstalls",
    "choco*",
    // Covered by `choco*`.
    "chocolatey",
    "Downloaded Installations",
    "Hyper-V",
    "PCHealthCheck",
    "RUXIM",
    "USOShared",
    "Programs",
    "CEF",
    "pip",
    "npm",
    "npm-cache",
    "nuget",
    "Update",
    "Updater",
    "CrashReports",
    "CrashDumps",
    "Maintenance Service",
    "Mozilla Maintenance Service",
];

/// Publisher folders that hold one folder per product; the container itself is never a ghost,
/// its children are examined instead.
pub const CONTAINER_FOLDERS: &[&str] = &["Google", "Mozilla", "Adobe", "JetBrains", "Unity", "Autodesk"];

/// Game launcher libraries: a folder that is, or lies under, one of these holds games a launcher
/// manages, so its ghost is never more than Uncertain (reason `launcher-library`).
pub const LAUNCHER_FOLDERS: &[&str] = &[
    "Epic Games",
    "Steam",
    "steamapps",
    "GOG Galaxy",
    "XboxGames",
    "Battle.net",
    "Origin Games",
    "EA Games",
    "Ubisoft Game Launcher",
    "Riot Games",
    "Games",
];

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

/// True when `name` is in [`LAUNCHER_FOLDERS`].
pub fn is_launcher_folder(name: &str) -> bool {
    LAUNCHER_FOLDERS.iter().any(|l| l.eq_ignore_ascii_case(name))
}

/// True when `p` or a folder above it is named in [`LAUNCHER_FOLDERS`].
pub fn in_launcher_library(p: &Path) -> bool {
    p.components().any(|c| is_launcher_folder(&c.as_os_str().to_string_lossy()))
}

/// The existing targets of every live reference the collectors read (service and driver
/// `ImagePath`s, task commands, Run / App Paths / COM server / shell command targets, shortcut
/// targets, executables that ran and still exist), [`norm`]ed. A folder holding one is in use.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveRefs {
    pub paths: Vec<String>,
}

impl LiveRefs {
    /// [`norm`]ed, sorted and deduplicated.
    pub fn from_targets(targets: impl IntoIterator<Item = PathBuf>) -> LiveRefs {
        let mut paths: Vec<String> = targets.into_iter().map(|p| norm(&p)).filter(|n| !n.is_empty()).collect();
        paths.sort();
        paths.dedup();
        LiveRefs { paths }
    }

    /// True when a live target equals `folder` or lies under it.
    pub fn holds(&self, folder: &Path) -> bool {
        let n = norm(folder);
        !n.is_empty() && self.paths.iter().any(|p| *p == n || normed_under(p, &n))
    }
}

/// What one collector found: its evidence, and the targets of the references it read that exist.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Collected {
    pub evidence: Vec<Evidence>,
    pub live_targets: Vec<PathBuf>,
}

/// `HIVE\path` (`HKLM\SOFTWARE\Foo`, `HKCU\…`, `HKU\…`), the form [`Evidence::DanglingRef`]
/// uses for a registry location.
pub fn format_location(k: &RegKeyPath) -> String {
    let hive = match k.hive {
        Hive::LocalMachine => "HKLM",
        Hive::CurrentUser => "HKCU",
        Hive::Users => "HKU",
    };
    format!(r"{hive}\{}", k.path)
}

/// The key a registry [`Evidence::DanglingRef`] location names; `None` for any other location
/// (a `.lnk` path, a service or task name). The form written before (`LocalMachine\…`,
/// `CurrentUser\…`, `Users\…`) is still read.
pub fn parse_location(s: &str) -> Option<RegKeyPath> {
    let (hive, path) = s.split_once('\\')?;
    let hive = match hive {
        "HKLM" | "LocalMachine" => Hive::LocalMachine,
        "HKCU" | "CurrentUser" => Hive::CurrentUser,
        "HKU" | "Users" => Hive::Users,
        _ => return None,
    };
    (!path.is_empty()).then(|| RegKeyPath { hive, path: path.to_string() })
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

    /// [`name_matches_exact`] against any installed name: the program itself, not its vendor.
    pub fn matches_exact(&self, name: &str) -> bool {
        self.names.iter().any(|n| name_matches_exact(name, n))
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

    /// True when an installed location lies under a child of `container`: the publisher folder
    /// is shared with a program that is still installed.
    pub fn owns_sibling_in(&self, container: &Path) -> bool {
        self.holds_location(container)
    }

    /// True when `name` ([`normalize_name`]d) is a run of whole words of an installed name
    /// (`Code` in `Microsoft Visual Studio Code`).
    pub fn is_word_of_name(&self, name: &str) -> bool {
        let n = normalize_name(name);
        let words: Vec<&str> = n.split(' ').filter(|w| !w.is_empty()).collect();
        !words.is_empty()
            && self.names.iter().any(|installed| {
                let tokens: Vec<&str> = installed.split(' ').collect();
                tokens.windows(words.len()).any(|w| w == words.as_slice())
            })
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
    use crate::model::{ProgramId, Scope};

    fn program(name: &str, install: Option<&str>, hidden: bool) -> Program {
        Program {
            account: None,
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
    fn parse_location_roundtrip() {
        for k in [
            RegKeyPath { hive: Hive::LocalMachine, path: r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run".into() },
            RegKeyPath { hive: Hive::CurrentUser, path: r"Software\Classes\CLSID\{0000-1}".into() },
            RegKeyPath { hive: Hive::Users, path: r"S-1-5-21\Software\Foo".into() },
        ] {
            assert_eq!(parse_location(&format_location(&k)), Some(k));
        }
        for s in [r"C:\Menu\Gone.lnk", "GoneSvc", r"Vendor\Updater", "LocalMachine", r"LocalMachine\"] {
            assert_eq!(parse_location(s), None, "{s}");
        }
    }

    #[test]
    fn skip_and_container_names() {
        for n in ["microsoft", "Windows Kits", "WINDOWSAPPS", ".config", "NuGet", "Crashpad", "updater", "Mozilla Maintenance Service", "Microsoft SDKs", "Microsoft VS Code", "SquirrelMachineInstalls", "Chocolatey", "usoshared", "chocoportable", "Downloaded Installations", "Package Cache", "hyper-v", "PCHealthCheck", "RUXIM"] {
            assert!(is_skipped_folder(n), "{n}");
        }
        for n in ["Google", "Old Tool", "Macrosoft"] {
            assert!(!is_skipped_folder(n), "{n}");
        }
        assert!(is_container_folder("google") && !is_container_folder("Epic Games"));
        assert!(in_launcher_library(Path::new(r"C:\Program Files\Epic Games\Fortnite")));
        assert!(in_launcher_library(Path::new(r"D:\SteamLibrary\steamapps\common\X")));
        assert!(!in_launcher_library(Path::new(r"C:\Program Files\Old Tool")));
    }

    #[test]
    fn location_roundtrip_and_legacy_form() {
        let keys = [
            RegKeyPath { hive: Hive::LocalMachine, path: r"SOFTWARE\Classes\CLSID\{1}".into() },
            RegKeyPath { hive: Hive::CurrentUser, path: r"Software\Microsoft\Windows\CurrentVersion\Run".into() },
            RegKeyPath { hive: Hive::Users, path: r"S-1-5-21-9\Software\Foo".into() },
        ];
        let formatted: Vec<String> = keys.iter().map(format_location).collect();
        assert_eq!(
            formatted,
            vec![
                r"HKLM\SOFTWARE\Classes\CLSID\{1}",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                r"HKU\S-1-5-21-9\Software\Foo",
            ]
        );
        for (k, s) in keys.iter().zip(&formatted) {
            assert_eq!(parse_location(s).as_ref(), Some(k), "{s}");
        }
        // The form written before (the hive's Debug name) still parses.
        for (legacy, k) in [
            (r"LocalMachine\SOFTWARE\Classes\CLSID\{1}", &keys[0]),
            (r"CurrentUser\Software\Microsoft\Windows\CurrentVersion\Run", &keys[1]),
            (r"Users\S-1-5-21-9\Software\Foo", &keys[2]),
        ] {
            assert_eq!(parse_location(legacy).as_ref(), Some(k), "{legacy}");
        }
        assert_eq!(parse_location(r"Vendor\Updater"), None);
        assert_eq!(parse_location("HKLM"), None);
        assert_eq!(parse_location(r"HKLM\"), None);
    }

    #[test]
    fn live_refs_hold_equal_or_nested_targets() {
        let live = LiveRefs::from_targets([PathBuf::from(r"c:\program files\Vendor\svc.exe"), PathBuf::from(r"C:\Tools\x.sys")]);
        assert!(live.holds(Path::new(r"C:\Program Files\Vendor")));
        assert!(live.holds(Path::new(r"C:\Tools\x.sys")));
        assert!(!live.holds(Path::new(r"C:\Program Files\Vend")));
        assert!(!live.holds(Path::new(r"C:\Program Files\Vendor\svc.exe\sub")));
    }
}
