use std::path::PathBuf;

use chrono::NaiveDate;

use crate::model::{Hive, Program, ProgramId, RegKeyPath, Scope};
use crate::pathutil::{drive_letter, expand_env, first_path_in_command};
use crate::registry::{key, RegValue, RegistryReader};

pub const UNINSTALL_ROOTS: [(Hive, &str, Scope); 3] = [
    (Hive::LocalMachine, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall", Scope::Machine),
    (
        Hive::LocalMachine,
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        Scope::Machine,
    ),
    (Hive::CurrentUser, r"Software\Microsoft\Windows\CurrentVersion\Uninstall", Scope::CurrentUser),
];

/// Per-user Uninstall key, relative to a user hive (`HKCU` or `HKU\<SID>`).
const USER_UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";

/// Never fails; unreadable roots or keys are skipped. Besides the machine and current-user
/// roots, reads every other loaded user hive under `HKU` (`Scope::OtherUser`).
pub fn enumerate_programs(reg: &dyn RegistryReader) -> Vec<Program> {
    let mut programs = Vec::new();
    for (hive, root, scope) in UNINSTALL_ROOTS {
        read_root(reg, hive, root, scope, &mut programs);
    }
    for sid in other_user_sids(reg) {
        read_root(reg, Hive::Users, &format!(r"{sid}\{USER_UNINSTALL}"), Scope::OtherUser, &mut programs);
    }
    programs
}

fn read_root(reg: &dyn RegistryReader, hive: Hive, root: &str, scope: Scope, out: &mut Vec<Program>) {
    let Ok(subkeys) = reg.subkeys(&key(hive, root)) else { return };
    for sub in subkeys {
        let path = key(hive, &format!(r"{root}\{sub}"));
        let Ok(values) = reg.values(&path) else { continue };
        out.extend(program_from_values(path, scope, &values));
    }
}

/// Lowercased, sorted subkey names of a user's Uninstall key (empty when missing).
fn uninstall_key_names(reg: &dyn RegistryReader, hive: Hive, path: &str) -> Vec<String> {
    let mut names: Vec<String> = reg.subkeys(&key(hive, path)).unwrap_or_default().iter().map(|n| n.to_lowercase()).collect();
    names.sort();
    names
}

/// SIDs under `HKU` other than `.DEFAULT`, `*_Classes` and the current user. The current user
/// is recognized by an Uninstall key with exactly the same subkey names as `HKCU`'s.
fn other_user_sids(reg: &dyn RegistryReader) -> Vec<String> {
    let current = uninstall_key_names(reg, Hive::CurrentUser, USER_UNINSTALL);
    reg.subkeys(&key(Hive::Users, ""))
        .unwrap_or_default()
        .into_iter()
        .filter(|sid| !sid.eq_ignore_ascii_case(".DEFAULT") && !sid.to_lowercase().ends_with("_classes"))
        .filter(|sid| uninstall_key_names(reg, Hive::Users, &format!(r"{sid}\{USER_UNINSTALL}")) != current)
        .collect()
}

fn find<'a>(values: &'a [(String, RegValue)], name: &str) -> Option<&'a RegValue> {
    values.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v)
}

