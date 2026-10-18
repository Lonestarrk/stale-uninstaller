//! Registry keys and values a program left behind.

use super::names::{
    child_matches_in_container, name_matches, name_matches_exact, name_matches_first_word_only, publisher_matches,
};
use super::names::normalize_name;
use super::{Confidence, Leftover, LeftoverKind, ProgramHint, NOTE_PROGID_OF_PROGRAM};
use crate::fsys::FsReader;
use crate::model::{Hive, RegKeyPath};
use crate::pathutil::{drive_letter, exe_under, expand_env, expand_env_wow64, first_path_in_command, wow64_vars};
use crate::programs::other_user_sids;
use crate::registry::{key, RegValue, RegistryReader};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Keys to look under. Roots that do not exist are skipped at scan time.
pub struct RegScanRoots {
    /// Vendor/program keys: `HKLM\SOFTWARE`, its WOW6432Node, `HKCU\Software`, `HKU\<SID>\Software`.
    pub software: Vec<RegKeyPath>,
    /// File types, ProgIDs, protocols and `CLSID`.
    pub classes: Vec<RegKeyPath>,
    /// `Run`/`RunOnce`; their values are command lines.
    pub run: Vec<RegKeyPath>,
    /// `App Paths`; each subkey's default value is an exe path.
    pub app_paths: Vec<RegKeyPath>,
}

impl RegScanRoots {
    pub fn standard(reg: &dyn RegistryReader) -> RegScanRoots {
        let hklm = |p: &str| key(Hive::LocalMachine, p);
        let hkcu = |p: &str| key(Hive::CurrentUser, p);
        let current_version = |leaf: &str| {
            let cv = format!(r"Microsoft\Windows\CurrentVersion\{leaf}");
            vec![
                hklm(&format!(r"SOFTWARE\{cv}")),
                hklm(&format!(r"SOFTWARE\WOW6432Node\{cv}")),
                hkcu(&format!(r"Software\{cv}")),
            ]
        };
        let mut software = vec![hklm("SOFTWARE"), hklm(r"SOFTWARE\WOW6432Node"), hkcu("Software")];
        software.extend(other_user_sids(reg).into_iter().map(|sid| key(Hive::Users, &format!(r"{sid}\Software"))));
        RegScanRoots {
            software,
            classes: vec![hklm(r"SOFTWARE\Classes"), hkcu(r"Software\Classes")],
            run: [current_version("Run"), current_version("RunOnce")].concat(),
            app_paths: current_version("App Paths"),
        }
    }
}

/// What a class/Run/App Paths entry must point at to belong to the program.
enum Target {
    /// Any file inside the (non-shared) install folder.
    Install(PathBuf),
    /// Lowercased, non-generic exe file names from the display icon and uninstall command; a
    /// referenced exe must also sit in a folder (parent or grandparent) named like `program`.
    ExeNames { names: Vec<String>, program: String },
}

/// Stem fragments of uninstallers, installers and updaters (`unins000`, Squirrel's `Update`),
/// which many unrelated programs share.
const GENERIC_EXE_STEMS: [&str; 6] = ["unins", "uninstall", "setup", "install", "update", "msiexec"];

impl Target {
    /// `None` when there is nothing to match on.
    fn from_hint(hint: &ProgramHint) -> Option<Target> {
        if let Some(dir) = hint.effective_install_dir() {
            return Some(Target::Install(dir));
        }
        let uninstall_exe = hint.uninstall_string.as_deref().and_then(first_path_in_command);
        let mut names: Vec<String> = [hint.display_icon.as_deref(), uninstall_exe.as_deref()]
            .into_iter()
            .flatten()
            .filter_map(lower_file_name)
            .filter(|n| n.strip_suffix(".exe").is_some_and(|stem| !GENERIC_EXE_STEMS.iter().any(|g| stem.contains(g))))
            .collect();
        names.dedup();
        (!names.is_empty()).then(|| Target::ExeNames { names, program: hint.name.clone() })
    }

    fn refers_to(&self, path: &Path) -> bool {
        match self {
            Target::Install(dir) => exe_under(path, dir),
            Target::ExeNames { names, program } => {
                lower_file_name(path).is_some_and(|n| names.contains(&n))
                    && path
                        .ancestors()
                        .skip(1)
                        .take(2)
                        .filter_map(|dir| dir.file_name())
                        .any(|dir| name_matches(&dir.to_string_lossy(), program))
            }
        }
    }

    /// True when a string value names a path this target claims.
    fn claims(&self, value: Option<String>) -> bool {
        value.as_deref().and_then(path_in_value).is_some_and(|p| self.refers_to(&p))
    }
}

fn lower_file_name(p: &Path) -> Option<String> {
    p.file_name().map(|n| n.to_string_lossy().to_lowercase())
}

pub(crate) fn child(parent: &RegKeyPath, name: &str) -> RegKeyPath {
    key(parent.hive, &format!(r"{}\{name}", parent.path))
}

/// Per-user folder variables: expanded in this process they name *our* profile, not the one of
/// the account whose hive the value was read from.
const USER_VARIABLES: [&str; 6] = ["%LOCALAPPDATA%", "%APPDATA%", "%USERPROFILE%", "%TEMP%", "%TMP%", "%HOMEPATH%"];

