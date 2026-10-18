//! Registry write access: [`WinRegistryWriter`] for production, [`FakeRegistryWriter`] for tests.

use std::ffi::OsStr;
use std::path::Path;
use std::sync::Mutex;

use crate::model::{Hive, RegKeyPath};

pub trait RegistryWriter: Send + Sync {
    /// Exports `key` and its subkeys to a `.reg` file (overwriting it).
    fn export_key(&self, key: &RegKeyPath, to_file: &Path) -> Result<(), String>;
    /// Deletes `key` and everything below it.
    fn delete_key(&self, key: &RegKeyPath) -> Result<(), String>;
    fn delete_value(&self, key: &RegKeyPath, name: &str) -> Result<(), String>;
    /// Imports a `.reg` file written by [`RegistryWriter::export_key`].
    fn import_file(&self, file: &Path) -> Result<(), String>;
}

fn trimmed(key: &RegKeyPath) -> &str {
    key.path.trim_matches('\\')
}

/// `HKEY_LOCAL_MACHINE\<path>`, the form `reg.exe` expects.
fn reg_exe_key(key: &RegKeyPath) -> String {
    let hive = match key.hive {
        Hive::LocalMachine => "HKEY_LOCAL_MACHINE",
        Hive::CurrentUser => "HKEY_CURRENT_USER",
        Hive::Users => "HKEY_USERS",
    };
    match trimmed(key) {
        "" => hive.to_string(),
        p => format!(r"{hive}\{p}"),
    }
}

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Runs a console tool without a window; spawn failure or a non-zero exit becomes `Err(<its output>)`.
pub(crate) fn run_hidden(program: &str, args: &[&OsStr]) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
    let msg = [text(&out.stderr), text(&out.stdout)].into_iter().find(|s| !s.is_empty());
    Err(msg.unwrap_or_else(|| format!("{program} exited with {}", out.status)))
}

fn predef(h: Hive) -> winreg::RegKey {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, HKEY_USERS};
    winreg::RegKey::predef(match h {
        Hive::LocalMachine => HKEY_LOCAL_MACHINE,
        Hive::CurrentUser => HKEY_CURRENT_USER,
        Hive::Users => HKEY_USERS,
    })
}

/// Writes to the real registry (64-bit view); `.reg` files go through `reg.exe`.
pub struct WinRegistryWriter;

impl RegistryWriter for WinRegistryWriter {
    fn export_key(&self, key: &RegKeyPath, to_file: &Path) -> Result<(), String> {
        let k = reg_exe_key(key);
        run_hidden("reg.exe", &["export".as_ref(), k.as_ref(), to_file.as_os_str(), "/y".as_ref()])
    }

    fn delete_key(&self, key: &RegKeyPath) -> Result<(), String> {
        use winreg::enums::{KEY_ALL_ACCESS, KEY_WOW64_64KEY};
        let (parent, leaf) = trimmed(key).rsplit_once('\\').unwrap_or(("", trimmed(key)));
        if leaf.is_empty() {
            return Err("refusing to delete a hive root".to_string());
        }
        predef(key.hive)
            .open_subkey_with_flags(parent, KEY_ALL_ACCESS | KEY_WOW64_64KEY)
            .and_then(|p| p.delete_subkey_all(leaf))
            .map_err(|e| e.to_string())
    }

    fn delete_value(&self, key: &RegKeyPath, name: &str) -> Result<(), String> {
        use winreg::enums::{KEY_SET_VALUE, KEY_WOW64_64KEY};
        predef(key.hive)
            .open_subkey_with_flags(trimmed(key), KEY_SET_VALUE | KEY_WOW64_64KEY)
            .and_then(|k| k.delete_value(name))
            .map_err(|e| e.to_string())
    }

    fn import_file(&self, file: &Path) -> Result<(), String> {
        run_hidden("reg.exe", &["import".as_ref(), file.as_os_str()])
    }
}

/// Records every call in `log`; a call whose log line starts with an entry of `fail_on` fails.
#[derive(Default)]
pub struct FakeRegistryWriter {
    /// `"export HKLM\x -> file"`, `"delete HKLM\x"`, `"delete-value HKLM\x\Name"`, `"import file"`.
    pub log: Mutex<Vec<String>>,
    pub fail_on: Mutex<Vec<String>>,
}

fn short_key(key: &RegKeyPath) -> String {
    let hive = match key.hive {
        Hive::LocalMachine => "HKLM",
        Hive::CurrentUser => "HKCU",
        Hive::Users => "HKU",
    };
    format!(r"{hive}\{}", trimmed(key))
}

impl FakeRegistryWriter {
    fn record(&self, op: String) -> Result<(), String> {
        let fail = self.fail_on.lock().unwrap().iter().any(|f| op.starts_with(f.as_str()));
        self.log.lock().unwrap().push(op);
        if fail { Err("forced failure".to_string()) } else { Ok(()) }
    }
}

