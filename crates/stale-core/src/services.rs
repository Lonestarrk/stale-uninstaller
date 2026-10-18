use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::model::Hive;
use crate::pathutil::{drive_letter, expand_env, first_path_in_command, norm};
use crate::registry::{key, RegValue, RegistryReader};

pub const SERVICES_ROOT: &str = r"SYSTEM\CurrentControlSet\Services";

/// `norm()`ed executable path of every service and driver (from `ImagePath`).
pub fn service_exes(reg: &dyn RegistryReader) -> HashSet<String> {
    let Ok(names) = reg.subkeys(&key(Hive::LocalMachine, SERVICES_ROOT)) else {
        return HashSet::new();
    };
    names
        .iter()
        .filter_map(|name| {
            let values = reg.values(&key(Hive::LocalMachine, &format!(r"{SERVICES_ROOT}\{name}"))).ok()?;
            let image_path = values.into_iter().find_map(|(n, v)| match v {
                RegValue::Str(s) | RegValue::ExpandStr(s) if n.eq_ignore_ascii_case("ImagePath") => Some(s),
                _ => None,
            })?;
            normalize_image_path(&image_path).map(|p| norm(&p))
        })
        .collect()
}

/// Strips a case-insensitive ASCII `prefix`.
fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    s.get(..prefix.len()).filter(|head| head.eq_ignore_ascii_case(prefix)).map(|_| &s[prefix.len()..])
}

/// `ImagePath` -> exe path. Handles `%VAR%`, NT `\??\` paths, `\SystemRoot\…`
/// and paths relative to the Windows folder (`System32\drivers\x.sys`).
pub fn normalize_image_path(raw: &str) -> Option<PathBuf> {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    let expanded = expand_env(raw.trim());
    let mut cmd = expanded.as_str();
    if let Some(rest) = strip_prefix_ci(cmd, r"\??\") {
        cmd = rest;
    }
    let cmd = match strip_prefix_ci(cmd, r"\SystemRoot\") {
        Some(rest) => format!(r"{system_root}\{rest}"),
        None => cmd.to_string(),
    };
    let exe = first_path_in_command(&cmd)?;
    Some(if drive_letter(&exe).is_none() && !exe.to_string_lossy().starts_with('\\') {
        Path::new(&system_root).join(exe)
    } else {
        exe
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::FakeRegistry;

    fn system_root() -> String {
        std::env::var("SystemRoot").unwrap().to_uppercase()
    }

    #[test]
    fn service_exes_normalizes_image_paths() {
        let mut reg = FakeRegistry::new();
        let svc = |name: &str| key(Hive::LocalMachine, &format!(r"{SERVICES_ROOT}\{name}"));
        reg.set(svc("Quoted"), "ImagePath", RegValue::Str(r#""C:\Program Files\Foo\FooSvc.exe" -k run"#.into()));
        reg.set(svc("NtPath"), "ImagePath", RegValue::ExpandStr(r"\??\C:\Tools\drv.sys".into()));
        reg.set(svc("SysRoot"), "ImagePath", RegValue::ExpandStr(r"\SystemRoot\system32\svc.exe".into()));
        reg.set(svc("Relative"), "ImagePath", RegValue::ExpandStr(r"System32\drivers\rel.sys".into()));
        reg.set(svc("EnvVar"), "ImagePath", RegValue::ExpandStr(r"%SystemRoot%\System32\svchost.exe -k netsvcs".into()));
        reg.set(svc("WrongType"), "ImagePath", RegValue::Dword(1));
        reg.add_key(svc("NoImagePath"));
        let got = service_exes(&reg);
        let sr = system_root();
        let want: HashSet<String> = [
            r"C:\PROGRAM FILES\FOO\FOOSVC.EXE".to_string(),
            r"C:\TOOLS\DRV.SYS".to_string(),
            format!(r"{sr}\SYSTEM32\SVC.EXE"),
            format!(r"{sr}\SYSTEM32\DRIVERS\REL.SYS"),
            format!(r"{sr}\SYSTEM32\SVCHOST.EXE"),
        ]
        .into();
        assert_eq!(got, want);
    }

    #[test]
    fn service_exes_empty_when_root_missing() {
        assert!(service_exes(&FakeRegistry::new()).is_empty());
    }
}
