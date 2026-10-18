use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::model::Hive;
use crate::pathutil::{drive_letter, expand_env, norm, resolve_unquoted_path};
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

/// `ImagePath` -> exe path, judged against the real disk ([`normalize_image_path_with`]).
pub fn normalize_image_path(raw: &str) -> Option<PathBuf> {
    normalize_image_path_with(raw, &|p: &Path| p.is_file())
}

/// `ImagePath` -> exe path. Handles `%VAR%`, NT `\??\` paths, `\SystemRoot\…`, paths relative
/// to the Windows folder (`System32\drivers\x.sys`) and unquoted paths with spaces, which are
/// resolved by the CreateProcess rule against `is_file` ([`resolve_unquoted_path`]).
pub fn normalize_image_path_with(raw: &str, is_file: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    let system_root = crate::paths::windows_dir().to_string_lossy().into_owned();
    let expanded = expand_env(raw.trim());
    let mut cmd = expanded.as_str();
    if let Some(rest) = strip_prefix_ci(cmd, r"\??\") {
        cmd = rest;
    }
    let cmd = match strip_prefix_ci(cmd, r"\SystemRoot\") {
        Some(rest) => format!(r"{system_root}\{rest}"),
        None => cmd.to_string(),
    };
    let absolute = |p: &Path| {
        if drive_letter(p).is_none() && !p.to_string_lossy().starts_with('\\') {
            Path::new(&system_root).join(p)
        } else {
            p.to_path_buf()
        }
    };
    let exe = resolve_unquoted_path(&cmd, &|p: &Path| is_file(&absolute(p)))?;
    Some(absolute(&exe))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::FakeRegistry;

    fn system_root() -> String {
        crate::paths::windows_dir().to_string_lossy().to_uppercase()
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
    fn unquoted_image_path_with_spaces_resolves_to_existing_file() {
        let exists = |p: &Path| p == Path::new(r"C:\ProgramData\Vendor\Some Dir\x.sys");
        assert_eq!(
            normalize_image_path_with(r"\??\C:\ProgramData\Vendor\Some Dir\x.sys", &exists),
            Some(PathBuf::from(r"C:\ProgramData\Vendor\Some Dir\x.sys"))
        );
        // Missing, it is still the whole path (it ends in an extension), never the `Some` prefix.
        assert_eq!(
            normalize_image_path_with(r"\??\C:\ProgramData\Vendor\Other Dir\y.sys", &exists),
            Some(PathBuf::from(r"C:\ProgramData\Vendor\Other Dir\y.sys"))
        );
        // Relative paths are checked below the Windows folder.
        let sr = crate::paths::windows_dir();
        let rel = PathBuf::from(&sr).join(r"System32\drivers\My Drv\d.sys");
        let exists_rel = |p: &Path| p == rel.as_path();
        assert_eq!(normalize_image_path_with(r"System32\drivers\My Drv\d.sys --x", &exists_rel), Some(rel.clone()));
    }

    #[test]
    fn directory_prefix_is_not_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("Vendor")).unwrap();
        let raw = format!(r"{}\Vendor Suite\gone.sys", tmp.path().display());
        assert_eq!(normalize_image_path(&raw), Some(PathBuf::from(&raw)));
    }

    #[test]
    fn service_exes_empty_when_root_missing() {
        assert!(service_exes(&FakeRegistry::new()).is_empty());
    }
}
