//! User settings persisted as JSON.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub show_hidden: bool,
    pub quarantine_days: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { show_hidden: false, quarantine_days: 30 }
    }
}

impl Settings {
    /// Defaults when the file is missing or unparsable; unknown fields are ignored.
    pub fn load_from(path: &Path) -> Settings {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Writes pretty JSON, creating parent folders.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)
    }

    /// `data_dir()\settings.json`.
    pub fn path() -> PathBuf {
        crate::paths::data_dir().join("settings.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_roundtrip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("settings.json");
        assert_eq!(Settings::load_from(&path), Settings { show_hidden: false, quarantine_days: 30 });
        let s = Settings { show_hidden: true, quarantine_days: 7 };
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), s);
    }

    #[test]
    fn settings_ignores_unknown_and_missing_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"show_hidden":true,"future":1}"#).unwrap();
        assert_eq!(Settings::load_from(&path), Settings { show_hidden: true, quarantine_days: 30 });
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
    }
}
