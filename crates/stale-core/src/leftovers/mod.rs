//! Leftover data model: what remains on disk/registry after an uninstall.

pub mod fs_scan;
pub mod names;
pub mod other_scan;
pub mod reg_scan;
pub mod scan;

pub use scan::{mark_safe, scan_leftovers, LeftoverSources};

use crate::model::{Program, ProgramId, RegKeyPath};
use crate::pathutil::{drive_letter, first_path_in_command, is_shared_root, norm, normed_under};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// `Leftover::note` of a folder or file that is, holds or lies inside another installed
/// program's location.
pub const NOTE_OTHER_PROGRAM: &str = "used-by-other-program";

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
        let system_root = norm(Path::new(&std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string())));
        let mut seen: HashSet<String> = self.other_locations.iter().map(|p| norm(p)).collect();
        for p in programs.iter().filter(|p| &p.id != me && !p.hidden) {
            let icon_dir = p.display_icon.as_deref().and_then(Path::parent).map(Path::to_path_buf);
            let uninstaller_dir = p
                .uninstall_string
                .as_deref()
                .and_then(first_path_in_command)
                .and_then(|exe| exe.parent().map(Path::to_path_buf));
            for dir in [p.install_location.clone(), icon_dir, uninstaller_dir].into_iter().flatten() {
                let n = norm(&dir);
                if drive_letter(&dir).is_none() || is_shared_root(&dir) || n == system_root || normed_under(&n, &system_root) {
                    continue;
                }
                if seen.insert(n.clone()) {
                    self.other_locations.push(PathBuf::from(n));
                }
            }
        }
        self
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