/// A non-empty string value (`Str` or env-expanded `ExpandStr`); other types count as absent.
fn string(values: &[(String, RegValue)], name: &str) -> Option<String> {
    let s = match find(values, name)? {
        RegValue::Str(s) => s.clone(),
        RegValue::ExpandStr(s) => expand_env(s),
        _ => return None,
    };
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

fn dword(values: &[(String, RegValue)], name: &str) -> Option<u32> {
    match find(values, name)? {
        RegValue::Dword(d) => Some(*d),
        _ => None,
    }
}

fn clean_dir(s: &str) -> Option<PathBuf> {
    let s = expand_env(s.trim().trim_matches('"').trim());
    let s = s.trim_end_matches(['\\', '/']);
    (!s.is_empty()).then(|| PathBuf::from(s))
}

/// `None` only when there is no usable `DisplayName` and no `UninstallString`.
pub fn program_from_values(
    key: RegKeyPath,
    scope: Scope,
    values: &[(String, RegValue)],
) -> Option<Program> {
    let display_name = string(values, "DisplayName");
    let uninstall_string = string(values, "UninstallString");
    if display_name.is_none() && uninstall_string.is_none() {
        return None;
    }
    let leaf = key.path.rsplit('\\').next().unwrap_or("").to_string();
    let hidden = display_name.is_none()
        || dword(values, "SystemComponent") == Some(1)
        || find(values, "ParentKeyName").is_some();

    let install_location = string(values, "InstallLocation").and_then(|s| clean_dir(&s));
    let display_icon = string(values, "DisplayIcon").and_then(|s| {
        // Strip a trailing ",<index>" resource suffix.
        let path = match s.rsplit_once(',') {
            Some((p, idx)) if idx.trim().trim_start_matches('-').parse::<u32>().is_ok() => p,
            _ => &s,
        };
        clean_dir(path)
    });
    let drive = install_location
        .as_deref()
        .and_then(drive_letter)
        .or_else(|| display_icon.as_deref().and_then(drive_letter))
        .or_else(|| {
            uninstall_string
                .as_deref()
                .and_then(first_path_in_command)
                .and_then(|p| drive_letter(&p))
        });

    let install_date = string(values, "InstallDate")
        .and_then(|s| (s.len() == 8).then(|| NaiveDate::parse_from_str(&s, "%Y%m%d").ok()).flatten());
    let estimated_size_bytes = match find(values, "EstimatedSize") {
        Some(RegValue::Dword(kb)) => Some(u64::from(*kb) * 1024),
        _ => None,
    };

    Some(Program {
        id: ProgramId(format!(r"{:?}\{}", key.hive, key.path)),
        name: display_name.unwrap_or(leaf),
        publisher: string(values, "Publisher"),
        version: string(values, "DisplayVersion"),
        install_location,
        install_date,
        quiet_uninstall_string: string(values, "QuietUninstallString"),
        uninstall_string,
        display_icon,
        is_msi: dword(values, "WindowsInstaller") == Some(1),
        estimated_size_bytes,
        drive,
        hidden,
        scope,
        key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::FakeRegistry;
    use RegValue::*;

    const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

    fn vals(pairs: &[(&str, RegValue)]) -> Vec<(String, RegValue)> {
        pairs.iter().map(|(n, v)| (n.to_string(), v.clone())).collect()
    }

    fn k(leaf: &str) -> RegKeyPath {
        key(Hive::LocalMachine, &format!(r"{UNINSTALL}\{leaf}"))
    }

    #[test]
    fn parses_full_entry() {
        let p = program_from_values(
            k("Unity Hub"),
            Scope::Machine,
            &vals(&[
                ("DisplayName", Str("Unity Hub".into())),
                ("Publisher", Str("Unity Technologies".into())),
                ("DisplayVersion", Str("3.8.0".into())),
                ("InstallLocation", Str(r"D:\Program Files\Unity Hub\".into())),
                ("InstallDate", Str("20240120".into())),
                ("EstimatedSize", Dword(421888)),
                (
                    "UninstallString",
                    Str(r#""D:\Program Files\Unity Hub\Uninstall Unity Hub.exe""#.into()),
                ),
            ]),
        )
        .unwrap();
        assert_eq!(p.name, "Unity Hub");
        assert_eq!(p.publisher.as_deref(), Some("Unity Technologies"));
        assert_eq!(p.version.as_deref(), Some("3.8.0"));
        assert_eq!(p.install_location, Some(PathBuf::from(r"D:\Program Files\Unity Hub")));
        assert_eq!(p.drive, Some('D'));
        assert_eq!(p.estimated_size_bytes, Some(421888 * 1024));
        assert_eq!(p.install_date, Some(NaiveDate::from_ymd_opt(2024, 1, 20).unwrap()));
        assert!(!p.hidden);
        assert!(!p.is_msi);
        assert_eq!(p.id, ProgramId(format!(r"LocalMachine\{UNINSTALL}\Unity Hub")));
    }

    #[test]
    fn hidden_when_system_component_or_parent_key_or_no_name() {
        let sys = program_from_values(
            k("A"),
            Scope::Machine,
            &vals(&[("DisplayName", Str("A".into())), ("SystemComponent", Dword(1))]),
        )
        .unwrap();
        assert!(sys.hidden);
        let parent = program_from_values(
            k("B"),
            Scope::Machine,
            &vals(&[("DisplayName", Str("B".into())), ("ParentKeyName", Str("Office".into()))]),
        )
        .unwrap();
        assert!(parent.hidden);
        let noname = program_from_values(
            k("{GUID-1}"),
            Scope::Machine,
            &vals(&[("UninstallString", Str("x.exe".into()))]),
        )
        .unwrap();
        assert!(noname.hidden);
        assert_eq!(noname.name, "{GUID-1}");
    }

    #[test]
    fn none_without_name_and_uninstall_string() {
        assert!(program_from_values(k("Z"), Scope::Machine, &vals(&[("Publisher", Str("P".into()))]))
            .is_none());
    }

    #[test]
    fn msi_flag_and_drive_from_display_icon_then_uninstall_string() {
        let msi = program_from_values(
            k("M"),
            Scope::Machine,
            &vals(&[("DisplayName", Str("M".into())), ("WindowsInstaller", Dword(1))]),
        )
        .unwrap();
        assert!(msi.is_msi);

        let icon = program_from_values(
            k("I"),
            Scope::Machine,
            &vals(&[
                ("DisplayName", Str("I".into())),
                ("DisplayIcon", Str(r"E:\Apps\x\x.exe,0".into())),
                ("UninstallString", Str(r"F:\u.exe".into())),
            ]),
        )
        .unwrap();
        assert_eq!(icon.drive, Some('E'));
        assert_eq!(icon.display_icon, Some(PathBuf::from(r"E:\Apps\x\x.exe")));

        let pf = std::env::var("ProgramFiles").unwrap();
        let expected = pf.chars().next().unwrap().to_ascii_uppercase();
        let un = program_from_values(
            k("U"),
            Scope::Machine,
            &vals(&[
                ("DisplayName", Str("U".into())),
                ("UninstallString", Str(r#""%ProgramFiles%\Foo\unins000.exe" /SILENT"#.into())),
            ]),
        )
        .unwrap();
        assert_eq!(un.drive, Some(expected));
    }

    #[test]
    fn wrong_value_types_are_ignored_not_fatal() {
        let p = program_from_values(
            k("W"),
            Scope::Machine,
            &vals(&[
                ("DisplayName", Binary(vec![1, 2])),
                ("UninstallString", Str("x.exe".into())),
                ("EstimatedSize", Str("123".into())),
                ("InstallLocation", ExpandStr(r"%ProgramFiles%\Bar".into())),
                ("Publisher", Dword(5)),
            ]),
        )
        .unwrap();
        assert!(p.hidden);
        assert_eq!(p.name, "W");
        assert_eq!(p.estimated_size_bytes, None);
        assert_eq!(p.publisher, None);
        let pf = std::env::var("ProgramFiles").unwrap();
        assert_eq!(p.install_location, Some(PathBuf::from(format!(r"{pf}\Bar"))));
    }

    #[test]
    fn install_date_bad_format_is_none() {
        for bad in ["2024-01-20", "", "20241399"] {
            let p = program_from_values(
                k("D"),
                Scope::Machine,
                &vals(&[("DisplayName", Str("D".into())), ("InstallDate", Str(bad.into()))]),
            )
            .unwrap();
            assert_eq!(p.install_date, None, "{bad:?}");
        }
    }

    #[test]
    fn enumerate_reads_all_three_roots_and_skips_missing_root() {
        let mut reg = FakeRegistry::new();
        let (h0, r0, _) = UNINSTALL_ROOTS[0];
        let (h2, r2, _) = UNINSTALL_ROOTS[2];
        reg.set(key(h0, &format!(r"{r0}\One")), "DisplayName", Str("One".into()));
        reg.set(key(h2, &format!(r"{r2}\One")), "DisplayName", Str("One".into()));
        // junk entry with neither name nor uninstall string is dropped
        reg.set(key(h0, &format!(r"{r0}\Junk")), "Foo", Str("x".into()));
        let progs = enumerate_programs(&reg);
        assert_eq!(progs.len(), 2);
        assert_eq!(progs[0].scope, Scope::Machine);
        assert_eq!(progs[1].scope, Scope::CurrentUser);
        assert_ne!(progs[0].id, progs[1].id);
    }

    #[test]
    fn enumerate_reads_other_users_but_not_the_current_one_twice() {
        let mut reg = FakeRegistry::new();
        let name = |n: &str| Str(n.into());
        reg.set(key(Hive::CurrentUser, &format!(r"{USER_UNINSTALL}\A")), "DisplayName", name("A"));
        // The current user's own hive, also loaded under HKU: same key names as HKCU.
        reg.set(key(Hive::Users, &format!(r"S-1-5-21-1\{USER_UNINSTALL}\A")), "DisplayName", name("A"));
        reg.set(key(Hive::Users, &format!(r"S-1-5-21-2\{USER_UNINSTALL}\B")), "DisplayName", name("B"));
        reg.set(key(Hive::Users, &format!(r".DEFAULT\{USER_UNINSTALL}\D")), "DisplayName", name("D"));
        reg.set(key(Hive::Users, &format!(r"S-1-5-21-2_Classes\{USER_UNINSTALL}\C")), "DisplayName", name("C"));
        let progs = enumerate_programs(&reg);
        let found: Vec<(&str, Scope)> = progs.iter().map(|p| (p.name.as_str(), p.scope)).collect();
        assert_eq!(found, [("A", Scope::CurrentUser), ("B", Scope::OtherUser)]);
        assert_eq!(progs[1].key.hive, Hive::Users);
        assert_eq!(progs[1].id.0, format!(r"Users\S-1-5-21-2\{USER_UNINSTALL}\B"));
    }
}
