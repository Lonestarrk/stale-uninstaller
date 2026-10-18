//! Leftover data model: what remains on disk/registry after an uninstall.

pub mod fs_scan;
pub mod names;
pub mod reg_scan;

use crate::model::{Program, RegKeyPath};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
        }
    }

    pub fn manual(name: &str, dir: Option<PathBuf>) -> ProgramHint {
        ProgramHint {
            name: name.to_string(),
            publisher: None,
            install_location: dir,
            uninstall_key: None,
            uninstall_string: None,
            display_icon: None,
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
    }
}