/// True when `k` lies under a `WOW6432Node` key (the 32-bit view).
fn in_wow64_view(k: &RegKeyPath) -> bool {
    k.path.split('\\').any(|c| c.eq_ignore_ascii_case("WOW6432Node"))
}

/// `Str` and env-expanded `ExpandStr` of a value read under the key `at`; other types are
/// ignored. Under a `WOW6432Node` key `%ProgramFiles%` and `%CommonProgramFiles%` name the x86
/// folders (in a `Str` too, which is expanded later as a command line). Under another account's
/// hive ([`Hive::Users`]) an `ExpandStr` naming a per-user folder variable is skipped.
pub(crate) fn string_value(v: &RegValue, at: &RegKeyPath) -> Option<String> {
    let wow64 = in_wow64_view(at);
    match v {
        RegValue::Str(s) => Some(if wow64 { wow64_vars(s) } else { s.clone() }),
        RegValue::ExpandStr(s) => {
            let upper = s.to_uppercase();
            if at.hive == Hive::Users && USER_VARIABLES.iter().any(|var| upper.contains(var)) {
                return None;
            }
            Some(if wow64 { expand_env_wow64(s) } else { expand_env(s) })
        }
        _ => None,
    }
}

pub(crate) fn default_string(reg: &dyn RegistryReader, k: &RegKeyPath) -> Option<String> {
    reg.values(k).ok()?.iter().find(|(n, _)| n.is_empty()).and_then(|(_, v)| string_value(v, k))
}

/// The path in a value that is either a bare path (a DLL path may hold unquoted spaces) or a
/// command line.
pub(crate) fn path_in_value(s: &str) -> Option<PathBuf> {
    let s = s.trim();
    if s.starts_with('"') || s.to_ascii_lowercase().contains(".exe") {
        first_path_in_command(s)
    } else {
        (!s.is_empty()).then(|| PathBuf::from(s))
    }
}

/// Collects leftovers, dropping repeats of the same key (and value name).
#[derive(Default)]
struct Found {
    out: Vec<Leftover>,
    seen: HashSet<(Hive, String, Option<String>)>,
}

impl Found {
    fn push(&mut self, kind: LeftoverKind, confidence: Confidence, note: Option<&str>) {
        let id = match &kind {
            LeftoverKind::RegKey { key } => (key.hive, key.path.to_lowercase(), None),
            LeftoverKind::RegValue { key, name } => (key.hive, key.path.to_lowercase(), Some(name.to_lowercase())),
            _ => unreachable!("registry scan only reports keys and values"),
        };
        if self.seen.insert(id) {
            self.out.push(Leftover { kind, confidence, note: note.map(str::to_string) });
        }
    }

    fn key(&mut self, key: RegKeyPath, confidence: Confidence, note: Option<&str>) {
        self.push(LeftoverKind::RegKey { key }, confidence, note);
    }
}

/// High-confidence items come first; a key found by several rules is listed once. `fs` tells
/// whether a file type's handler still exists.
pub fn scan_registry(reg: &dyn RegistryReader, fs: &dyn FsReader, roots: &RegScanRoots, hint: &ProgramHint) -> Vec<Leftover> {
    let mut found = Found::default();
    uninstall_key(reg, hint, &mut found);
    let target = Target::from_hint(hint);
    for root in roots.classes.iter().filter(|r| reg.key_exists(r)) {
        class_keys(reg, fs, root, target.as_ref(), hint, &mut found);
    }
    if let Some(target) = target {
        for root in roots.run.iter().filter(|r| reg.key_exists(r)) {
            run_values(reg, root, &target, &mut found);
        }
        for root in roots.app_paths.iter().filter(|r| reg.key_exists(r)) {
            app_path_keys(reg, root, &target, &mut found);
        }
    }
    for root in roots.software.iter().filter(|r| reg.key_exists(r)) {
        software_keys(reg, root, hint, &mut found);
    }
    found.out
}

/// (a) The program's own Uninstall key, if it is still there.
fn uninstall_key(reg: &dyn RegistryReader, hint: &ProgramHint, found: &mut Found) {
    if let Some(k) = hint.uninstall_key.as_ref().filter(|k| reg.key_exists(k)) {
        found.key(k.clone(), Confidence::High, None);
    }
}

