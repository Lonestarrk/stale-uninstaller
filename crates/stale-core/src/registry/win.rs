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
