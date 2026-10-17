use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use super::filetime::filetime_to_utc;
use super::{UsageHit, UsageProvider, UsageSource};
use crate::registry::{key, RegValue, RegistryReader};
use crate::Hive;

pub const USERASSIST_EXE_GUID: &str = "{CEBFF5CD-ACE2-4F4F-9178-9926F41749EA}";
pub const USERASSIST_LNK_GUID: &str = "{F4E57C4B-2036-45F0-A9AB-443BCFE33D9F}";

pub fn rot13(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' => (((c as u8 - b'a') + 13) % 26 + b'a') as char,
            'A'..='Z' => (((c as u8 - b'A') + 13) % 26 + b'A') as char,
            _ => c,
        })
        .collect()
}

fn is_absolute_windows(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
}

pub fn parse_userassist_value(
    name_rot13: &str,
    data: &[u8],
    resolve: &dyn Fn(&str) -> Option<PathBuf>,
) -> Option<UsageHit> {
    if data.len() < 68 {
        return None;
    }
    let name = rot13(name_rot13);
    if name.starts_with("UEME_") {
        return None;
    }
    let exe_path = if name.starts_with('{') {
        let end = name.find("}\\")?;
        let base = resolve(&name[..=end])?;
        base.join(&name[end + 2..])
    } else {
        PathBuf::from(&name)
    };
    if !is_absolute_windows(&exe_path.to_string_lossy()) {
        return None;
    }
    let run_count = u32::from_le_bytes(data[4..8].try_into().ok()?);
    let ft = u64::from_le_bytes(data[60..68].try_into().ok()?);
    let last_run = filetime_to_utc(ft)?;
    Some(UsageHit { exe_path, last_run, source: UsageSource::UserAssist, run_count: Some(run_count) })
}

pub struct UserAssistProvider {
    pub reg: Arc<dyn RegistryReader>,
}

