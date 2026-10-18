//! Leftover data model: what remains on disk/registry after an uninstall.

pub mod fs_scan;
pub mod names;
pub mod other_scan;
pub mod reg_scan;
pub mod scan;

pub use scan::{mark_safe, scan_leftovers, LeftoverSources};

use crate::model::{Program, ProgramId, RegKeyPath};
use crate::pathutil::{drive_letter, first_path_in_command, is_shared_root, norm, normed_under};
use names::name_matches;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// `Leftover::note` of a folder or file that is, holds or lies inside another installed
/// program's location.
pub const NOTE_OTHER_PROGRAM: &str = "used-by-other-program";

/// `Leftover::note` of an install folder with a subfolder that likely holds the user's own files
/// (saves, screenshots, profiles, projects); such a folder is Medium instead of High.
pub const NOTE_CONTAINS_USER_DATA: &str = "contains-user-data";

/// `Leftover::note` of a folder named like the program plus a helper suffix
/// (`obsidian-updater`), see [`names::name_matches_suffixed`].
pub const NOTE_SUFFIX_MATCH: &str = "suffix-match";

/// `Leftover::note` of a file-type key (`.canvas`) whose ProgID is named like the program
/// (`Obsidian Canvas`), and of that ProgID's key.
pub const NOTE_PROGID_OF_PROGRAM: &str = "progid-of-program";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Confidence { High, Medium, Low }

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LeftoverKind {
    Dir { path: PathBuf, size: Option<u64> },
    File { path: PathBuf, size: Option<u64> },
    RegKey { key: RegKeyPath },
    RegValue { key: RegKeyPath, name: String }, // Run/RunOnce/App Paths entries
    Shortcut { lnk: PathBuf, target: PathBuf },
    Service { name: String, image: PathBuf },
    ScheduledTask { name: String, xml: PathBuf, command: PathBuf },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Leftover {
    pub kind: LeftoverKind,
    pub confidence: Confidence,
    /// Language-free key such as "user-folder"; the GUI maps it to text.
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProgramHint {
    pub name: String,
    pub publisher: Option<String>,
    pub install_location: Option<PathBuf>,
    pub uninstall_key: Option<RegKeyPath>,
    pub uninstall_string: Option<String>,
    pub display_icon: Option<PathBuf>,
    /// Install locations, display-icon folders and uninstaller folders of every other visible
    /// installed program ([`norm`]ed); a leftover touching one is never preselected.
    #[serde(default)]
    pub other_locations: Vec<PathBuf>,
}

impl ProgramHint {
    pub fn from_program(p: &Program) -> ProgramHint {
        ProgramHint {
            name: p.name.clone(),
            publisher: p.publisher.clone(),
            install_location: p.install_location.clone(),
            uninstall_key: Some(p.key.clone()),
            uninstall_string: p.uninstall_string.clone(),
            display_icon: p.display_icon.clone(),
            other_locations: Vec::new(),
        }
    }

    /// Adds the locations of every visible program in `programs` except `me` to
    /// `other_locations`: install folder, display-icon folder and uninstaller folder. Shared roots
    /// (`Program Files`, a drive root, ...), paths without a drive letter and anything inside
    /// `%SystemRoot%` (msiexec, rundll32, `Installer` icons) are left out.
    pub fn with_other_programs(mut self, programs: &[Program], me: &ProgramId) -> ProgramHint {
        let system_root = system_root_norm();
        let mut seen: HashSet<String> = self.other_locations.iter().map(|p| norm(p)).collect();
        for p in programs.iter().filter(|p| &p.id != me && !p.hidden) {
            for n in program_locations(p, &system_root) {
                if seen.insert(n.clone()) {
                    self.other_locations.push(PathBuf::from(n));
                }
            }
        }
        self
    }

    /// The folder the program is installed in: `install_location` unless it is empty or a shared
    /// root; else the folder of the uninstaller from `uninstall_string`, else the folder of
    /// `display_icon`, each only when [`Self::exe_dir`] accepts it. Many per-user installers
    /// (electron-builder, Squirrel) write no `InstallLocation`.
    pub fn effective_install_dir(&self) -> Option<PathBuf> {
        if let Some(loc) = self.install_location.as_deref().filter(|p| !p.as_os_str().is_empty() && !is_shared_root(p)) {
            return Some(loc.to_path_buf());
        }
        let uninstaller = self.uninstall_string.as_deref().and_then(first_path_in_command);
        uninstaller
            .as_deref()
            .and_then(|exe| self.exe_dir(exe))
            .or_else(|| self.display_icon.as_deref().and_then(|exe| self.exe_dir(exe)))
    }

    /// `exe`'s folder when `exe` has a drive letter, is not `msiexec` or inside `%SystemRoot%`,
    /// the folder is not a shared root, and either the exe is named like an uninstaller
    /// ([`UNINSTALLER_STEMS`]) or the folder is named like the program.
    fn exe_dir(&self, exe: &Path) -> Option<PathBuf> {
        let stem = exe.file_stem()?.to_string_lossy().to_lowercase();
        let dir = exe.parent()?;
        if drive_letter(exe).is_none()
            || stem == "msiexec"
            || normed_under(&norm(exe), &system_root_norm())
            || is_shared_root(dir)
        {
            return None;
        }
        let named_like_uninstaller = UNINSTALLER_STEMS.iter().any(|u| stem.starts_with(u));
        let dir_named_like_program = dir.file_name().is_some_and(|d| name_matches(&d.to_string_lossy(), &self.name));
        (named_like_uninstaller || dir_named_like_program).then(|| dir.to_path_buf())
    }

    pub fn manual(name: &str, dir: Option<PathBuf>) -> ProgramHint {
        ProgramHint {
            name: name.to_string(),
            publisher: None,
            install_location: dir,
            uninstall_key: None,
            uninstall_string: None,
            display_icon: None,
            other_locations: Vec::new(),
        }
    }
}

/// Lowercase file-name prefixes of uninstallers (`unins000`, `Uninstall Obsidian`, `uninst`,
/// `setup`, `install`): such an exe sits in the program's own folder.
const UNINSTALLER_STEMS: [&str; 5] = ["unins", "uninstall", "uninst", "setup", "install"];

/// `%SystemRoot%` [`norm`]ed (`C:\WINDOWS` when unset).
pub(crate) fn system_root_norm() -> String {
    norm(Path::new(&std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string())))
}

/// The folders `p` occupies, [`norm`]ed: install folder, display-icon folder and uninstaller
/// folder. Shared roots, paths without a drive letter and anything inside `system_root`
/// (msiexec, rundll32, `Installer` icons) are left out.
pub(crate) fn program_locations(p: &Program, system_root: &str) -> Vec<String> {
    let icon_dir = p.display_icon.as_deref().and_then(Path::parent).map(Path::to_path_buf);
    let uninstaller_dir =
        p.uninstall_string.as_deref().and_then(first_path_in_command).and_then(|exe| exe.parent().map(Path::to_path_buf));
    [p.install_location.clone(), icon_dir, uninstaller_dir]
        .into_iter()
        .flatten()
        .filter(|dir| drive_letter(dir).is_some() && !is_shared_root(dir))
        .map(|dir| norm(&dir))
        .filter(|n| n != system_root && !normed_under(n, system_root))
        .collect()
}

/// Folders and files that are, hold or lie inside one of `others` become Low with the note
/// [`NOTE_OTHER_PROGRAM`]: another installed program still uses them.
pub fn mark_used_by_others(items: &mut [Leftover], others: &[PathBuf]) {
    if others.is_empty() {
        return;
    }
    let others: Vec<String> = others.iter().map(|p| norm(p)).collect();
    for it in items {
        let (LeftoverKind::Dir { path, .. } | LeftoverKind::File { path, .. }) = &it.kind else { continue };
        let n = norm(path);
        if others.iter().any(|o| *o == n || normed_under(o, &n) || normed_under(&n, o)) {
            it.confidence = Confidence::Low;
            it.note = Some(NOTE_OTHER_PROGRAM.to_string());
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LeftoverReport { pub items: Vec<Leftover> }

impl Leftover {
    pub fn default_checked(&self) -> bool { self.confidence != Confidence::Low }

    pub fn size(&self) -> Option<u64> {
        match &self.kind {
            LeftoverKind::Dir { size, .. } | LeftoverKind::File { size, .. } => *size,
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Hive, ProgramId, Scope};

    fn leftover(confidence: Confidence) -> Leftover {
        Leftover { kind: LeftoverKind::Dir { path: PathBuf::from(r"C:\x"), size: Some(5) }, confidence, note: None }
    }

    #[test]
    fn default_checked_is_false_only_for_low() {
        assert!(leftover(Confidence::High).default_checked());
        assert!(leftover(Confidence::Medium).default_checked());
        assert!(!leftover(Confidence::Low).default_checked());
        assert_eq!(leftover(Confidence::High).size(), Some(5));
    }

    #[test]
    fn manual_hint_has_empty_publisher() {
        let h = ProgramHint::manual("Foo", None);
        assert_eq!(h.name, "Foo");
        assert!(h.publisher.is_none() && h.install_location.is_none() && h.uninstall_key.is_none());
    }

    #[test]
    fn from_program_copies_fields() {
        let key = RegKeyPath { hive: Hive::LocalMachine, path: r"Software\X".into() };
        let p = Program {
            id: ProgramId("id".into()),
            key: key.clone(),
            name: "Foo".into(),
            publisher: Some("Bar".into()),
            version: None,
            install_location: Some(PathBuf::from(r"C:\Foo")),
            install_date: None,
            uninstall_string: Some("u.exe".into()),
            quiet_uninstall_string: None,
            display_icon: Some(PathBuf::from(r"C:\Foo\f.ico")),
            is_msi: false,
            estimated_size_bytes: None,
            drive: None,
            hidden: false,
            scope: Scope::Machine,
        };
        let h = ProgramHint::from_program(&p);
        assert_eq!(h.name, "Foo");
        assert_eq!(h.publisher.as_deref(), Some("Bar"));
        assert_eq!(h.install_location, Some(PathBuf::from(r"C:\Foo")));
        assert_eq!(h.uninstall_key, Some(key));
        assert_eq!(h.uninstall_string.as_deref(), Some("u.exe"));
        assert_eq!(h.display_icon, Some(PathBuf::from(r"C:\Foo\f.ico")));
        assert!(h.other_locations.is_empty());
    }

    fn other(id: &str, install: Option<&str>, icon: Option<&str>, uninstall: Option<&str>, hidden: bool) -> Program {
        Program {
            id: ProgramId(id.into()),
            key: RegKeyPath { hive: Hive::LocalMachine, path: format!(r"Software\{id}") },
            name: id.into(),
            publisher: None,
            version: None,
            install_location: install.map(PathBuf::from),
            install_date: None,
            uninstall_string: uninstall.map(str::to_string),
            quiet_uninstall_string: None,
            display_icon: icon.map(PathBuf::from),
            is_msi: false,
            estimated_size_bytes: None,
            drive: None,
            hidden,
            scope: Scope::Machine,
        }
    }

    fn hint_with(install: Option<&str>, uninstall: Option<&str>, icon: Option<&str>) -> ProgramHint {
        let mut h = ProgramHint::manual("Obsidian", install.map(PathBuf::from));
        h.uninstall_string = uninstall.map(str::to_string);
        h.display_icon = icon.map(PathBuf::from);
        h
    }

    #[test]
    fn effective_install_dir_from_uninstall_string() {
        let obsidian = r#""C:\Users\Jimmy\AppData\Local\Programs\Obsidian\Uninstall Obsidian.exe" /currentuser"#;
        let want = Some(PathBuf::from(r"C:\Users\Jimmy\AppData\Local\Programs\Obsidian"));
        assert_eq!(hint_with(None, Some(obsidian), None).effective_install_dir(), want);
        // An InstallLocation that is a shared root falls through to the uninstaller too.
        let pf = std::env::var("ProgramFiles").unwrap();
        assert_eq!(hint_with(Some(&pf), Some(obsidian), None).effective_install_dir(), want);
        // A real InstallLocation wins.
        assert_eq!(hint_with(Some(r"D:\Obs"), Some(obsidian), None).effective_install_dir(), Some(PathBuf::from(r"D:\Obs")));
        // Display icon: only in a folder named like the program.
        assert_eq!(
            hint_with(None, None, Some(r"D:\Apps\Obsidian\Obsidian.exe")).effective_install_dir(),
            Some(PathBuf::from(r"D:\Apps\Obsidian"))
        );
        assert_eq!(hint_with(None, None, Some(r"D:\Apps\Bin\Obsidian.exe")).effective_install_dir(), None);
        // An uninstaller without an uninstaller name, in a folder not named like the program.
        assert_eq!(hint_with(None, Some(r#""D:\Cache\{X}\bundle.exe" /uninstall"#), None).effective_install_dir(), None);
    }

    #[test]
    fn effective_install_dir_rejects_msiexec_and_system32() {
        let sys = std::env::var("SystemRoot").unwrap();
        assert_eq!(hint_with(None, Some("MsiExec.exe /X{ABC}"), None).effective_install_dir(), None);
        assert_eq!(hint_with(None, Some(&format!(r"{sys}\System32\MsiExec.exe /X{{ABC}}")), None).effective_install_dir(), None);
        assert_eq!(hint_with(None, Some(&format!(r"{sys}\System32\Obsidian\uninstall.exe")), None).effective_install_dir(), None);
        assert_eq!(hint_with(None, None, Some(&format!(r"{sys}\Installer\{{X}}\setup.exe"))).effective_install_dir(), None);
    }

    #[test]
    fn effective_install_dir_rejects_shared_root_parent() {
        assert_eq!(hint_with(None, Some(r#""C:\Program Files\unins000.exe" /SILENT"#), None).effective_install_dir(), None);
        let local = std::env::var("LOCALAPPDATA").unwrap();
        assert_eq!(hint_with(None, Some(&format!(r"{local}\Programs\uninstall.exe")), None).effective_install_dir(), None);
        assert_eq!(hint_with(None, Some(r"C:\unins000.exe"), None).effective_install_dir(), None);
    }

    #[test]
    fn with_other_programs_collects_other_visible_locations() {
        let programs = [
            other("me", Some(r"C:\Apps\Me"), None, None, false),
            other("a", Some(r"C:\Apps\A\"), Some(r"D:\Icons\a.ico"), Some(r#""E:\Uninst\A\unins000.exe" /S"#), false),
            other("hidden", Some(r"C:\Apps\Hidden"), None, None, true),
            other("msi", Some(r"C:\Program Files"), Some(r"C:\Windows\Installer\{X}\a.ico"), Some("MsiExec.exe /X{X}"), false),
            other("dup", Some(r"c:\apps\a"), None, None, false),
        ];
        let h = ProgramHint::manual("Me", None).with_other_programs(&programs, &ProgramId("me".into()));
        let got: Vec<String> = h.other_locations.iter().map(|p| p.display().to_string()).collect();
        assert_eq!(got, vec![r"C:\APPS\A", r"D:\ICONS", r"E:\UNINST\A"]);
    }
}