/// (b) `<root>\<Name>` (exact name match) and `<container>\<Name>`, where a container is a
/// publisher key or a first-word vendor key (`Google` for `Google Chrome`). A container holding
/// nothing but the program's key (and no values) is listed in its place; otherwise it never is.
fn software_keys(reg: &dyn RegistryReader, root: &RegKeyPath, hint: &ProgramHint, found: &mut Found) {
    let Ok(subkeys) = reg.subkeys(root) else { return };
    for sub in subkeys {
        let k = child(root, &sub);
        if name_matches_exact(&sub, &hint.name) {
            found.key(k, Confidence::Medium, None);
            continue;
        }
        let is_container = name_matches_first_word_only(&sub, &hint.name)
            || hint.publisher.as_deref().is_some_and(|p| publisher_matches(&sub, p));
        if !is_container {
            continue;
        }
        let (matched, others): (Vec<String>, Vec<String>) = reg
            .subkeys(&k)
            .unwrap_or_default()
            .into_iter()
            .partition(|c| child_matches_in_container(c, &sub, &hint.name));
        if matched.is_empty() {
            continue;
        }
        if matched.len() == 1 && others.is_empty() && reg.values(&k).is_ok_and(|v| v.is_empty()) {
            found.key(k, Confidence::Medium, None);
            continue;
        }
        let note = (!others.is_empty()).then_some("shared-publisher-key");
        for m in matched {
            found.key(child(&k, &m), Confidence::Medium, note);
        }
    }
}

/// The ProgID's first token (split at `.` and whitespace) names the program's first word:
/// `Obsidian.md` or `Obsidian Canvas` for `Obsidian`.
fn progid_names_program(progid: &str, program_first_word: &str) -> bool {
    progid
        .trim()
        .split(|c: char| c == '.' || c.is_whitespace())
        .next()
        .is_some_and(|t| name_matches_exact(t, program_first_word))
}

/// (c) COM classes, file types, ProgIDs and protocols that point at the program (High), and file
/// types whose ProgID is named like the program (Medium; see [`named_file_type`]). One pass over
/// the root: per subkey only its default value and `shell\open\command`; per `CLSID\{…}` only the
/// `InprocServer32`/`LocalServer32` defaults.
fn class_keys(
    reg: &dyn RegistryReader,
    fs: &dyn FsReader,
    root: &RegKeyPath,
    target: Option<&Target>,
    hint: &ProgramHint,
    found: &mut Found,
) {
    let Ok(subkeys) = reg.subkeys(root) else { return };
    let normalized = normalize_name(&hint.name);
    let first_word = normalized.split(' ').next().unwrap_or("");
    let mut named_progids: Vec<(RegKeyPath, String)> = Vec::new();
    for sub in subkeys {
        let k = child(root, &sub);
        if sub.eq_ignore_ascii_case("CLSID") {
            let Some(target) = target else { continue };
            for clsid in reg.subkeys(&k).unwrap_or_default() {
                let c = child(&k, &clsid);
                let server = |s: &str| target.claims(default_string(reg, &child(&c, s)));
                if server("InprocServer32") || server("LocalServer32") {
                    found.key(c, Confidence::High, None);
                }
            }
            continue;
        }
        let is_file_type = sub.starts_with('.');
        if target.is_none() && !is_file_type {
            continue;
        }
        let default = default_string(reg, &k);
        if let Some(progid) = default.as_deref().filter(|p| is_file_type && progid_names_program(p, first_word)) {
            named_progids.push((k.clone(), progid.trim().to_string()));
        }
        if target.is_some_and(|t| t.claims(default) || t.claims(default_string(reg, &child(&k, r"shell\open\command")))) {
            found.key(k, Confidence::High, None);
        }
    }
    // After the High pass, so a ProgID that points into the install folder stays High.
    for (ext, progid) in named_progids {
        named_file_type(reg, fs, root, ext, &progid, target, found);
    }
}

/// A file type `ext` whose ProgID `progid` is named like the program is listed (Medium) only when
/// it is orphaned or the program's: the ProgID key is missing, or its `shell\open\command` exe is
/// missing on disk or lies inside the program's target. The ProgID key itself is listed only in
/// the latter two cases. A file type whose handler is a live exe elsewhere (a side-by-side
/// Python's `.py` → `Python.File` → `py.exe`) is never listed.
fn named_file_type(
    reg: &dyn RegistryReader,
    fs: &dyn FsReader,
    root: &RegKeyPath,
    ext: RegKeyPath,
    progid: &str,
    target: Option<&Target>,
    found: &mut Found,
) {
    if progid.contains('\\') {
        return;
    }
    let note = Some(NOTE_PROGID_OF_PROGRAM);
    let progid_key = child(root, progid);
    if !reg.key_exists(&progid_key) {
        found.key(ext, Confidence::Medium, note);
        return;
    }
    let exe = default_string(reg, &child(&progid_key, r"shell\open\command")).as_deref().and_then(path_in_value);
    // A handler without a drive letter (`notepad.exe`, `rundll32.exe`) resolves through the
    // search path: treated as live.
    let ours = exe.is_some_and(|exe| {
        (drive_letter(&exe).is_some() && !fs.exists(&exe)) || target.is_some_and(|t| t.refers_to(&exe))
    });
    if ours {
        found.key(ext, Confidence::Medium, note);
        found.key(progid_key, Confidence::Medium, note);
    }
}

/// (d) Autostart command lines that launch the program.
fn run_values(reg: &dyn RegistryReader, root: &RegKeyPath, target: &Target, found: &mut Found) {
    let Ok(values) = reg.values(root) else { return };
    for (name, value) in values {
        let exe = string_value(&value, root).as_deref().and_then(first_path_in_command);
        if exe.is_some_and(|p| target.refers_to(&p)) {
            found.push(LeftoverKind::RegValue { key: root.clone(), name }, Confidence::High, None);
        }
    }
}

