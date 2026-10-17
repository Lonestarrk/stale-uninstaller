use std::collections::HashMap;
use std::path::PathBuf;

use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::QueryDosDeviceW;

/// Maps NT device names (`\Device\HarddiskVolume3`) to drive letters.
pub trait DeviceMap: Send + Sync {
    fn drive_for_device(&self, device: &str) -> Option<char>;
}

/// Device map built once from `QueryDosDeviceW` over `A:`..`Z:`; keys are lowercased.
pub struct WinDeviceMap {
    map: HashMap<String, char>,
}

impl WinDeviceMap {
    pub fn new() -> Self {
        let mut map = HashMap::new();
        for letter in 'A'..='Z' {
            let name: Vec<u16> = format!("{letter}:").encode_utf16().chain(std::iter::once(0)).collect();
            let mut buf = [0u16; 1024];
            // SAFETY: `name` is NUL-terminated and outlives the call; `buf` is a valid writable slice.
            let n = unsafe { QueryDosDeviceW(PCWSTR(name.as_ptr()), Some(&mut buf)) };
            if n == 0 {
                continue;
            }
            let first = buf[..n as usize].split(|&c| c == 0).next().unwrap_or(&[]);
            if first.is_empty() {
                continue;
            }
            map.entry(String::from_utf16_lossy(first).to_lowercase()).or_insert(letter);
        }
        Self { map }
    }
}

impl Default for WinDeviceMap {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceMap for WinDeviceMap {
    fn drive_for_device(&self, device: &str) -> Option<char> {
        self.map.get(&device.to_lowercase()).copied()
    }
}

pub struct FakeDeviceMap(pub HashMap<String, char>);

impl DeviceMap for FakeDeviceMap {
    fn drive_for_device(&self, device: &str) -> Option<char> {
        let d = device.to_lowercase();
        self.0.iter().find(|(k, _)| k.to_lowercase() == d).map(|(_, c)| *c)
    }
}

/// `\Device\HarddiskVolume3\x\y.exe` -> `C:\x\y.exe`. Already-drive paths pass through;
/// `\??\C:\...` is stripped to a drive path; anything else yields `None`.
pub fn device_to_drive_path(p: &str, map: &dyn DeviceMap) -> Option<PathBuf> {
    let p = p.strip_prefix(r"\??\").unwrap_or(p);
    let b = p.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\' {
        return Some(PathBuf::from(p));
    }
    let prefix = r"\Device\";
    if !p.is_char_boundary(prefix.len()) || !p[..prefix.len()].eq_ignore_ascii_case(prefix) {
        return None;
    }
    // The device name must be followed by `\`, so Volume1 never matches Volume10.
    let dev_end = prefix.len() + p[prefix.len()..].find('\\')?;
    let letter = map.drive_for_device(&p[..dev_end])?;
    Some(PathBuf::from(format!("{}:{}", letter.to_ascii_uppercase(), &p[dev_end..])))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake() -> FakeDeviceMap {
        let mut m = HashMap::new();
        m.insert(r"\Device\HarddiskVolume3".to_string(), 'C');
        m.insert(r"\Device\HarddiskVolume10".to_string(), 'D');
        FakeDeviceMap(m)
    }

    #[test]
    fn device_path_maps_to_drive() {
        let m = fake();
        assert_eq!(
            device_to_drive_path(r"\Device\HarddiskVolume3\Program Files\x.exe", &m),
            Some(PathBuf::from(r"C:\Program Files\x.exe"))
        );
        assert_eq!(device_to_drive_path(r"\device\harddiskvolume10\a.exe", &m), Some(PathBuf::from(r"D:\a.exe")));
        assert_eq!(device_to_drive_path(r"\Device\HarddiskVolume9\x.exe", &m), None);
        assert_eq!(device_to_drive_path(r"\Device\HarddiskVolume3", &m), None);
        assert_eq!(device_to_drive_path(r"C:\Apps\x.exe", &m), Some(PathBuf::from(r"C:\Apps\x.exe")));
        assert_eq!(device_to_drive_path(r"\??\C:\Apps\x.exe", &m), Some(PathBuf::from(r"C:\Apps\x.exe")));
        assert_eq!(device_to_drive_path("Microsoft.WindowsStore_8wekyb3d8bbwe", &m), None);
    }

    #[test]
    fn device_name_prefix_does_not_cross_match() {
        let mut m = HashMap::new();
        m.insert(r"\Device\HarddiskVolume1".to_string(), 'C');
        let m = FakeDeviceMap(m);
        assert_eq!(device_to_drive_path(r"\Device\HarddiskVolume10\a.exe", &m), None);
    }

    #[test]
    #[ignore]
    fn win_device_map_has_system_drive() {
        let m = WinDeviceMap::new();
        assert!(m.map.values().any(|&c| c == 'C'));
    }
}
