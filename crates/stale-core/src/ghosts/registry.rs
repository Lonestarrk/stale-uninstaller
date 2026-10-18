//! Registry ghosts: vendor keys whose install path is gone, and Run / App Paths / Classes
//! entries that point at missing files.

use super::{format_location, Evidence, InstalledIndex, RefKind};
use crate::fsys::FsReader;
use crate::keep::KeepList;
use crate::leftovers::reg_scan::{child, default_string, path_in_value, string_value, RegScanRoots};
use crate::leftovers::system_root_norm;
use crate::model::RegKeyPath;
use crate::pathutil::{drive_letter, first_path_in_command, is_shared_root, norm, normed_under};
use crate::registry::RegistryReader;
use std::path::{Path, PathBuf};

/// Value names under a vendor key that hold its install folder (or main exe).
pub const PATH_VALUE_NAMES: [&str; 8] =
    ["InstallPath", "InstallDir", "InstallLocation", "Install_Dir", "InstallRoot", "Path", "AppPath", "ExePath"];

/// Key names under a software root that never belong to one program (compared
/// case-insensitively, so `Wow6432Node` is covered too).
const SKIP_KEYS: [&str; 7] = ["Microsoft", "Classes", "Policies", "Clients", "RegisteredApplications", "WOW6432Node", "Intel"];

/// Vendor keys that hold one key per product: their children are examined instead.
const CONTAINER_KEYS: [&str; 1] = ["Google"];

/// COM server subkeys of a `CLSID\{…}` key whose default value is the server's path.
const SERVER_KEYS: [&str; 2] = ["InprocServer32", "LocalServer32"];

struct Ctx<'a> {
    reg: &'a dyn RegistryReader,
    fs: &'a dyn FsReader,
    installed: &'a InstalledIndex,
    keep: &'a KeepList,
    drive_present: &'a dyn Fn(char) -> bool,
    system_root: String,
}

impl Ctx<'_> {
    /// An absolute path on a present drive, outside Windows and the shared roots, that no
    /// installed program or keep entry claims and that is missing on disk.
    fn is_dead(&self, p: &Path) -> bool {
        let n = norm(p);
        p.is_absolute()
            && drive_letter(p).is_some_and(|d| (self.drive_present)(d))
            && !is_shared_root(p)
            && n != self.system_root
            && !normed_under(&n, &self.system_root)
            && !self.installed.covers_path(p)
            && !self.keep.is_path_kept(p)
            && !self.fs.exists(p)
    }

    fn examines(&self, k: &RegKeyPath) -> bool {
        !self.keep.is_key_kept(k)
    }
}

fn dangling(kind: RefKind, k: &RegKeyPath, target: PathBuf, value_name: Option<String>) -> Evidence {
    Evidence::DanglingRef { kind, location: format_location(k), target, value_name }
}

/// Vendor keys whose install path is gone, then Run values, App Paths entries and COM classes /
/// shell commands that point at missing files. Targets on drives that are not present, inside
/// Windows, owned by an installed program or kept are never evidence; neither are kept keys.
pub fn registry_evidence(
    reg: &dyn RegistryReader,
    fs: &dyn FsReader,
    roots: &RegScanRoots,
    installed: &InstalledIndex,
    keep: &KeepList,
    drive_present: &dyn Fn(char) -> bool,
) -> Vec<Evidence> {
    let cx = Ctx { reg, fs, installed, keep, drive_present, system_root: system_root_norm() };
    let mut out = Vec::new();
    for root in &roots.software {
        software_keys(&cx, root, &mut out);
    }
    for root in &roots.run {
        run_values(&cx, root, &mut out);
    }
    for root in &roots.app_paths {
        app_path_keys(&cx, root, &mut out);
    }
    for root in &roots.classes {
        class_keys(&cx, root, &mut out);
    }
    out
}