/// `norm()`ed exes the `Run`/`RunOnce` values (HKLM, its WOW6432Node, HKCU) start at logon,
/// with the target a launcher names in its arguments (Squirrel's `Update.exe --processStart
/// App.exe`, see [`autostart_targets`](crate::usage::matching::autostart_targets)). Unreadable
/// keys contribute nothing.
pub fn run_value_exes(reg: &dyn RegistryReader) -> HashSet<String> {
    let roots = RegScanRoots::standard(reg).run;
    roots
        .iter()
        .filter_map(|root| reg.values(root).ok().map(|values| (root, values)))
        .flat_map(|(root, values)| values.into_iter().filter_map(move |(_, v)| string_value(&v, root)))
        .flat_map(|cmd| {
            let exe = first_path_in_command(&cmd);
            exe.map(|exe| crate::usage::matching::autostart_targets(&exe, command_arguments(&cmd))).unwrap_or_default()
        })
        .collect()
}

/// What follows the executable in a command line: after the closing quote of a quoted path,
/// else after the first `.exe`, else after the first space.
pub(crate) fn command_arguments(cmd: &str) -> &str {
    let cmd = cmd.trim();
    let end = if let Some(rest) = cmd.strip_prefix('"') {
        rest.find('"').map(|i| i + 2)
    } else {
        cmd.to_ascii_lowercase().find(".exe").map(|i| i + 4).or_else(|| cmd.find(' '))
    };
    end.and_then(|i| cmd.get(i..)).unwrap_or_default().trim()
}

