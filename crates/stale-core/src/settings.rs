//! User settings persisted as JSON.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub show_hidden: bool,
    pub quarantine_days: u32,
    /// The user interface language, `"sv"` or `"en"`; `None` follows the Windows display language.
    pub language: Option<String>,
    /// The interface zoom factor (0.75..=3.0); `None` (missing, not finite or out of range) means the app's default.
    #[serde(deserialize_with = "valid_ui_scale")]
    pub ui_scale: Option<f32>,
}

fn valid_ui_scale<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<f32>, D::Error> {
    // A wrong type is as good as no value: it must not discard the rest of the file.
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(v.and_then(|v| v.as_f64()).map(|f| f as f32).filter(|f| f.is_finite() && (0.75..=3.0).contains(f)))
}

impl Default for Settings {
    fn default() -> Self {
        Settings { show_hidden: false, quarantine_days: 30, language: None, ui_scale: None }
    }
}

impl Settings {
    /// Defaults when the file is missing or unparsable; unknown fields are ignored. Defaults with
    /// a warning when it is a link or reparse point, larger than 1 MiB, or not owned by
    /// Administrators or SYSTEM (judged on the handle it is read from, see
    /// [`crate::trusted::read_trusted`]).
    pub fn load_from(path: &Path) -> Settings {
        Self::load_from_with(path, &crate::trusted::live_owner_trusted)
    }

    /// [`Self::load_from`] with the owner check `owner_trusted` (tests and tools that read a copy
    /// outside the data folder).
    pub fn load_from_with(path: &Path, owner_trusted: &dyn Fn(&std::fs::File) -> Option<bool>) -> Settings {
        crate::trusted::load_json(path, crate::trusted::MAX_TRUSTED_BYTES, owner_trusted)
    }

    /// Writes pretty JSON to a new file renamed over `path` (parent folders created); refuses a
    /// link or reparse point at `path`.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        crate::trusted::write_replacing(path, serde_json::to_string_pretty(self)?.as_bytes())
    }

    /// `data_dir()\settings.json`.
    pub fn path() -> PathBuf {
        crate::paths::data_dir().join("settings.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trusted::tests::{trust_all, trust_none};

    #[test]
    fn settings_roundtrip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("settings.json");
        assert_eq!(Settings::load_from_with(&path, &trust_all), Settings { show_hidden: false, quarantine_days: 30, language: None, ui_scale: None });
        let s = Settings { show_hidden: true, quarantine_days: 7, language: None, ui_scale: None };
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from_with(&path, &trust_all), s);
    }

    #[test]
    fn untrusted_or_linked_settings_are_defaults_and_never_written_through() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        Settings { show_hidden: true, quarantine_days: 7, language: None, ui_scale: None }.save_to(&path).unwrap();
        assert_eq!(Settings::load_from_with(&path, &trust_none), Settings::default(), "untrusted owner");
        // A junction at the name: defaults, and a save refuses instead of writing through it.
        let target = dir.path().join("elsewhere");
        std::fs::create_dir(&target).unwrap();
        let link = dir.path().join("linked.json");
        assert!(crate::testutil::junction(&link, &target), "mklink /J failed");
        assert_eq!(Settings::load_from_with(&link, &trust_all), Settings::default());
        assert!(Settings::default().save_to(&link).is_err());
        assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
        std::fs::remove_dir(&link).unwrap();
    }

    #[test]
    fn settings_ignores_unknown_and_missing_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"show_hidden":true,"future":1}"#).unwrap();
        assert_eq!(Settings::load_from_with(&path, &trust_all), Settings { show_hidden: true, quarantine_days: 30, language: None, ui_scale: None });
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(Settings::load_from_with(&path, &trust_all), Settings::default());
    }

    #[test]
    fn settings_language_round_trips_and_defaults_to_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"show_hidden":false,"quarantine_days":30}"#).unwrap();
        assert_eq!(Settings::load_from_with(&path, &trust_all).language, None, "an old file has no language");
        let s = Settings { language: Some("en".into()), ..Settings::default() };
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from_with(&path, &trust_all).language.as_deref(), Some("en"));
    }

    #[test]
    fn out_of_range_or_corrupt_scale_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        for bad in ["0.1", "9", "\"big\"", "null", "-1", "1e40"] {
            std::fs::write(&path, format!(r#"{{"quarantine_days":7,"ui_scale":{bad}}}"#)).unwrap();
            let s = Settings::load_from_with(&path, &trust_all);
            assert_eq!(s.ui_scale, None, "{bad}");
            assert_eq!(s.quarantine_days, 7, "{bad}: the rest of the file still loads");
        }
        std::fs::write(&path, r#"{"ui_scale":1.75}"#).unwrap();
        assert_eq!(Settings::load_from_with(&path, &trust_all).ui_scale, Some(1.75));
    }

    #[test]
    fn old_settings_file_without_scale_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"show_hidden":true,"quarantine_days":9,"language":"en"}"#).unwrap();
        let s = Settings::load_from_with(&path, &trust_all);
        assert_eq!((s.show_hidden, s.quarantine_days, s.ui_scale), (true, 9, None));
    }
}
