//! Quarantine manifests on disk: `<manifest dir>\<id>\manifest.json`, with the `.reg` exports and
//! task XML copies of that quarantine next to it.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::{QuarantineId, QuarantineManifest};

const MANIFEST_FILE: &str = "manifest.json";

/// `quarantine_manifest_dir()\<id>\manifest.json`.
pub fn manifest_path(id: &QuarantineId) -> PathBuf {
    crate::paths::quarantine_manifest_dir().join(&id.0).join(MANIFEST_FILE)
}

/// Whether `id` is safe to use as a single folder name (letters, digits and `-` only).
pub(crate) fn valid_id(id: &QuarantineId) -> bool {
    !id.0.is_empty() && id.0.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Writes `dir\<id>\manifest.json` (via a temporary file, so a crash never leaves half a manifest).
pub fn save_manifest(dir: &Path, m: &QuarantineManifest) -> Result<(), String> {
    if !valid_id(&m.id) {
        return Err(format!("invalid quarantine id {:?}", m.id.0));
    }
    let folder = dir.join(&m.id.0);
    std::fs::create_dir_all(&folder).map_err(|e| format!("create {}: {e}", folder.display()))?;
    let json = serde_json::to_string_pretty(m).map_err(|e| e.to_string())?;
    let tmp = folder.join("manifest.json.tmp");
    let file = folder.join(MANIFEST_FILE);
    std::fs::write(&tmp, json).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &file).map_err(|e| format!("write {}: {e}", file.display()))
}

/// Every readable manifest below `dir`, newest first; unreadable or mismatching ones are skipped.
pub fn list_manifests(dir: &Path) -> Vec<QuarantineManifest> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<QuarantineManifest> = rd
        .flatten()
        .filter_map(|e| {
            let text = std::fs::read_to_string(e.path().join(MANIFEST_FILE)).ok()?;
            let m: QuarantineManifest = serde_json::from_str(&text).ok()?;
            (valid_id(&m.id) && e.file_name().to_str() == Some(m.id.0.as_str())).then_some(m)
        })
        .collect();
    out.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.0.cmp(&a.id.0)));
    out
}

/// `"<yyyymmdd-hhmmss>-<4 hex>"`; the suffix is random.
pub fn new_id(now: DateTime<Utc>) -> QuarantineId {
    let random = RandomState::new().hash_one(now.timestamp_nanos_opt().unwrap_or_default()) & 0xffff;
    QuarantineId(format!("{}-{:04x}", now.format("%Y%m%d-%H%M%S"), random))
}