/// (a) Direct children of a software root, and the children of container keys (a vendor in
/// [`CONTAINER_KEYS`], or 2+ subkeys and no path value of its own), one level deep.
fn software_keys(cx: &Ctx, root: &RegKeyPath, out: &mut Vec<Evidence>) {
    for sub in cx.reg.subkeys(root).unwrap_or_default() {
        if SKIP_KEYS.iter().any(|s| s.eq_ignore_ascii_case(&sub)) || cx.installed.matches_name(&sub) {
            continue;
        }
        let k = child(root, &sub);
        if !cx.examines(&k) {
            continue;
        }
        let forced = CONTAINER_KEYS.iter().any(|c| c.eq_ignore_ascii_case(&sub));
        if !forced && path_values(cx, &k, out) {
            continue;
        }
        let children = cx.reg.subkeys(&k).unwrap_or_default();
        if !forced && children.len() < 2 {
            continue;
        }
        for c in children {
            let ck = child(&k, &c);
            if !cx.installed.matches_name(&c) && !cx.installed.matches_in_container(&c, &sub) && cx.examines(&ck) {
                path_values(cx, &ck, out);
            }
        }
    }
}

/// Pushes a [`Evidence::RegKeyMissingPath`] per dead install-path value of `k`; true when `k`
/// has any value named in [`PATH_VALUE_NAMES`].
fn path_values(cx: &Ctx, k: &RegKeyPath, out: &mut Vec<Evidence>) -> bool {
    let mut has_path_value = false;
    for (name, value) in cx.reg.values(k).unwrap_or_default() {
        if !PATH_VALUE_NAMES.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
            continue;
        }
        has_path_value = true;
        if let Some(path) = string_value(&value).as_deref().and_then(install_dir).filter(|p| cx.is_dead(p)) {
            out.push(Evidence::RegKeyMissingPath { key: k.clone(), value_name: name, path });
        }
    }
    has_path_value
}

/// The folder an install-path value names: quotes and trailing separators stripped, a file
/// path replaced by its folder. `PATH`-style lists name no single folder.
fn install_dir(s: &str) -> Option<PathBuf> {
    let s = s.trim().trim_matches('"').trim().trim_end_matches(['\\', '/']);
    if s.is_empty() || s.contains(';') {
        return None;
    }
    let p = PathBuf::from(s);
    if looks_like_file(&p) {
        p.parent().map(Path::to_path_buf)
    } else {
        Some(p)
    }
}

/// A 1-4 character alphanumeric extension with a letter (`.exe`, `.dll`); `Foo 1.2` is a folder.
fn looks_like_file(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        (1..=4).contains(&e.len()) && e.chars().all(|c| c.is_ascii_alphanumeric()) && e.chars().any(|c| c.is_ascii_alphabetic())
    })
}

/// (b) Autostart command lines whose exe is missing.
fn run_values(cx: &Ctx, root: &RegKeyPath, out: &mut Vec<Evidence>) {
    if !cx.examines(root) {
        return;
    }
    for (name, value) in cx.reg.values(root).unwrap_or_default() {
        if let Some(target) = string_value(&value).as_deref().and_then(first_path_in_command).filter(|p| cx.is_dead(p)) {
            out.push(dangling(RefKind::Run, root, target, Some(name)));
        }
    }
}

/// (c) `App Paths` entries whose default value is a missing file.
fn app_path_keys(cx: &Ctx, root: &RegKeyPath, out: &mut Vec<Evidence>) {
    for sub in cx.reg.subkeys(root).unwrap_or_default() {
        let k = child(root, &sub);
        if !cx.examines(&k) {
            continue;
        }
        if let Some(target) = default_string(cx.reg, &k).as_deref().and_then(path_in_value).filter(|p| cx.is_dead(p)) {
            out.push(dangling(RefKind::AppPaths, &k, target, None));
        }
    }
}