/// (e) `App Paths` entries whose default value is the program's exe.
fn app_path_keys(reg: &dyn RegistryReader, root: &RegKeyPath, target: &Target, found: &mut Found) {
    let Ok(subkeys) = reg.subkeys(root) else { return };
    for sub in subkeys {
        let k = child(root, &sub);
        if target.claims(default_string(reg, &k)) {
            found.key(k, Confidence::High, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pathutil::norm;
    use crate::leftovers::{Confidence, LeftoverKind};
    use crate::model::Hive;
    use crate::fsys::FakeFs;
    use crate::registry::{key, FakeRegistry, RegValue};
    use std::path::PathBuf;

    const INSTALL: &str = r"D:\Apps\Foo Studio";
    const RUN: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
    const APP_PATHS: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths";
    // The scanner reports keys in its roots' spelling, which is `Software` under HKCU.
    const HKCU_RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const HKCU_APP_PATHS: &str = r"Software\Microsoft\Windows\CurrentVersion\App Paths";

    fn hint(name: &str, publisher: Option<&str>, loc: Option<&str>) -> ProgramHint {
        let mut h = ProgramHint::manual(name, loc.map(PathBuf::from));
        h.publisher = publisher.map(str::to_string);
        h
    }

    fn s(v: &str) -> RegValue {
        RegValue::Str(v.into())
    }

    /// `("Hive\path[:value]", confidence, note)`, sorted for stable comparison.
    fn found(items: &[Leftover]) -> Vec<(String, Confidence, Option<String>)> {
        let mut v: Vec<_> = items
            .iter()
            .map(|l| {
                let id = match &l.kind {
                    LeftoverKind::RegKey { key } => format!(r"{:?}\{}", key.hive, key.path),
                    LeftoverKind::RegValue { key, name } => format!(r"{:?}\{}:{name}", key.hive, key.path),
                    k => panic!("unexpected {k:?}"),
                };
                (id, l.confidence, l.note.clone())
            })
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    #[test]
    fn uninstall_key_is_high() {
        let mut reg = FakeRegistry::new();
        let uk = key(Hive::LocalMachine, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Foo");
        reg.set(uk.clone(), "DisplayName", s("Foo Studio"));
        let mut h = hint("Foo Studio", None, None);
        h.uninstall_key = Some(uk.clone());
        let out = scan_registry(&reg, &FakeFs::new(), &RegScanRoots::standard(&reg), &h);
        assert_eq!(out, vec![Leftover { kind: LeftoverKind::RegKey { key: uk }, confidence: Confidence::High, note: None }]);

        // A key that is already gone is not reported.
        let empty = FakeRegistry::new();
        assert!(scan_registry(&empty, &FakeFs::new(), &RegScanRoots::standard(&empty), &h).is_empty());
    }

    #[test]
    fn publisher_then_name_is_medium_and_lonely_publisher_key_is_listed_instead() {
        let mut reg = FakeRegistry::new();
        // Lonely: the publisher key holds only the program's key and no values.
        reg.set(key(Hive::CurrentUser, r"Software\Foo Corp\Foo Studio"), "Theme", s("dark"));
        // Not lonely: the publisher key has values of its own.
        reg.set(key(Hive::LocalMachine, r"SOFTWARE\Foo Corp\Foo Studio"), "Path", s("x"));
        reg.set(key(Hive::LocalMachine, r"SOFTWARE\Foo Corp"), "Installed", RegValue::Dword(1));
        // Direct name match under a software root.
        reg.add_key(key(Hive::LocalMachine, r"SOFTWARE\WOW6432Node\FooStudio"));
        // Another loaded user hive (told apart from HKCU by its Uninstall key names); a publisher
        // key with no matching child is never listed.
        reg.add_key(key(Hive::CurrentUser, r"Software\Microsoft\Windows\CurrentVersion\Uninstall\X"));
        reg.add_key(key(Hive::Users, r"S-1-5-21-9\Software\Foo Corp\Other"));
        reg.add_key(key(Hive::Users, r"S-1-5-21-9\Software\Foo Corp Inc\Foo Studio"));
        reg.add_key(key(Hive::Users, r".DEFAULT\Software\Foo Studio"));
        let roots = RegScanRoots::standard(&reg);
        let out = scan_registry(&reg, &FakeFs::new(), &roots, &hint("Foo Studio", Some("Foo Corp"), None));
        assert_eq!(
            found(&out),
            vec![
                (r"CurrentUser\Software\Foo Corp".to_string(), Confidence::Medium, None),
                (r"LocalMachine\SOFTWARE\Foo Corp\Foo Studio".to_string(), Confidence::Medium, None),
                (r"LocalMachine\SOFTWARE\WOW6432Node\FooStudio".to_string(), Confidence::Medium, None),
                (r"Users\S-1-5-21-9\Software\Foo Corp Inc".to_string(), Confidence::Medium, None),
            ]
        );
    }

    #[test]
    fn shared_publisher_child_has_note() {
        let mut reg = FakeRegistry::new();
        reg.add_key(key(Hive::CurrentUser, r"Software\Foo Corp\Foo Studio\Settings"));
        reg.add_key(key(Hive::CurrentUser, r"Software\Foo Corp\Bar Player"));
        let out = scan_registry(&reg, &FakeFs::new(), &RegScanRoots::standard(&reg), &hint("Foo Studio", Some("Foo Corp Inc."), None));
        assert_eq!(
            found(&out),
            vec![(
                r"CurrentUser\Software\Foo Corp\Foo Studio".to_string(),
                Confidence::Medium,
                Some("shared-publisher-key".to_string())
            )]
        );
    }

    #[test]
    fn clsid_inproc_under_install_is_high() {
        let mut reg = FakeRegistry::new();
        let clsid = r"SOFTWARE\Classes\CLSID";
        reg.set(key(Hive::LocalMachine, &format!(r"{clsid}\{{A}}\InprocServer32")), "", s(&format!(r"{INSTALL}\shell ext.dll")));
        reg.set(key(Hive::LocalMachine, &format!(r"{clsid}\{{B}}\LocalServer32")), "", s(&format!(r#""{INSTALL}\foo.exe" -Embedding"#)));
        reg.set(key(Hive::LocalMachine, &format!(r"{clsid}\{{C}}\InprocServer32")), "", s(r"C:\Windows\System32\other.dll"));
        // A protocol and a ProgID that point into the install folder; an unrelated file type.
        reg.set(key(Hive::CurrentUser, r"Software\Classes\foo\shell\open\command"), "", s(&format!(r#""{INSTALL}\foo.exe" "%1""#)));
        reg.set(key(Hive::LocalMachine, r"SOFTWARE\Classes\FooStudio.Project"), "", RegValue::ExpandStr(format!(r"{INSTALL}\foo.exe")));
        reg.set(key(Hive::LocalMachine, r"SOFTWARE\Classes\.txt"), "", s("txtfile"));
        let out = scan_registry(&reg, &FakeFs::new(), &RegScanRoots::standard(&reg), &hint("Foo Studio", None, Some(INSTALL)));
        assert_eq!(
            found(&out),
            vec![
                (r"CurrentUser\Software\Classes\foo".to_string(), Confidence::High, None),
                (r"LocalMachine\SOFTWARE\Classes\CLSID\{A}".to_string(), Confidence::High, None),
                (r"LocalMachine\SOFTWARE\Classes\CLSID\{B}".to_string(), Confidence::High, None),
                (r"LocalMachine\SOFTWARE\Classes\FooStudio.Project".to_string(), Confidence::High, None),
            ]
        );
    }

    #[test]
    fn run_key_target_is_autostart() {
        let mut reg = FakeRegistry::new();
        reg.set(key(Hive::CurrentUser, RUN), "FooStudio", s(&format!(r#""{INSTALL}\foo.exe" --tray"#)));
        let once = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\RunOnce";
        reg.set(key(Hive::LocalMachine, once), "Bar", s(r"C:\Bar\bar.exe /x"));
        let exes = run_value_exes(&reg);
        assert_eq!(exes, HashSet::from([norm(&Path::new(INSTALL).join("foo.exe")), norm(Path::new(r"C:\Bar\bar.exe"))]));
        assert!(run_value_exes(&FakeRegistry::new()).is_empty());
    }

    #[test]
    fn run_value_under_install_is_high() {
        let mut reg = FakeRegistry::new();
        reg.set(key(Hive::CurrentUser, RUN), "FooStudio", s(&format!(r#""{INSTALL}\foo.exe" --tray"#)));
        reg.set(key(Hive::CurrentUser, RUN), "Other", s(r#""C:\Other\other.exe""#));
        let once = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\RunOnce";
        reg.set(key(Hive::LocalMachine, once), "FooOnce", s(&format!(r"{INSTALL}\setup helper.exe /x")));
        let out = scan_registry(&reg, &FakeFs::new(), &RegScanRoots::standard(&reg), &hint("Foo Studio", None, Some(INSTALL)));
        assert_eq!(
            found(&out),
            vec![
                (format!(r"CurrentUser\{HKCU_RUN}:FooStudio"), Confidence::High, None),
                (format!(r"LocalMachine\{once}:FooOnce"), Confidence::High, None),
            ]
        );

        // With a shared-root install location the exe file name decides; msiexec never matches.
        let mut h = hint("Foo Studio", None, Some(r"C:\Program Files"));
        h.display_icon = Some(PathBuf::from(r"C:\Elsewhere\FOO.EXE"));
        h.uninstall_string = Some(r"MsiExec.exe /X{123}".into());
        reg.set(key(Hive::CurrentUser, RUN), "Msi", s(r"C:\Windows\System32\msiexec.exe /fa {123}"));
        let out = scan_registry(&reg, &FakeFs::new(), &RegScanRoots::standard(&reg), &h);
        assert_eq!(found(&out), vec![(format!(r"CurrentUser\{HKCU_RUN}:FooStudio"), Confidence::High, None)]);
    }

    #[test]
    fn hint_without_install_location_uses_the_uninstaller_folder() {
        let dir = r"C:\Users\A\AppData\Local\Programs\Obsidian";
        let mut reg = FakeRegistry::new();
        reg.set(key(Hive::CurrentUser, RUN), "Obsidian", s(&format!(r#""{dir}\Obsidian.exe" --hidden"#)));
        reg.set(key(Hive::CurrentUser, r"Software\Classes\obsidian\shell\open\command"), "", s(&format!(r#""{dir}\Obsidian.exe" "%1""#)));
        reg.set(key(Hive::CurrentUser, &format!(r"{APP_PATHS}\Obsidian.exe")), "", s(&format!(r"{dir}\Obsidian.exe")));
        let mut h = hint("Obsidian", None, None);
        h.uninstall_string = Some(format!(r#""{dir}\Uninstall Obsidian.exe" /currentuser"#));
        let out = scan_registry(&reg, &FakeFs::new(), &RegScanRoots::standard(&reg), &h);
        assert_eq!(
            found(&out),
            vec![
                (r"CurrentUser\Software\Classes\obsidian".to_string(), Confidence::High, None),
                (format!(r"CurrentUser\{HKCU_APP_PATHS}\Obsidian.exe"), Confidence::High, None),
                (format!(r"CurrentUser\{HKCU_RUN}:Obsidian"), Confidence::High, None),
            ]
        );
    }

    #[test]
    fn file_type_keys_naming_the_programs_progid_are_medium() {
        let classes = r"Software\Classes";
        let mut reg = FakeRegistry::new();
        let ck = |p: &str| key(Hive::CurrentUser, &format!(r"{classes}\{p}"));
        // After the vendor uninstaller: the ProgIDs are gone, the extensions still name them.
        reg.set(ck(".canvas"), "", s("Obsidian Canvas"));
        reg.set(ck(".base"), "", s("Obsidian Base"));
        reg.set(ck(".obs"), "", s("Obsidian.Note.1"));
        // A ProgID whose handler is gone from disk is listed as well.
        reg.set(ck(".vault"), "", s("Obsidian.Vault"));
        reg.set(ck(r"Obsidian.Vault\shell\open\command"), "", s(r#""C:\Gone\Obsidian.exe" "%1""#));
        // A ProgID without a command is not the program's for sure: neither is listed.
        reg.set(ck(".note"), "", s("Obsidian.Note"));
        reg.add_key(ck(r"Obsidian.Note\DefaultIcon"));
        // Unrelated ProgIDs and non-extension keys are not.
        reg.set(ck(".md"), "", s("Markdown"));
        reg.set(ck(".txt"), "", s("ObsidianLike.txt"));
        reg.set(ck("Obsidian Notes"), "", s("Obsidian Canvas"));
        let roots = RegScanRoots { software: vec![], classes: vec![key(Hive::CurrentUser, classes)], run: vec![], app_paths: vec![] };
        let out = scan_registry(&reg, &FakeFs::new(), &roots, &hint("Obsidian", None, None));
        let note = Some(NOTE_PROGID_OF_PROGRAM.to_string());
        assert_eq!(
            found(&out),
            vec![
                (format!(r"CurrentUser\{classes}\.base"), Confidence::Medium, note.clone()),
                (format!(r"CurrentUser\{classes}\.canvas"), Confidence::Medium, note.clone()),
                (format!(r"CurrentUser\{classes}\.obs"), Confidence::Medium, note.clone()),
                (format!(r"CurrentUser\{classes}\.vault"), Confidence::Medium, note.clone()),
                (format!(r"CurrentUser\{classes}\Obsidian.Vault"), Confidence::Medium, note.clone()),
            ]
        );
        assert_eq!(NOTE_PROGID_OF_PROGRAM, "progid-of-program");

        // A ProgID whose command points into the install folder stays High.
        let dir = r"C:\Apps\Obsidian";
        reg.set(ck(r"Obsidian.Vault\shell\open\command"), "", s(&format!(r#""{dir}\Obsidian.exe" "%1""#)));
        let out = scan_registry(&reg, &FakeFs::new(), &roots, &hint("Obsidian", None, Some(dir)));
        let vault = found(&out).into_iter().find(|(k, ..)| k.ends_with("Obsidian.Vault")).unwrap();
        assert_eq!(vault.1, Confidence::High);
        // A generic first word matches nothing.
        assert!(scan_registry(&reg, &FakeFs::new(), &roots, &hint("App Obsidian", None, None)).is_empty());
    }

    /// `.py`/`.pyw` → `Python.File`/`Python.NoConFile` → `C:\Windows\py.exe`, as the Python
    /// launcher registers them.
    fn python_classes(reg: &mut FakeRegistry) -> RegScanRoots {
        let ck = |p: &str| key(Hive::CurrentUser, &format!(r"Software\Classes\{p}"));
        reg.set(ck(".py"), "", s("Python.File"));
        reg.set(ck(".pyw"), "", s("Python.NoConFile"));
        reg.set(ck(r"Python.File\shell\open\command"), "", s(r#""C:\Windows\py.exe" "%L" %*"#));
        reg.set(ck(r"Python.NoConFile\shell\open\command"), "", s(r#""C:\Windows\pyw.exe" "%L" %*"#));
        RegScanRoots { software: vec![], classes: vec![key(Hive::CurrentUser, r"Software\Classes")], run: vec![], app_paths: vec![] }
    }

    #[test]
    fn python_file_types_with_live_handler_are_not_listed() {
        let mut reg = FakeRegistry::new();
        let roots = python_classes(&mut reg);
        let fs = FakeFs::new().file(r"C:\Windows\py.exe", 1).file(r"C:\Windows\pyw.exe", 1);
        let h = hint("Python 3.11.4 (64-bit)", None, Some(r"C:\Users\A\AppData\Local\Programs\Python\Python311"));
        let out = scan_registry(&reg, &fs, &roots, &h);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn orphaned_progid_file_type_is_listed() {
        let mut reg = FakeRegistry::new();
        let roots = python_classes(&mut reg);
        reg.set(key(Hive::CurrentUser, r"Software\Classes\.pyz"), "", s("Python.ArchiveFile"));
        let fs = FakeFs::new().file(r"C:\Windows\py.exe", 1).file(r"C:\Windows\pyw.exe", 1);
        let out = scan_registry(&reg, &fs, &roots, &hint("Python 3.11.4 (64-bit)", None, None));
        assert_eq!(
            found(&out),
            vec![(r"CurrentUser\Software\Classes\.pyz".to_string(), Confidence::Medium, Some(NOTE_PROGID_OF_PROGRAM.to_string()))]
        );
    }

    #[test]
    fn file_type_whose_handler_is_under_target_is_listed() {
        let dir = r"C:\Users\A\AppData\Local\Programs\Obsidian";
        let mut reg = FakeRegistry::new();
        let ck = |p: &str| key(Hive::CurrentUser, &format!(r"Software\Classes\{p}"));
        reg.set(ck(".canvas"), "", s("Obsidian Canvas"));
        reg.set(ck(r"Obsidian Canvas\shell\open\command"), "", s(&format!(r#""{dir}\Obsidian.exe" "%1""#)));
        let roots = RegScanRoots { software: vec![], classes: vec![key(Hive::CurrentUser, r"Software\Classes")], run: vec![], app_paths: vec![] };
        // The handler still exists (before the uninstall) but lies in the program's folder.
        let fs = FakeFs::new().file(&format!(r"{dir}\Obsidian.exe"), 1);
        let mut h = hint("Obsidian", None, None);
        h.uninstall_string = Some(format!(r#""{dir}\Uninstall Obsidian.exe" /currentuser"#));
        let out = scan_registry(&reg, &fs, &roots, &h);
        assert_eq!(
            found(&out),
            vec![
                (r"CurrentUser\Software\Classes\.canvas".to_string(), Confidence::Medium, Some(NOTE_PROGID_OF_PROGRAM.to_string())),
                (r"CurrentUser\Software\Classes\Obsidian Canvas".to_string(), Confidence::High, None),
            ]
        );
    }

    #[test]
    fn app_paths_key_is_high() {
        let mut reg = FakeRegistry::new();
        reg.set(key(Hive::LocalMachine, &format!(r"{APP_PATHS}\foo.exe")), "", s(&format!(r"{INSTALL}\foo.exe")));
        reg.set(key(Hive::LocalMachine, &format!(r"{APP_PATHS}\bar.exe")), "", s(r"C:\Bar\bar.exe"));
        reg.set(key(Hive::CurrentUser, &format!(r"{APP_PATHS}\FooStudio.exe")), "", s(&format!(r#""{INSTALL}\foo.exe""#)));
        let out = scan_registry(&reg, &FakeFs::new(), &RegScanRoots::standard(&reg), &hint("Foo Studio", None, Some(INSTALL)));
        assert_eq!(
            found(&out),
            vec![
                (format!(r"CurrentUser\{HKCU_APP_PATHS}\FooStudio.exe"), Confidence::High, None),
                (format!(r"LocalMachine\{APP_PATHS}\foo.exe"), Confidence::High, None),
            ]
        );
    }

    #[test]
    fn first_word_vendor_key_is_not_listed_but_its_matching_child_is() {
        let mut reg = FakeRegistry::new();
        reg.add_key(key(Hive::LocalMachine, r"SOFTWARE\Google\Chrome"));
        reg.add_key(key(Hive::LocalMachine, r"SOFTWARE\Google\Update"));
        let out = scan_registry(&reg, &FakeFs::new(), &RegScanRoots::standard(&reg), &hint("Google Chrome", None, None));
        assert_eq!(
            found(&out),
            vec![(r"LocalMachine\SOFTWARE\Google\Chrome".to_string(), Confidence::Medium, Some("shared-publisher-key".to_string()))]
        );
    }

    #[test]
    fn rule_f_ignores_generic_uninstaller_names() {
        let mut reg = FakeRegistry::new();
        reg.set(key(Hive::CurrentUser, RUN), "Discord", s(r"C:\Users\A\AppData\Local\discord\Update.exe --processStart Discord.exe"));
        let mut h = hint("Slack", None, None);
        h.uninstall_string = Some(r#""C:\Users\A\AppData\Local\slack\Update.exe" --uninstall"#.into());
        let out = scan_registry(&reg, &FakeFs::new(), &RegScanRoots::standard(&reg), &h);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn rule_f_matches_when_parent_folder_matches() {
        let mut reg = FakeRegistry::new();
        reg.set(key(Hive::CurrentUser, RUN), "Foo", s(r#""C:\Apps\Foo Studio\foo.exe" /tray"#));
        // Same exe name, but no folder above it names the program.
        reg.set(key(Hive::CurrentUser, RUN), "OtherFoo", s(r#""C:\Apps\Other\bin\foo.exe" /tray"#));
        let mut h = hint("Foo Studio", None, None);
        h.display_icon = Some(PathBuf::from(r"C:\Apps\Foo Studio\foo.exe"));
        let out = scan_registry(&reg, &FakeFs::new(), &RegScanRoots::standard(&reg), &h);
        assert_eq!(found(&out), vec![(format!(r"CurrentUser\{HKCU_RUN}:Foo"), Confidence::High, None)]);
    }

    #[test]
    fn generic_name_matches_nothing() {
        let mut reg = FakeRegistry::new();
        reg.add_key(key(Hive::LocalMachine, r"SOFTWARE\Tools"));
        reg.add_key(key(Hive::CurrentUser, r"Software\Tools\Tools"));
        reg.set(key(Hive::CurrentUser, RUN), "Tools", s(r"C:\Tools\tools.exe"));
        let out = scan_registry(&reg, &FakeFs::new(), &RegScanRoots::standard(&reg), &hint("Tools", Some("Tools"), None));
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn classes_scan_reads_only_server_values() {
        let mut reg = FakeRegistry::new();
        let guid = r"SOFTWARE\Classes\CLSID\{A}";
        reg.set(key(Hive::LocalMachine, &format!(r"{guid}\InprocServer32")), "", s(&format!(r"{INSTALL}\ext.dll")));
        reg.set(key(Hive::LocalMachine, guid), "", s("Foo Shell Extension"));
        reg.set(key(Hive::LocalMachine, &format!(r"{guid}\ProgID")), "", s("Foo.Ext"));
        reg.set(key(Hive::LocalMachine, &format!(r"{guid}\shell\open\command")), "", s("x"));
        let roots = RegScanRoots {
            software: vec![],
            classes: vec![key(Hive::LocalMachine, r"SOFTWARE\Classes")],
            run: vec![],
            app_paths: vec![],
        };
        let out = scan_registry(&reg, &FakeFs::new(), &roots, &hint("Foo Studio", None, Some(INSTALL)));
        assert_eq!(found(&out), vec![(format!(r"LocalMachine\{guid}"), Confidence::High, None)]);
        let reads = reg.reads();
        let inproc = format!(r"LocalMachine\{guid}\InprocServer32");
        assert!(reads.iter().any(|r| r.eq_ignore_ascii_case(&inproc)), "{reads:?}");
        let listings = [r"LocalMachine\SOFTWARE\Classes", r"LocalMachine\SOFTWARE\Classes\CLSID"];
        let server = |r: &str| {
            let rest = r.strip_prefix(&format!(r"LocalMachine\{guid}")).unwrap_or("");
            rest.eq_ignore_ascii_case(r"\InprocServer32") || rest.eq_ignore_ascii_case(r"\LocalServer32")
        };
        for r in &reads {
            assert!(listings.contains(&r.as_str()) || server(r), "unexpected read {r}");
        }
    }
}
