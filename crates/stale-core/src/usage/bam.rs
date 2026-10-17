use std::sync::Arc;

use super::filetime::filetime_to_utc;
use super::{UsageHit, UsageProvider, UsageSource};
use crate::devicepath::{device_to_drive_path, DeviceMap};
use crate::registry::{key, RegValue, RegistryReader};
use crate::Hive;

/// 1809+ first, then 1709/1803.
pub const BAM_ROOTS: [&str; 2] = [
    r"SYSTEM\CurrentControlSet\Services\bam\State\UserSettings",
    r"SYSTEM\CurrentControlSet\Services\bam\UserSettings",
];

pub fn parse_bam_value(name: &str, data: &[u8], map: &dyn DeviceMap) -> Option<UsageHit> {
    if name == "Version" || name == "SequenceNumber" || data.len() < 8 {
        return None;
    }
    let exe_path = device_to_drive_path(name, map)?;
    let last_run = filetime_to_utc(u64::from_le_bytes(data[..8].try_into().ok()?))?;
    Some(UsageHit { exe_path, last_run, source: UsageSource::Bam, run_count: None })
}

pub struct BamProvider {
    pub reg: Arc<dyn RegistryReader>,
    pub map: Arc<dyn DeviceMap>,
}

impl UsageProvider for BamProvider {
    fn name(&self) -> &'static str {
        "BAM"
    }

    fn collect(&self) -> Result<Vec<UsageHit>, String> {
        let Some(root) = BAM_ROOTS.iter().find(|r| self.reg.key_exists(&key(Hive::LocalMachine, r))) else {
            return Ok(vec![]);
        };
        let sids = self.reg.subkeys(&key(Hive::LocalMachine, root)).map_err(|e| e.to_string())?;
        let mut hits = Vec::new();
        for sid in sids {
            let k = key(Hive::LocalMachine, &format!(r"{root}\{sid}"));
            let Ok(values) = self.reg.values(&k) else { continue };
            for (name, value) in values {
                let RegValue::Binary(data) = value else { continue };
                if let Some(hit) = parse_bam_value(&name, &data, self.map.as_ref()) {
                    hits.push(hit);
                }
            }
        }
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::*;
    use crate::devicepath::{FakeDeviceMap, WinDeviceMap};
    use crate::registry::{FakeRegistry, WinRegistry};

    const FT: u64 = 133_500_000_000_000_000;

    fn map() -> FakeDeviceMap {
        FakeDeviceMap(HashMap::from([(r"\Device\HarddiskVolume3".to_string(), 'C')]))
    }

    fn data(ft: u64) -> Vec<u8> {
        let mut d = ft.to_le_bytes().to_vec();
        d.extend_from_slice(&[0u8; 16]);
        d
    }

    #[test]
    fn parse_bam_value_reads_filetime_and_skips_metadata() {
        let m = map();
        assert!(parse_bam_value("Version", &data(FT), &m).is_none());
        assert!(parse_bam_value("SequenceNumber", &data(FT), &m).is_none());
        assert!(parse_bam_value(r"\Device\HarddiskVolume3\a.exe", &data(FT)[..7], &m).is_none());
        assert!(parse_bam_value(r"\Device\HarddiskVolume3\a.exe", &data(0), &m).is_none());
        assert!(parse_bam_value("Microsoft.WindowsStore_8wekyb3d8bbwe", &data(FT), &m).is_none());
        let hit = parse_bam_value(r"\Device\HarddiskVolume3\Program Files\Foo\foo.exe", &data(FT), &m).unwrap();
        assert_eq!(hit.exe_path, PathBuf::from(r"C:\Program Files\Foo\foo.exe"));
        assert_eq!(hit.source, UsageSource::Bam);
        assert_eq!(hit.run_count, None);
        let hit = parse_bam_value(r"C:\Apps\bar.exe", &data(FT), &m).unwrap();
        assert_eq!(hit.exe_path, PathBuf::from(r"C:\Apps\bar.exe"));
    }

    #[test]
    fn bam_provider_reads_first_existing_root_for_every_sid() {
        let mut reg = FakeRegistry::new();
        let sid = key(Hive::LocalMachine, &format!(r"{}\S-1-5-21-x", BAM_ROOTS[0]));
        reg.add_key(key(Hive::LocalMachine, BAM_ROOTS[0]));
        reg.add_key(sid.clone());
        reg.set(sid.clone(), r"\Device\HarddiskVolume3\a.exe", RegValue::Binary(data(FT)));
        reg.set(sid.clone(), r"\Device\HarddiskVolume3\b.exe", RegValue::Binary(data(FT)));
        reg.set(sid.clone(), "Version", RegValue::Dword(1));
        reg.set(sid, "SequenceNumber", RegValue::Binary(data(FT)));
        // fallback root must not be read when the first exists
        let old = key(Hive::LocalMachine, &format!(r"{}\S-1-5-21-y", BAM_ROOTS[1]));
        reg.add_key(old.clone());
        reg.set(old, r"\Device\HarddiskVolume3\old.exe", RegValue::Binary(data(FT)));
        let p = BamProvider { reg: Arc::new(reg), map: Arc::new(map()) };
        assert_eq!(p.collect().unwrap().len(), 2);
    }

    #[test]
    fn bam_provider_falls_back_and_handles_missing() {
        let mut reg = FakeRegistry::new();
        let sid = key(Hive::LocalMachine, &format!(r"{}\S-1-5-21-y", BAM_ROOTS[1]));
        reg.add_key(sid.clone());
        reg.set(sid, r"\Device\HarddiskVolume3\old.exe", RegValue::Binary(data(FT)));
        let p = BamProvider { reg: Arc::new(reg), map: Arc::new(map()) };
        assert_eq!(p.collect().unwrap().len(), 1);
        let p = BamProvider { reg: Arc::new(FakeRegistry::new()), map: Arc::new(map()) };
        assert!(p.collect().unwrap().is_empty());
    }

    #[test]
    #[ignore]
    fn bam_live_returns_hits() {
        let p = BamProvider { reg: Arc::new(WinRegistry), map: Arc::new(WinDeviceMap::new()) };
        let hits = p.collect().unwrap();
        println!("{} hits", hits.len());
        for h in hits.iter().take(5) {
            println!("{:?}", h);
        }
        assert!(!hits.is_empty());
    }
}