/// (d) One pass over a classes root: per `CLSID\{…}` only the [`SERVER_KEYS`] defaults, per
/// other subkey only the `shell\open\command` default; nothing deeper.
fn class_keys(cx: &Ctx, root: &RegKeyPath, out: &mut Vec<Evidence>) {
    for sub in cx.reg.subkeys(root).unwrap_or_default() {
        let k = child(root, &sub);
        if sub.eq_ignore_ascii_case("CLSID") {
            for clsid in cx.reg.subkeys(&k).unwrap_or_default() {
                let c = child(&k, &clsid);
                if !cx.examines(&c) {
                    continue;
                }
                let target = SERVER_KEYS
                    .iter()
                    .filter_map(|s| default_string(cx.reg, &child(&c, s)).as_deref().and_then(path_in_value))
                    .find(|p| cx.is_dead(p));
                if let Some(target) = target {
                    out.push(dangling(RefKind::ClassesServer, &c, target, None));
                }
            }
        } else if cx.examines(&k) {
            let command = default_string(cx.reg, &child(&k, r"shell\open\command"));
            if let Some(target) = command.as_deref().and_then(first_path_in_command).filter(|p| cx.is_dead(p)) {
                out.push(dangling(RefKind::ShellCommand, &k, target, None));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsys::FakeFs;
    use crate::ghosts::RefKind;
    use crate::model::{Hive, Program, ProgramId, RegKeyPath, Scope};
    use crate::pathutil::expand_env;
    use crate::registry::{key, FakeRegistry, RegValue};
    use std::path::PathBuf;

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const APP_PATHS: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths";
    const CLSID: &str = r"SOFTWARE\Classes\CLSID";

    fn roots() -> RegScanRoots {
        RegScanRoots {
            software: vec![key(Hive::LocalMachine, "SOFTWARE"), key(Hive::CurrentUser, "Software")],
            classes: vec![key(Hive::LocalMachine, r"SOFTWARE\Classes")],
            run: vec![key(Hive::CurrentUser, RUN)],
            app_paths: vec![key(Hive::LocalMachine, APP_PATHS)],
        }
    }

    fn program(name: &str, install: Option<&str>) -> Program {
        Program {
            id: ProgramId(name.into()),
            key: RegKeyPath { hive: Hive::LocalMachine, path: format!(r"Software\{name}") },
            name: name.into(),
            publisher: None,
            version: None,
            install_location: install.map(PathBuf::from),
            install_date: None,
            uninstall_string: None,
            quiet_uninstall_string: None,
            display_icon: None,
            is_msi: false,
            estimated_size_bytes: None,
            drive: None,
            hidden: false,
            scope: Scope::Machine,
        }
    }

    fn s(v: &str) -> RegValue {
        RegValue::Str(v.into())
    }

    fn all_drives(_: char) -> bool {
        true
    }

    fn scan(reg: &FakeRegistry, fs: &FakeFs, programs: &[Program], keep: &KeepList) -> Vec<Evidence> {
        registry_evidence(reg, fs, &roots(), &InstalledIndex::from_programs(programs), keep, &all_drives)
    }

    fn missing(hive: Hive, path: &str, value_name: &str, target: &str) -> Evidence {
        Evidence::RegKeyMissingPath { key: key(hive, path), value_name: value_name.into(), path: PathBuf::from(target) }
    }

    fn dangling(kind: RefKind, location: &str, target: &str, value_name: Option<&str>) -> Evidence {
        Evidence::DanglingRef {
            kind,
            location: location.into(),
            target: PathBuf::from(target),
            value_name: value_name.map(str::to_string),
        }
    }

    #[test]
    fn missing_install_path_value_is_evidence() {
        let mut reg = FakeRegistry::new();
        let hklm = |p: &str| key(Hive::LocalMachine, p);
        let hkcu = |p: &str| key(Hive::CurrentUser, p);
        // Quotes and the trailing separator are stripped.
        reg.set(hklm(r"SOFTWARE\Gone App"), "InstallLocation", s(r#""D:\Apps\Gone App\""#));
        // A file path names its folder.
        reg.set(hkcu(r"Software\Old Tool"), "exepath", RegValue::ExpandStr(r"C:\Tools\Old\old.exe".into()));
        // A container (2+ subkeys, no path value of its own) is descended one level only.
        reg.set(hklm(r"SOFTWARE\Gone Corp\Viewer"), "InstallDir", s(r"D:\Gone Corp\Viewer"));
        reg.add_key(hklm(r"SOFTWARE\Gone Corp\Shared"));
        reg.set(hklm(r"SOFTWARE\Gone Corp\Shared\Deep"), "InstallDir", s(r"D:\Deep"));
        // `Google` is a container even with a single child.
        reg.set(hklm(r"SOFTWARE\Google\Picasa"), "Path", s(r"C:\Program Files\Picasa"));
        // Skipped key names, and values not named like an install path.
        reg.set(hklm(r"SOFTWARE\Microsoft\Gone"), "InstallPath", s(r"C:\Gone"));
        reg.set(hklm(r"SOFTWARE\Policies\Gone"), "InstallPath", s(r"C:\Gone"));
        reg.set(hklm(r"SOFTWARE\Intel\Gone"), "InstallPath", s(r"C:\Gone"));
        reg.set(hklm(r"SOFTWARE\WOW6432Node\Gone"), "InstallPath", s(r"C:\Gone"));
        reg.set(hklm(r"SOFTWARE\Data Only"), "DataFolder", s(r"C:\Gone"));
        let ev = scan(&reg, &FakeFs::new(), &[], &KeepList::default());
        assert_eq!(
            ev,
            vec![
                missing(Hive::LocalMachine, r"SOFTWARE\Gone App", "InstallLocation", r"D:\Apps\Gone App"),
                missing(Hive::LocalMachine, r"SOFTWARE\Gone Corp\Viewer", "InstallDir", r"D:\Gone Corp\Viewer"),
                missing(Hive::LocalMachine, r"SOFTWARE\Google\Picasa", "Path", r"C:\Program Files\Picasa"),
                missing(Hive::CurrentUser, r"Software\Old Tool", "exepath", r"C:\Tools\Old"),
            ]
        );
    }

    #[test]
    fn existing_path_is_not_evidence() {
        let mut reg = FakeRegistry::new();
        let hklm = |p: &str| key(Hive::LocalMachine, p);
        reg.set(hklm(r"SOFTWARE\Present"), "InstallDir", s(r"c:\apps\present\"));
        reg.set(hklm(r"SOFTWARE\Present Exe"), "ExePath", s(r"C:\Apps\Present\run.exe"));
        // Inside an installed program's folder, even though that folder is gone.
        reg.set(hklm(r"SOFTWARE\Plugin Bits"), "InstallPath", s(r"D:\Suite\Plugins"));
        // Relative paths, `PATH`-style lists and non-string values say nothing.
        reg.set(hklm(r"SOFTWARE\Relative"), "Path", s(r"bin\tools"));
        reg.set(hklm(r"SOFTWARE\Search List"), "Path", s(r"C:\Gone A;C:\Gone B"));
        reg.set(hklm(r"SOFTWARE\Dword"), "InstallPath", RegValue::Dword(1));
        let fs = FakeFs::new().file(r"C:\Apps\Present\run.exe", 1);
        let ev = scan(&reg, &fs, &[program("Some Suite", Some(r"D:\Suite"))], &KeepList::default());
        assert_eq!(ev, vec![]);
    }

    #[test]
    fn installed_name_key_is_skipped() {
        let mut reg = FakeRegistry::new();
        let hklm = |p: &str| key(Hive::LocalMachine, p);
        reg.set(hklm(r"SOFTWARE\FooStudio"), "InstallPath", s(r"D:\Moved\Foo Studio"));
        reg.set(hklm(r"SOFTWARE\Foo Corp\Foo Studio"), "InstallPath", s(r"D:\Moved\Foo Studio"));
        reg.set(hklm(r"SOFTWARE\Foo Corp\Bar Player"), "InstallPath", s(r"D:\Gone\Bar Player"));
        let ev = scan(&reg, &FakeFs::new(), &[program("Foo Studio", Some(r"E:\Foo Studio"))], &KeepList::default());
        assert_eq!(ev, vec![missing(Hive::LocalMachine, r"SOFTWARE\Foo Corp\Bar Player", "InstallPath", r"D:\Gone\Bar Player")]);
    }

    #[test]
    fn dangling_run_value_is_evidence() {
        let mut reg = FakeRegistry::new();
        let run = key(Hive::CurrentUser, RUN);
        reg.set(run.clone(), "Gone", s(r#""C:\Gone Tray\tray.exe" --minimized"#));
        reg.set(run.clone(), "Alive", s(r"C:\Apps\Alive\alive.exe /background"));
        // Windows' own launchers and installed programs' missing exes are not ghosts.
        reg.set(run.clone(), "Sys", s(&expand_env(r"%SystemRoot%\System32\rundll32.exe gone.dll,Entry")));
        reg.set(run.clone(), "Installed", s(r"D:\Suite\updater.exe"));
        reg.set(run, "Number", RegValue::Dword(3));
        let fs = FakeFs::new().file(r"C:\Apps\Alive\alive.exe", 1);
        let ev = scan(&reg, &fs, &[program("Some Suite", Some(r"D:\Suite"))], &KeepList::default());
        assert_eq!(
            ev,
            vec![dangling(RefKind::Run, &format!(r"CurrentUser\{RUN}"), r"C:\Gone Tray\tray.exe", Some("Gone"))]
        );
    }

    #[test]
    fn dangling_app_path_and_shell_command_are_evidence() {
        let mut reg = FakeRegistry::new();
        let hklm = |p: &str| key(Hive::LocalMachine, p);
        reg.set(hklm(&format!(r"{APP_PATHS}\gone.exe")), "", s(r#""C:\Gone\gone.exe""#));
        reg.set(hklm(&format!(r"{APP_PATHS}\alive.exe")), "", s(r"C:\Apps\Alive\alive.exe"));
        reg.set(hklm(r"SOFTWARE\Classes\goneproto\shell\open\command"), "", s(r#"C:\Program Files\Gone\gone.exe "%1""#));
        reg.set(hklm(r"SOFTWARE\Classes\aliveproto\shell\open\command"), "", s(r#""C:\Apps\Alive\alive.exe" "%1""#));
        let fs = FakeFs::new().file(r"C:\Apps\Alive\alive.exe", 1);
        let ev = scan(&reg, &fs, &[], &KeepList::default());
        assert_eq!(
            ev,
            vec![
                dangling(RefKind::AppPaths, &format!(r"LocalMachine\{APP_PATHS}\gone.exe"), r"C:\Gone\gone.exe", None),
                dangling(
                    RefKind::ShellCommand,
                    r"LocalMachine\SOFTWARE\Classes\goneproto",
                    r"C:\Program Files\Gone\gone.exe",
                    None
                ),
            ]
        );
    }

    #[test]
    fn run_value_on_absent_drive_is_skipped() {
        let mut reg = FakeRegistry::new();
        reg.set(key(Hive::CurrentUser, RUN), "Usb", s(r"E:\Portable\tool.exe"));
        reg.set(key(Hive::LocalMachine, r"SOFTWARE\Usb Tool"), "InstallDir", s(r"e:\Portable"));
        let no_e = |c: char| c != 'E';
        let installed = InstalledIndex::default();
        let ev = registry_evidence(&reg, &FakeFs::new(), &roots(), &installed, &KeepList::default(), &no_e);
        assert_eq!(ev, vec![]);
        // With the drive plugged in, both are ghosts.
        let ev = registry_evidence(&reg, &FakeFs::new(), &roots(), &installed, &KeepList::default(), &all_drives);
        assert_eq!(ev.len(), 2, "{ev:?}");
    }

    #[test]
    fn dangling_clsid_server_is_evidence() {
        let mut reg = FakeRegistry::new();
        let hklm = |p: &str| key(Hive::LocalMachine, p);
        // An unquoted DLL path with spaces is one path.
        reg.set(hklm(&format!(r"{CLSID}\{{A}}\InprocServer32")), "", s(r"C:\Program Files\Gone Ext\ext.dll"));
        reg.set(hklm(&format!(r"{CLSID}\{{A}}")), "", s("Gone Shell Extension"));
        reg.set(hklm(&format!(r"{CLSID}\{{A}}\ProgID")), "", s("Gone.Ext"));
        reg.set(hklm(&format!(r"{CLSID}\{{B}}\LocalServer32")), "", s(r#""C:\Apps\Alive\alive.exe" -Embedding"#));
        reg.set(hklm(&format!(r"{CLSID}\{{C}}\InprocServer32")), "", s("mscoree.dll"));
        reg.set(hklm(&format!(r"{CLSID}\{{D}}\LocalServer32")), "", s(r"D:\Gone\server.exe /automation"));
        let fs = FakeFs::new().file(r"C:\Apps\Alive\alive.exe", 1);
        let roots = RegScanRoots { software: vec![], classes: roots().classes, run: vec![], app_paths: vec![] };
        let ev = registry_evidence(&reg, &fs, &roots, &InstalledIndex::default(), &KeepList::default(), &all_drives);
        assert_eq!(
            ev,
            vec![
                dangling(RefKind::ClassesServer, &format!(r"LocalMachine\{CLSID}\{{A}}"), r"C:\Program Files\Gone Ext\ext.dll", None),
                dangling(RefKind::ClassesServer, &format!(r"LocalMachine\{CLSID}\{{D}}"), r"D:\Gone\server.exe", None),
            ]
        );
        // One pass: only the root listings and the server defaults are read.
        let listings = [r"LocalMachine\SOFTWARE\Classes".to_string(), format!(r"LocalMachine\{CLSID}")];
        for r in reg.reads() {
            let server = r.ends_with(r"\InprocServer32") || r.ends_with(r"\LocalServer32");
            assert!(listings.contains(&r) || server, "unexpected read {r}");
        }
    }

    #[test]
    fn kept_key_is_skipped() {
        let mut reg = FakeRegistry::new();
        let hklm = |p: &str| key(Hive::LocalMachine, p);
        reg.set(hklm(r"SOFTWARE\Kept App"), "InstallDir", s(r"D:\Kept App"));
        reg.set(hklm(r"SOFTWARE\Kept Corp\One"), "InstallDir", s(r"D:\One"));
        reg.set(hklm(r"SOFTWARE\Kept Corp\Two"), "InstallDir", s(r"D:\Two"));
        reg.set(hklm(r"SOFTWARE\Kept Path"), "InstallDir", s(r"D:\Kept Path\bin"));
        reg.set(hklm(r"SOFTWARE\Dropped"), "InstallDir", s(r"D:\Dropped"));
        reg.set(key(Hive::CurrentUser, RUN), "Gone", s(r"C:\Gone\gone.exe"));
        reg.set(hklm(&format!(r"{CLSID}\{{A}}\InprocServer32")), "", s(r"C:\Gone\ext.dll"));
        let keep = KeepList {
            keys: vec![
                hklm(r"software\kept app"),
                hklm(r"SOFTWARE\Kept Corp"),
                key(Hive::CurrentUser, RUN),
                hklm(&format!(r"{CLSID}\{{A}}")),
            ],
            paths: vec![PathBuf::from(r"D:\Kept Path")],
            ..KeepList::default()
        };
        let ev = scan(&reg, &FakeFs::new(), &[], &keep);
        assert_eq!(ev, vec![missing(Hive::LocalMachine, r"SOFTWARE\Dropped", "InstallDir", r"D:\Dropped")]);
    }
}