impl RegistryWriter for FakeRegistryWriter {
    fn export_key(&self, key: &RegKeyPath, to_file: &Path) -> Result<(), String> {
        self.record(format!("export {} -> {}", short_key(key), to_file.display()))
    }

    fn delete_key(&self, key: &RegKeyPath) -> Result<(), String> {
        self.record(format!("delete {}", short_key(key)))
    }

    fn delete_value(&self, key: &RegKeyPath, name: &str) -> Result<(), String> {
        self.record(format!(r"delete-value {}\{name}", short_key(key)))
    }

    fn import_file(&self, file: &Path) -> Result<(), String> {
        self.record(format!("import {}", file.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::key;

    #[test]
    fn fake_registry_writer_logs_operations_and_can_fail() {
        let w = FakeRegistryWriter::default();
        w.fail_on.lock().unwrap().push(r"delete HKLM\SOFTWARE\Locked".to_string());
        let vendor = key(Hive::LocalMachine, r"SOFTWARE\Vendor");
        w.export_key(&vendor, Path::new(r"C:\q\1.reg")).unwrap();
        w.delete_key(&vendor).unwrap();
        w.delete_value(&key(Hive::CurrentUser, r"Software\Run"), "Vendor").unwrap();
        w.import_file(Path::new(r"C:\q\1.reg")).unwrap();
        assert_eq!(w.delete_key(&key(Hive::LocalMachine, r"SOFTWARE\Locked")), Err("forced failure".to_string()));
        w.delete_value(&key(Hive::LocalMachine, r"SOFTWARE\Locked"), "Name").unwrap();
        assert_eq!(
            *w.log.lock().unwrap(),
            vec![
                r"export HKLM\SOFTWARE\Vendor -> C:\q\1.reg",
                r"delete HKLM\SOFTWARE\Vendor",
                r"delete-value HKCU\Software\Run\Vendor",
                r"import C:\q\1.reg",
                r"delete HKLM\SOFTWARE\Locked",
                r"delete-value HKLM\SOFTWARE\Locked\Name",
            ]
        );
    }

    #[test]
    fn reg_exe_key_uses_full_hive_names() {
        assert_eq!(reg_exe_key(&key(Hive::LocalMachine, r"SOFTWARE\Vendor\")), r"HKEY_LOCAL_MACHINE\SOFTWARE\Vendor");
        assert_eq!(reg_exe_key(&key(Hive::CurrentUser, r"Software\X")), r"HKEY_CURRENT_USER\Software\X");
        assert_eq!(reg_exe_key(&key(Hive::Users, r"S-1-5-21-1\Software")), r"HKEY_USERS\S-1-5-21-1\Software");
    }

    const TEST_KEY: &str = r"Software\StaleUninstallerTest";

    /// Deletes `HKCU\Software\StaleUninstallerTest` when dropped, so a failing test leaves nothing behind.
    struct TestKeyGuard;

    impl Drop for TestKeyGuard {
        fn drop(&mut self) {
            let _ = predef(Hive::CurrentUser).delete_subkey_all(TEST_KEY);
        }
    }

    #[test]
    #[ignore]
    fn reg_export_import_roundtrip_in_hkcu_stale_test_key() {
        let _guard = TestKeyGuard;
        let hkcu = predef(Hive::CurrentUser);
        let (k, _) = hkcu.create_subkey(TEST_KEY).unwrap();
        k.set_value("Name", &"Stale \"test\" \u{e9}").unwrap();
        k.set_value("Count", &42u32).unwrap();
        let (sub, _) = k.create_subkey("Sub").unwrap();
        sub.set_value("Inner", &"x").unwrap();
        drop((k, sub));

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test key.reg");
        let w = WinRegistryWriter;
        let test_key = key(Hive::CurrentUser, TEST_KEY);
        w.export_key(&test_key, &file).unwrap();
        assert!(file.metadata().unwrap().len() > 0);

        w.delete_value(&test_key, "Count").unwrap();
        assert!(hkcu.open_subkey(TEST_KEY).unwrap().get_value::<u32, _>("Count").is_err());
        w.delete_key(&test_key).unwrap();
        assert!(hkcu.open_subkey(TEST_KEY).is_err());
        assert!(w.delete_key(&test_key).is_err(), "deleting a missing key is an error");

        w.import_file(&file).unwrap();
        let k = hkcu.open_subkey(TEST_KEY).unwrap();
        assert_eq!(k.get_value::<String, _>("Name").unwrap(), "Stale \"test\" \u{e9}");
        assert_eq!(k.get_value::<u32, _>("Count").unwrap(), 42);
        assert_eq!(k.open_subkey("Sub").unwrap().get_value::<String, _>("Inner").unwrap(), "x");

        assert!(w.import_file(&dir.path().join("missing.reg")).is_err());
        w.delete_key(&test_key).unwrap();
        assert!(hkcu.open_subkey(TEST_KEY).is_err());
    }
}
