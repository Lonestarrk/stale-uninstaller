//! Quarantine: leftovers moved aside (files) or exported and deleted (registry) so they can be restored.

pub mod fsmove;
pub mod ops;
pub mod store;

pub use ops::{auto_purge, DRIVE_MISSING, UNKNOWN_CONTENT_LEFT, purge, quarantine, restore, QuarantineOps, QuarantineProgress, RestoreOutcome};
pub use store::{list_manifests, list_trusted_manifests, manifest_path, new_id, owned_by_admins};

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::model::RegKeyPath;
use crate::pathutil::drive_letter;

/// `"<yyyymmdd-hhmmss>-<4 hex>"`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuarantineId(pub String);

/// A file or folder moved from `original` to `stored`; `size` is in bytes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MovedItem {
    pub original: PathBuf,
    pub stored: PathBuf,
    pub is_dir: bool,
    pub size: u64,
}

/// A registry key exported to `reg_file` and then deleted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportedKey {
    pub key: RegKeyPath,
    pub reg_file: PathBuf,
}

/// A single value deleted from `key`; `reg_file` is the export of the whole key taken before.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemovedValue {
    pub key: RegKeyPath,
    pub name: String,
    pub reg_file: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemovedService {
    pub name: String,
    pub reg_file: PathBuf,
}

/// A scheduled task: `xml_backup` is the copy of its definition, `original_xml` where it lived.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemovedTask {
    pub name: String,
    pub xml_backup: PathBuf,
    pub original_xml: PathBuf,
}

/// Something that could not be quarantined; `what` names it for the user.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    pub what: String,
    pub error: String,
}

/// A destructive step that was about to run when the manifest was last saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PendingKind {
    /// A file, folder or shortcut being moved from `original` to `stored`.
    Move { original: PathBuf, stored: PathBuf },
    /// A key exported to `reg_file`, being deleted.
    KeyExport { key: RegKeyPath, reg_file: PathBuf },
    /// Value `name` of `key` (whose export is `reg_file`), being deleted.
    ValueExport { key: RegKeyPath, name: String, reg_file: PathBuf },
    /// A service exported to `reg_file`, being stopped and deleted.
    ServiceExport { name: String, reg_file: PathBuf },
    /// A task backed up to `xml_backup`, being deleted.
    TaskBackup { name: String, xml_backup: PathBuf },
}

/// Recorded (and saved) before a destructive step and replaced by its outcome after it, so a
/// crash in between leaves a trace that restore and purge can act on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingItem {
    pub kind: PendingKind,
    pub started: DateTime<Utc>,
}

/// Everything one quarantine did, enough to restore or purge it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuarantineManifest {
    pub id: QuarantineId,
    pub program_name: String,
    pub created: DateTime<Utc>,
    pub moved: Vec<MovedItem>,
    pub keys: Vec<ExportedKey>,
    pub values: Vec<RemovedValue>,
    pub services: Vec<RemovedService>,
    pub tasks: Vec<RemovedTask>,
    pub failures: Vec<Failure>,
    /// Language-free notes for the GUI, e.g. `"service-needs-reboot:<name>"` after a restore.
    #[serde(default)]
    pub notes: Vec<String>,
    /// Steps started but not recorded as done or failed (a crash or a failed save in between).
    #[serde(default)]
    pub pending: Vec<PendingItem>,
}

impl QuarantineManifest {
    /// Bytes held per drive letter, by where each item is stored (its original drive if `stored`
    /// has none). Items with no drive letter at all are left out.
    pub fn size_per_drive(&self) -> BTreeMap<char, u64> {
        let mut per = BTreeMap::new();
        for m in &self.moved {
            if let Some(d) = drive_letter(&m.stored).or_else(|| drive_letter(&m.original)) {
                *per.entry(d).or_insert(0) += m.size;
            }
        }
        per
    }

    /// Bytes of all moved items.
    pub fn total_size(&self) -> u64 {
        self.moved.iter().map(|m| m.size).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::quarantine_payload_root;
    use std::path::{Path, PathBuf};

    fn moved(original: &str, stored: &str, size: u64) -> MovedItem {
        MovedItem { original: PathBuf::from(original), stored: PathBuf::from(stored), is_dir: true, size }
    }

    #[test]
    fn manifest_size_per_drive_sums_moved_items() {
        let m = QuarantineManifest {
            id: QuarantineId("20261018-120000-ab12".to_string()),
            program_name: "Unity Hub".to_string(),
            created: chrono::Utc::now(),
            moved: vec![
                moved(r"D:\Program Files\Unity Hub", r"D:\Stale Uninstaller\quarantine\x\D\Program Files\Unity Hub", 100),
                moved(r"C:\Users\A\AppData\Roaming\UnityHub", r"c:\Stale Uninstaller\quarantine\x\C\Users\A\AppData\Roaming\UnityHub", 20),
                moved(r"C:\ProgramData\Unity", r"C:\Stale Uninstaller\quarantine\x\C\ProgramData\Unity", 5),
                moved(r"E:\Unity", r"\?\odd\stored", 7),
                moved(r"\server\share\Unity", r"\server\share\stored", 1000),
            ],
            keys: vec![],
            values: vec![],
            services: vec![],
            tasks: vec![],
            failures: vec![],
            notes: vec![],
            pending: vec![],
        };
        let per = m.size_per_drive();
        assert_eq!(per.into_iter().collect::<Vec<_>>(), vec![('C', 25), ('D', 100), ('E', 7)]);
        assert_eq!(m.total_size(), 1132);
    }

    #[test]
    fn payload_root_uses_same_drive() {
        assert_eq!(quarantine_payload_root('d'), PathBuf::from(r"D:\Stale Uninstaller\quarantine"));
        assert_eq!(quarantine_payload_root('C'), PathBuf::from(r"C:\Stale Uninstaller\quarantine"));
        let id = QuarantineId("20261018-120000-ab12".to_string());
        let original = Path::new(r"D:\Program Files\Unity Hub");
        let drive = crate::pathutil::drive_letter(original).unwrap();
        assert_eq!(
            fsmove::stored_path_for(&quarantine_payload_root(drive), &id, original),
            Some(PathBuf::from(r"D:\Stale Uninstaller\quarantine\20261018-120000-ab12\D\Program Files\Unity Hub"))
        );
    }
}