impl UserAssistProvider {
    fn collect_with(&self, resolve: &dyn Fn(&str) -> Option<PathBuf>) -> Result<Vec<UsageHit>, String> {
        const BASE: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\UserAssist";
        let mut roots: Vec<(Hive, String)> = vec![(Hive::CurrentUser, String::new())];
        if let Ok(sids) = self.reg.subkeys(&key(Hive::Users, "")) {
            for sid in sids {
                if sid == ".DEFAULT" || sid.ends_with("_Classes") {
                    continue;
                }
                roots.push((Hive::Users, format!(r"{sid}\")));
            }
        }
        let mut hits = Vec::new();
        let mut seen = HashSet::new();
        for (hive, prefix) in roots {
            for guid in [USERASSIST_EXE_GUID, USERASSIST_LNK_GUID] {
                let k = key(hive, &format!(r"{prefix}{BASE}\{guid}\Count"));
                let Ok(values) = self.reg.values(&k) else { continue };
                for (name, value) in values {
                    let RegValue::Binary(data) = value else { continue };
                    let Some(hit) = parse_userassist_value(&name, &data, resolve) else { continue };
                    let id = (hit.exe_path.to_string_lossy().to_lowercase(), hit.last_run);
                    if seen.insert(id) {
                        hits.push(hit);
                    }
                }
            }
        }
        Ok(hits)
    }
}

impl UsageProvider for UserAssistProvider {
    fn name(&self) -> &'static str {
        "UserAssist"
    }
    fn collect(&self) -> Result<Vec<UsageHit>, String> {
        self.collect_with(&crate::knownfolders::resolve)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{key, FakeRegistry, RegValue, WinRegistry};
    use crate::usage::UsageSource;
    use crate::Hive;

    const FT: u64 = 133_500_000_000_000_000;

    fn data(count: u32, ft: u64) -> Vec<u8> {
        let mut d = vec![0u8; 72];
        d[4..8].copy_from_slice(&count.to_le_bytes());
        d[60..68].copy_from_slice(&ft.to_le_bytes());
        d
    }

    #[test]
    fn rot13_roundtrips() {
        assert_eq!(rot13("Zvpebfbsg"), "Microsoft");
        assert_eq!(rot13("{PROSS5PQ-NPR2}"), "{CEBFF5CD-ACE2}");
    }

    #[test]
    fn parses_value_with_filetime_at_60() {
        let hit = parse_userassist_value(&rot13(r"C:\Apps\foo.exe"), &data(7, FT), &|_| None).unwrap();
        assert_eq!(hit.exe_path, PathBuf::from(r"C:\Apps\foo.exe"));
        assert_eq!(hit.run_count, Some(7));
        assert_eq!(hit.source, UsageSource::UserAssist);
    }

    #[test]
    fn resolves_known_folder_prefix() {
        let name = rot13(r"{6D809377-6AF0-444B-8957-A3773F02200E}\Foo\foo.exe");
        let resolve = |g: &str| (g == "{6D809377-6AF0-444B-8957-A3773F02200E}").then(|| PathBuf::from(r"C:\Program Files"));
        let hit = parse_userassist_value(&name, &data(1, FT), &resolve).unwrap();
        assert_eq!(hit.exe_path, PathBuf::from(r"C:\Program Files\Foo\foo.exe"));
        assert!(parse_userassist_value(&name, &data(1, FT), &|_| None).is_none());
    }

    #[test]
    fn skips_ueme_and_short_data_and_zero_filetime() {
        assert!(parse_userassist_value(&rot13("UEME_CTLSESSION"), &data(1, FT), &|_| None).is_none());
        assert!(parse_userassist_value(&rot13(r"C:\a.exe"), &data(1, FT)[..67], &|_| None).is_none());
        assert!(parse_userassist_value(&rot13(r"C:\a.exe"), &data(1, 0), &|_| None).is_none());
    }

    #[test]
    fn skips_non_absolute_names() {
        assert!(parse_userassist_value(&rot13("Microsoft.Windows.Store"), &data(1, FT), &|_| None).is_none());
    }

    fn count_key(root: Hive, sid: &str, guid: &str) -> crate::RegKeyPath {
        let p = format!(r"{sid}\Software\Microsoft\Windows\CurrentVersion\Explorer\UserAssist\{guid}\Count");
        let p = p.trim_start_matches('\\').to_string();
        key(root, &p)
    }

    #[test]
    fn provider_enumerates_hku_dedupes_hkcu_and_skips_classes() {
        let mut reg = FakeRegistry::new();
        let sid = "S-1-5-21-1";
        reg.add_key(key(Hive::Users, ""));
        for s in [sid, ".DEFAULT", "S-1-5-21-1_Classes"] {
            reg.add_key(key(Hive::Users, s));
        }
        let name = rot13(r"C:\Apps\foo.exe");
        // same hit in HKU and HKCU -> counted once
        let k = count_key(Hive::Users, sid, USERASSIST_EXE_GUID);
        reg.add_key(k.clone());
        reg.set(k, &name, RegValue::Binary(data(3, FT)));
        let k = count_key(Hive::CurrentUser, "", USERASSIST_EXE_GUID);
        reg.add_key(k.clone());
        reg.set(k, &name, RegValue::Binary(data(3, FT)));
        // ignored hives
        for s in [".DEFAULT", "S-1-5-21-1_Classes"] {
            let k = count_key(Hive::Users, s, USERASSIST_LNK_GUID);
            reg.add_key(k.clone());
            reg.set(k, &rot13(r"C:\Apps\ignored.exe"), RegValue::Binary(data(1, FT)));
        }
        // lnk GUID of the real SID
        let k = count_key(Hive::Users, sid, USERASSIST_LNK_GUID);
        reg.add_key(k.clone());
        reg.set(k, &rot13(r"C:\Apps\bar.lnk.exe"), RegValue::Binary(data(2, FT)));

        let p = UserAssistProvider { reg: Arc::new(reg) };
        let hits = p.collect_with(&|_| None).unwrap();
        let mut paths: Vec<_> = hits.iter().map(|h| h.exe_path.to_string_lossy().to_string()).collect();
        paths.sort();
        assert_eq!(paths, vec![r"C:\Apps\bar.lnk.exe", r"C:\Apps\foo.exe"]);
    }

    #[test]
    #[ignore]
    fn userassist_live_returns_hits() {
        let p = UserAssistProvider { reg: Arc::new(WinRegistry) };
        let hits = p.collect().unwrap();
        println!("{} hits", hits.len());
        for h in hits.iter().take(5) {
            println!("{:?}", h);
        }
        assert!(!hits.is_empty());
    }
}
