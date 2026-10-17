use std::collections::BTreeMap;

use super::{RegError, RegValue, RegistryReader};
use crate::model::{Hive, RegKeyPath};

const SEP: char = '\\';

#[derive(Default)]
struct Node {
    /// Original-case name of the last path segment.
    name: String,
    /// Keyed by lowercased value name; holds the original-case name.
    values: BTreeMap<String, (String, RegValue)>,
}

/// In-memory registry for tests. Paths are case-insensitive; setting a value
/// or adding a key implicitly creates all parent keys. An empty path is the
/// hive root, which always exists.
#[derive(Default)]
pub struct FakeRegistry {
    /// Keyed by lowercased `"{hive:?}\{path}"` (hive root has no trailing slash).
    keys: BTreeMap<String, Node>,
}

fn hive_prefix(hive: Hive) -> String {
    format!("{hive:?}").to_lowercase()
}

fn norm(key: &RegKeyPath) -> String {
    let path = key.path.trim_matches(SEP);
    if path.is_empty() {
        hive_prefix(key.hive)
    } else {
        format!("{}{SEP}{}", hive_prefix(key.hive), path.to_lowercase())
    }
}

impl FakeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_key(&mut self, key: RegKeyPath) {
        let path = key.path.trim_matches(SEP);
        let mut acc = hive_prefix(key.hive);
        for seg in path.split(SEP).filter(|s| !s.is_empty()) {
            acc.push(SEP);
            acc.push_str(&seg.to_lowercase());
            self.keys
                .entry(acc.clone())
                .or_insert_with(|| Node { name: seg.to_string(), ..Node::default() });
        }
    }

    pub fn set(&mut self, key: RegKeyPath, name: &str, value: RegValue) {
        self.add_key(key.clone());
        if let Some(node) = self.keys.get_mut(&norm(&key)) {
            node.values.insert(name.to_lowercase(), (name.to_string(), value));
        }
        // Setting a value on the hive root is not supported (nothing to store it in).
    }
}

impl RegistryReader for FakeRegistry {
    fn subkeys(&self, key: &RegKeyPath) -> Result<Vec<String>, RegError> {
        if !self.key_exists(key) {
            return Err(RegError::NotFound);
        }
        let prefix = format!("{}{SEP}", norm(key));
        Ok(self
            .keys
            .range(prefix.clone()..)
            .take_while(|(k, _)| k.starts_with(&prefix))
            .filter(|(k, _)| !k[prefix.len()..].contains(SEP))
            .map(|(_, n)| n.name.clone())
            .collect())
    }

    fn values(&self, key: &RegKeyPath) -> Result<Vec<(String, RegValue)>, RegError> {
        if key.path.trim_matches(SEP).is_empty() {
            return Ok(Vec::new());
        }
        let node = self.keys.get(&norm(key)).ok_or(RegError::NotFound)?;
        Ok(node.values.values().cloned().collect())
    }

    fn key_exists(&self, key: &RegKeyPath) -> bool {
        key.path.trim_matches(SEP).is_empty() || self.keys.contains_key(&norm(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Hive;
    use crate::registry::{key, RegError, RegValue, RegistryReader};

    #[test]
    fn fake_lists_subkeys_and_values() {
        let mut r = FakeRegistry::new();
        r.set(key(Hive::LocalMachine, r"SOFTWARE\A\B"), "DisplayName", RegValue::Str("B".into()));
        assert_eq!(r.subkeys(&key(Hive::LocalMachine, r"SOFTWARE\A")).unwrap(), vec!["B".to_string()]);
        assert!(r.key_exists(&key(Hive::LocalMachine, r"SOFTWARE\A")));
        assert!(matches!(r.subkeys(&key(Hive::LocalMachine, r"SOFTWARE\Nope")), Err(RegError::NotFound)));
        assert_eq!(
            r.values(&key(Hive::LocalMachine, r"SOFTWARE\A\B")).unwrap(),
            vec![("DisplayName".to_string(), RegValue::Str("B".into()))]
        );
    }

    #[test]
    fn fake_paths_are_case_insensitive() {
        let mut r = FakeRegistry::new();
        r.set(key(Hive::CurrentUser, r"software\a"), "X", RegValue::Dword(1));
        assert!(r.key_exists(&key(Hive::CurrentUser, r"SOFTWARE\A")));
        assert_eq!(
            r.values(&key(Hive::CurrentUser, r"SOFTWARE\A")).unwrap(),
            vec![("X".to_string(), RegValue::Dword(1))]
        );
        assert!(!r.key_exists(&key(Hive::LocalMachine, r"SOFTWARE\A")));
    }

    #[test]
    fn fake_empty_path_is_hive_root() {
        let mut r = FakeRegistry::new();
        r.add_key(key(Hive::Users, r"S-1-5-21-1\Software"));
        r.add_key(key(Hive::Users, r"S-1-5-21-2"));
        assert!(r.key_exists(&key(Hive::Users, "")));
        let mut sids = r.subkeys(&key(Hive::Users, "")).unwrap();
        sids.sort();
        assert_eq!(sids, vec!["S-1-5-21-1".to_string(), "S-1-5-21-2".to_string()]);
        assert!(r.subkeys(&key(Hive::CurrentUser, "")).unwrap().is_empty());
    }
}
