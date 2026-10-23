use std::io;

use winreg::enums::{
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, HKEY_USERS, KEY_READ, KEY_WOW64_64KEY, REG_BINARY,
    REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ, REG_QWORD, REG_SZ,
};
use winreg::types::FromRegValue;
use winreg::RegKey;

use super::{RegError, RegValue, RegistryReader};
use crate::model::{Hive, RegKeyPath};

const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_ACCESS_DENIED: i32 = 5;

/// Read-only access to the real Windows registry (64-bit view).
pub struct WinRegistry;

fn map_err(e: io::Error) -> RegError {
    match e.raw_os_error() {
        Some(ERROR_FILE_NOT_FOUND) => RegError::NotFound,
        Some(ERROR_ACCESS_DENIED) => RegError::AccessDenied,
        _ => RegError::Other(e.to_string()),
    }
}

fn open(key: &RegKeyPath) -> Result<RegKey, RegError> {
    let root = RegKey::predef(match key.hive {
        Hive::LocalMachine => HKEY_LOCAL_MACHINE,
        Hive::CurrentUser => HKEY_CURRENT_USER,
        Hive::Users => HKEY_USERS,
    });
    let path = key.path.trim_matches('\\');
    if path.is_empty() {
        return Ok(root);
    }
    root.open_subkey_with_flags(path, KEY_READ | KEY_WOW64_64KEY).map_err(map_err)
}

fn convert(v: &winreg::RegValue) -> RegValue {
    let s = |f: fn(&winreg::RegValue) -> io::Result<String>| f(v);
    match v.vtype {
        REG_SZ => s(String::from_reg_value).map(RegValue::Str).unwrap_or(RegValue::Other),
        REG_EXPAND_SZ => s(String::from_reg_value).map(RegValue::ExpandStr).unwrap_or(RegValue::Other),
        REG_MULTI_SZ => Vec::<String>::from_reg_value(v).map(RegValue::MultiStr).unwrap_or(RegValue::Other),
        REG_DWORD => u32::from_reg_value(v).map(RegValue::Dword).unwrap_or(RegValue::Other),
        REG_QWORD => u64::from_reg_value(v).map(RegValue::Qword).unwrap_or(RegValue::Other),
        REG_BINARY => RegValue::Binary(v.bytes.to_vec()),
        _ => RegValue::Other,
    }
}

impl RegistryReader for WinRegistry {
    fn subkeys(&self, key: &RegKeyPath) -> Result<Vec<String>, RegError> {
        open(key)?.enum_keys().map(|r| r.map_err(map_err)).collect()
    }

    fn values(&self, key: &RegKeyPath) -> Result<Vec<(String, RegValue)>, RegError> {
        open(key)?
            .enum_values()
            .map(|r| r.map(|(n, v)| (n, convert(&v))).map_err(map_err))
            .collect()
    }

    fn key_exists(&self, key: &RegKeyPath) -> bool {
        open(key).is_ok()
    }
}

/// Read-only access to a hive file loaded as an application hive (`RegLoadAppKeyW`, `KEY_READ`).
/// Keys are addressed by their path inside the hive; the `hive` field of a [`RegKeyPath`] is
/// ignored. Dropping it closes the key, which unloads the hive once nothing else holds it.
pub struct HiveFileReader {
    root: RegKey,
}

// SAFETY: the registry functions may be called on one HKEY from any thread; the reader only
// opens subkeys and enumerates, and the key is closed once, on drop.
unsafe impl Send for HiveFileReader {}
// SAFETY: as above; no method mutates the reader.
unsafe impl Sync for HiveFileReader {}

impl HiveFileReader {
    /// Loads `file` read-only. The file must be Stale's own copy: the registry keeps it open.
    pub fn load(file: &std::path::Path) -> Result<HiveFileReader, String> {
        RegKey::load_app_key_with_flags(file, KEY_READ, 0)
            .map(|root| HiveFileReader { root })
            .map_err(|e| format!("load {}: {e}", file.display()))
    }

    fn open(&self, key: &RegKeyPath) -> Result<RegKey, RegError> {
        self.root.open_subkey_with_flags(key.path.trim_matches('\\'), KEY_READ).map_err(map_err)
    }
}

impl RegistryReader for HiveFileReader {
    fn subkeys(&self, key: &RegKeyPath) -> Result<Vec<String>, RegError> {
        self.open(key)?.enum_keys().map(|r| r.map_err(map_err)).collect()
    }

    fn values(&self, key: &RegKeyPath) -> Result<Vec<(String, RegValue)>, RegError> {
        self.open(key)?
            .enum_values()
            .map(|r| r.map(|(n, v)| (n, convert(&v))).map_err(map_err))
            .collect()
    }

    fn key_exists(&self, key: &RegKeyPath) -> bool {
        self.open(key).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::key;

    #[test]
    #[ignore]
    fn win_registry_reads_uninstall_root() {
        let subs = WinRegistry
            .subkeys(&key(Hive::LocalMachine, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"))
            .unwrap();
        assert!(!subs.is_empty());
    }
}
