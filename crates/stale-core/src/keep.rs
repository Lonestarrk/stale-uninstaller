//! The user's keep list: paths, registry keys and program names the leftover scanner must not offer.

use crate::leftovers::names::normalize_name;
use crate::model::RegKeyPath;
use crate::pathutil::norm;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeepList {
    pub paths: Vec<PathBuf>,
    pub keys: Vec<RegKeyPath>,
    pub names: Vec<String>,
}

fn under(kept: &str, p: &str) -> bool {
    p == kept || (p.len() > kept.len() && p.starts_with(kept) && p.as_bytes()[kept.len()] == b'\\')
}

fn key_str(k: &RegKeyPath) -> String {
    k.path.trim_matches('\\').to_lowercase()
}

fn same_key(a: &RegKeyPath, b: &RegKeyPath) -> bool {
    a.hive == b.hive && key_str(a) == key_str(b)
}

impl KeepList {
    /// Defaults when the file is missing or unparsable; unknown fields are ignored.
    pub fn load_from(path: &Path) -> KeepList {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Writes pretty JSON atomically (`<path>.tmp`, flushed to disk, then renamed over `path`).
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let result = (|| {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(serde_json::to_string_pretty(self)?.as_bytes())?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    /// True when `p` equals a kept path or lies strictly under one (compared with [`norm`]).
    pub fn is_path_kept(&self, p: &Path) -> bool {
        let p = norm(p);
        self.paths.iter().any(|k| under(&norm(k), &p))
    }

    /// True when `k` equals a kept key or lies under one (same hive, case-insensitive).
    pub fn is_key_kept(&self, k: &RegKeyPath) -> bool {
        let p = key_str(k);
        self.keys.iter().any(|kept| {
            kept.hive == k.hive && {
                let kp = key_str(kept);
                !kp.is_empty() && under(&kp, &p)
            }
        })
    }

    pub fn is_name_kept(&self, name: &str) -> bool {
        let n = normalize_name(name);
        self.names.iter().any(|k| normalize_name(k) == n)
    }

    /// Adds `p` unless already kept; a drive root or a path without a real component (`C:\`, `C:`,
    /// empty) is ignored, since it would hide every leftover on the drive.
    pub fn keep_path(&mut self, p: PathBuf) {
        let has_component = p.components().any(|c| matches!(c, Component::Normal(_)));
        if has_component && !self.paths.iter().any(|k| norm(k) == norm(&p)) {
            self.paths.push(p);
        }
    }

    /// Adds `k` unless already kept; an empty key path (the whole hive) is ignored.
    pub fn keep_key(&mut self, k: RegKeyPath) {
        if !key_str(&k).is_empty() && !self.keys.iter().any(|x| same_key(x, &k)) {
            self.keys.push(k);
        }
    }

    pub fn keep_name(&mut self, n: String) {
        if !self.is_name_kept(&n) {
            self.names.push(n);
        }
    }

    pub fn remove_path(&mut self, p: &Path) {
        self.paths.retain(|k| norm(k) != norm(p));
    }

    pub fn remove_key(&mut self, k: &RegKeyPath) {
        self.keys.retain(|x| !same_key(x, k));
    }

    pub fn remove_name(&mut self, n: &str) {
        let n = normalize_name(n);
        self.names.retain(|k| normalize_name(k) != n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::leftovers::scan::filter_kept;
    use crate::leftovers::{Confidence, Leftover, LeftoverKind, LeftoverReport};
    use crate::model::Hive;
    use crate::registry::key;
    use std::path::{Path, PathBuf};

    #[test]
    fn keep_list_roundtrip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("important.json");
        assert_eq!(KeepList::load_from(&path), KeepList::default());
        let mut k = KeepList::default();
        k.keep_path(PathBuf::from(r"C:\Apps\Foo"));
        k.keep_key(key(Hive::CurrentUser, r"Software\Foo"));
        k.keep_name("Foo".to_string());
        k.save_to(&path).unwrap();
        assert_eq!(KeepList::load_from(&path), k);
        assert!(!path.with_extension("json.tmp").exists());
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(KeepList::load_from(&path), KeepList::default());
        std::fs::write(&path, r#"{"names":["x"],"future":1}"#).unwrap();
        assert_eq!(KeepList::load_from(&path).names, vec!["x".to_string()]);
    }

    #[test]
    fn is_path_kept_requires_boundary() {
        let mut k = KeepList::default();
        k.keep_path(PathBuf::from(r"C:\Apps\Foo"));
        assert!(k.is_path_kept(Path::new(r"c:\apps\foo")));
        assert!(k.is_path_kept(Path::new(r"C:\Apps\Foo\x")));
        assert!(!k.is_path_kept(Path::new(r"C:\Apps\Foobar")));
        assert!(!k.is_path_kept(Path::new(r"C:\Apps")));
    }

    #[test]
    fn is_key_kept_under() {
        let mut k = KeepList::default();
        k.keep_key(key(Hive::CurrentUser, r"Software\Foo"));
        assert!(k.is_key_kept(&key(Hive::CurrentUser, r"software\FOO")));
        assert!(k.is_key_kept(&key(Hive::CurrentUser, r"Software\Foo\Bar")));
        assert!(!k.is_key_kept(&key(Hive::CurrentUser, r"Software\Foobar")));
        assert!(!k.is_key_kept(&key(Hive::LocalMachine, r"Software\Foo")));
        k.keep_name("Unity Hub 3.1".to_string());
        assert!(k.is_name_kept("unity hub"));
        assert!(!k.is_name_kept("Other"));
    }

    fn item(kind: LeftoverKind) -> Leftover {
        Leftover { kind, confidence: Confidence::High, note: None }
    }

    #[test]
    fn filter_kept_drops_items_under_kept_paths_and_keys() {
        let mut k = KeepList::default();
        k.keep_path(PathBuf::from(r"C:\Keep"));
        k.keep_key(key(Hive::CurrentUser, r"Software\Keep"));
        let gone = |p: &str| PathBuf::from(p);
        let drop = vec![
            LeftoverKind::Dir { path: gone(r"C:\Keep\d"), size: None },
            LeftoverKind::File { path: gone(r"C:\Keep\f.txt"), size: None },
            LeftoverKind::RegKey { key: key(Hive::CurrentUser, r"Software\Keep\Sub") },
            LeftoverKind::RegValue { key: key(Hive::CurrentUser, r"Software\Keep"), name: "v".into() },
            LeftoverKind::Shortcut { lnk: gone(r"C:\Menu\a.lnk"), target: gone(r"C:\Keep\a.exe") },
            LeftoverKind::Service { name: "s".into(), image: gone(r"C:\Keep\s.exe") },
            LeftoverKind::ScheduledTask { name: "t".into(), xml: gone(r"C:\T\t"), command: gone(r"C:\Keep\t.exe") },
        ];
        let stay = vec![
            LeftoverKind::Dir { path: gone(r"C:\Keeper"), size: None },
            LeftoverKind::RegKey { key: key(Hive::CurrentUser, r"Software\Other") },
            LeftoverKind::Shortcut { lnk: gone(r"C:\Menu\Keep.lnk"), target: gone(r"C:\Other\a.exe") },
            LeftoverKind::Service { name: "s2".into(), image: gone(r"C:\Other\s.exe") },
        ];
        let items = drop.into_iter().chain(stay.clone()).map(item).collect();
        let out = filter_kept(LeftoverReport { items }, &k);
        assert_eq!(out.items.into_iter().map(|l| l.kind).collect::<Vec<_>>(), stay);
    }

    #[test]
    fn keep_path_rejects_drive_root() {
        let mut k = KeepList::default();
        for p in [r"C:\", "C:", "", r"\", "c:/"] {
            k.keep_path(PathBuf::from(p));
        }
        assert!(k.paths.is_empty(), "{:?}", k.paths);
        assert!(!k.is_path_kept(Path::new(r"C:\Apps\Foo")));
        k.keep_path(PathBuf::from(r"C:\Apps"));
        assert_eq!(k.paths.len(), 1);
    }

    #[test]
    fn keep_key_rejects_empty_path() {
        let mut k = KeepList::default();
        k.keep_key(key(Hive::CurrentUser, ""));
        k.keep_key(key(Hive::CurrentUser, r"\"));
        assert!(k.keys.is_empty(), "{:?}", k.keys);
        // A hand-edited empty entry no longer keeps the whole hive.
        k.keys.push(key(Hive::CurrentUser, ""));
        assert!(!k.is_key_kept(&key(Hive::CurrentUser, r"Software\Foo")));
    }

    #[test]
    fn keep_list_dedupes() {
        let mut k = KeepList::default();
        k.keep_path(PathBuf::from(r"C:\A"));
        k.keep_path(PathBuf::from(r"c:\a\"));
        k.keep_key(key(Hive::CurrentUser, "S"));
        k.keep_key(key(Hive::CurrentUser, "s"));
        k.keep_name("Foo".into());
        k.keep_name("foo 1.0".into());
        assert_eq!((k.paths.len(), k.keys.len(), k.names.len()), (1, 1, 1));
        k.remove_path(Path::new(r"C:\A"));
        k.remove_key(&key(Hive::CurrentUser, "S"));
        k.remove_name("FOO");
        assert_eq!(k, KeepList::default());
    }
}
