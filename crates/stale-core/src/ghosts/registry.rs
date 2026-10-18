//! Registry ghosts: vendor keys whose install path is gone, and Run / App Paths / Classes
//! entries that point at missing files.

use super::{format_location, Collected, Evidence, InstalledIndex, RefKind};
use crate::fsys::FsReader;
use crate::keep::KeepList;
use crate::leftovers::names::name_matches_exact;
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
    /// Existing targets of the Run / App Paths / COM server / shell command entries read.
    live: std::cell::RefCell<Vec<PathBuf>>,
    /// Names of the orphan folders found: a vendor key named exactly like one is that program's.
    folder_names: &'a [String],
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

    /// Records `p` as a live target when it is an existing absolute path; true then.
    fn note_live(&self, p: &Path) -> bool {
        let live = p.is_absolute() && drive_letter(p).is_some() && self.fs.exists(p);
        if live {
            self.live.borrow_mut().push(p.to_path_buf());
        }
        live
    }

    /// [`Self::is_dead`] for a reference's target; an existing target is recorded as live instead.
    fn is_dead_ref(&self, p: &Path) -> bool {
        !self.note_live(p) && self.is_dead(p)
    }

    fn examines(&self, k: &RegKeyPath) -> bool {
        !self.keep.is_key_kept(k)
    }
}

fn dangling(kind: RefKind, k: &RegKeyPath, target: PathBuf, value_name: Option<String>) -> Evidence {
    Evidence::DanglingRef { kind, location: format_location(k), target, value_name, partly_live: false }
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
    registry_collect(reg, fs, roots, installed, keep, drive_present, &[]).evidence
}

/// [`registry_evidence`], plus the existing targets of the Run values, App Paths entries, COM
/// servers and shell commands it read (live references). A vendor key without a path value
/// whose name is exactly one of `folder_names` (the orphan folders found) is a
/// [`Evidence::RegKeyOrphan`].
pub fn registry_collect(
    reg: &dyn RegistryReader,
    fs: &dyn FsReader,
    roots: &RegScanRoots,
    installed: &InstalledIndex,
    keep: &KeepList,
    drive_present: &dyn Fn(char) -> bool,
    folder_names: &[String],
) -> Collected {
    let cx = Ctx {
        reg,
        fs,
        installed,
        keep,
        drive_present,
        system_root: system_root_norm(),
        live: Default::default(),
        folder_names,
    };
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
    Collected { evidence: out, live_targets: cx.live.into_inner() }
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
        let children = cx.reg.subkeys(&k).unwrap_or_default();
        // Removing the key would take an installed program's subkey with it.
        let shared = children.iter().any(|c| cx.installed.matches_name(c) || cx.installed.matches_in_container(c, &sub));
        if !forced && path_values(cx, &k, shared, out) {
            continue;
        }
        // No path value, but named exactly like an orphan folder: that program's vendor key.
        if !forced && cx.folder_names.iter().any(|f| name_matches_exact(&sub, f)) {
            out.push(Evidence::RegKeyOrphan { key: k, shared });
            continue;
        }
        if !forced && children.len() < 2 {
            continue;
        }
        for c in children {
            let ck = child(&k, &c);
            if !cx.installed.matches_name(&c) && !cx.installed.matches_in_container(&c, &sub) && cx.examines(&ck) {
                let shared = cx.reg.subkeys(&ck).unwrap_or_default().iter().any(|g| cx.installed.matches_name(g));
                path_values(cx, &ck, shared, out);
            }
        }
    }
}

/// Pushes a [`Evidence::RegKeyMissingPath`] (with `shared`) per dead install-path value of `k`;
/// true when `k` has any value named in [`PATH_VALUE_NAMES`].
fn path_values(cx: &Ctx, k: &RegKeyPath, shared: bool, out: &mut Vec<Evidence>) -> bool {
    let mut has_path_value = false;
    for (name, value) in cx.reg.values(k).unwrap_or_default() {
        if !PATH_VALUE_NAMES.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
            continue;
        }
        has_path_value = true;
        if let Some(path) = string_value(&value, k).as_deref().and_then(install_dir).filter(|p| cx.is_dead(p)) {
            out.push(Evidence::RegKeyMissingPath { key: k.clone(), value_name: name, path, shared });
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

/// The DLL a `rundll32 <dll>,<entry>` command line loads; `None` for any other command.
fn rundll32_dll(cmd: &str) -> Option<PathBuf> {
    let exe = first_path_in_command(cmd)?;
    if !exe.file_stem()?.to_string_lossy().eq_ignore_ascii_case("rundll32") {
        return None;
    }
    let lower = cmd.to_ascii_lowercase();
    let mut at = lower.find("rundll32")? + "rundll32".len();
    if lower[at..].starts_with(".exe") {
        at += ".exe".len();
    }
    let rest = cmd[at..].trim_start_matches('"').trim_start();
    let dll = match rest.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next()?,
        None => rest.split(',').next()?,
    };
    let dll = dll.trim();
    (!dll.is_empty()).then(|| PathBuf::from(dll))
}

/// (b) Autostart command lines whose exe is missing. The existing exe, and the DLL of a
/// `rundll32` line, are live targets, under a kept key too.
fn run_values(cx: &Ctx, root: &RegKeyPath, out: &mut Vec<Evidence>) {
    let kept = !cx.examines(root);
    for (name, value) in cx.reg.values(root).unwrap_or_default() {
        let Some(cmd) = string_value(&value, root) else { continue };
        if let Some(dll) = rundll32_dll(&cmd) {
            cx.note_live(&dll);
        }
        if let Some(target) = first_path_in_command(&cmd).filter(|p| cx.is_dead_ref(p)) {
            if !kept {
                out.push(dangling(RefKind::Run, root, target, Some(name)));
            }
        }
    }
}

/// (c) `App Paths` entries whose default value is a missing file.
fn app_path_keys(cx: &Ctx, root: &RegKeyPath, out: &mut Vec<Evidence>) {
    for sub in cx.reg.subkeys(root).unwrap_or_default() {
        let k = child(root, &sub);
        let kept = !cx.examines(&k);
        if let Some(target) = default_string(cx.reg, &k).as_deref().and_then(path_in_value).filter(|p| cx.is_dead_ref(p)) {
            if !kept {
                out.push(dangling(RefKind::AppPaths, &k, target, None));
            }
        }
    }
}

/// (d) One pass over a classes root: per `CLSID\{…}` only the [`SERVER_KEYS`] defaults, per
/// other subkey only the `shell\open\command` default; nothing deeper. A kept key is never
/// evidence, but its existing targets are live.
fn class_keys(cx: &Ctx, root: &RegKeyPath, out: &mut Vec<Evidence>) {
    for sub in cx.reg.subkeys(root).unwrap_or_default() {
        let k = child(root, &sub);
        if sub.eq_ignore_ascii_case("CLSID") {
            for clsid in cx.reg.subkeys(&k).unwrap_or_default() {
                let c = child(&k, &clsid);
                let kept = !cx.examines(&c);
                // Every server is checked, so each existing one is recorded as live. A class
                // with any server that is not verifiably gone (live, a bare name such as
                // `mscoree.dll`, on a missing drive, kept or installed) is never evidence.
                let (mut target, mut other_alive) = (None, false);
                for p in SERVER_KEYS.iter().filter_map(|s| default_string(cx.reg, &child(&c, s)).as_deref().and_then(path_in_value)) {
                    if !cx.is_dead_ref(&p) {
                        other_alive = true;
                    } else if target.is_none() {
                        target = Some(p);
                    }
                }
                if let Some(target) = target.filter(|_| !kept && !other_alive) {
                    out.push(dangling(RefKind::ClassesServer, &c, target, None));
                }
            }
        } else {
            let command = default_string(cx.reg, &child(&k, r"shell\open\command"));
            if let Some(target) = command.as_deref().and_then(first_path_in_command).filter(|p| cx.is_dead_ref(p)) {
                if cx.examines(&k) {
                    out.push(dangling(RefKind::ShellCommand, &k, target, None));
                }
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
            account: None,
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
        Evidence::RegKeyMissingPath { key: key(hive, path), value_name: value_name.into(), path: PathBuf::from(target), shared: false }
    }

    fn dangling(kind: RefKind, location: &str, target: &str, value_name: Option<&str>) -> Evidence {
        Evidence::DanglingRef {
            kind,
            location: location.into(),
            target: PathBuf::from(target),
            value_name: value_name.map(str::to_string),
            partly_live: false,
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
        let _env = crate::env_lock();
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
            vec![dangling(RefKind::Run, &format!(r"HKCU\{RUN}"), r"C:\Gone Tray\tray.exe", Some("Gone"))]
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
                dangling(RefKind::AppPaths, &format!(r"HKLM\{APP_PATHS}\gone.exe"), r"C:\Gone\gone.exe", None),
                dangling(
                    RefKind::ShellCommand,
                    r"HKLM\SOFTWARE\Classes\goneproto",
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
                dangling(RefKind::ClassesServer, &format!(r"HKLM\{CLSID}\{{A}}"), r"C:\Program Files\Gone Ext\ext.dll", None),
                dangling(RefKind::ClassesServer, &format!(r"HKLM\{CLSID}\{{D}}"), r"D:\Gone\server.exe", None),
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
    fn existing_reference_targets_are_live() {
        let mut reg = FakeRegistry::new();
        let hklm = |p: &str| key(Hive::LocalMachine, p);
        reg.set(key(Hive::CurrentUser, RUN), "Tray", s(r#""C:\Apps\Live\tray.exe" --min"#));
        reg.set(hklm(&format!(r"{APP_PATHS}\live.exe")), "", s(r"C:\Apps\Live\live.exe"));
        reg.set(hklm(&format!(r"{CLSID}\{{A}}\InprocServer32")), "", s(r"C:\Apps\Live\ext.dll"));
        reg.set(hklm(r"SOFTWARE\Classes\liveproto\shell\open\command"), "", s(r#""C:\Apps\Live\open.exe" "%1""#));
        let fs = FakeFs::new()
            .file(r"C:\Apps\Live\tray.exe", 1)
            .file(r"C:\Apps\Live\live.exe", 1)
            .file(r"C:\Apps\Live\ext.dll", 1)
            .file(r"C:\Apps\Live\open.exe", 1);
        let got = registry_collect(&reg, &fs, &roots(), &InstalledIndex::default(), &KeepList::default(), &all_drives, &[]);
        assert_eq!(got.evidence, vec![]);
        let mut live: Vec<String> = got.live_targets.iter().map(|p| p.display().to_string()).collect();
        live.sort();
        assert_eq!(live, vec![r"C:\Apps\Live\ext.dll", r"C:\Apps\Live\live.exe", r"C:\Apps\Live\open.exe", r"C:\Apps\Live\tray.exe"]);
    }

    #[test]
    fn clsid_with_live_server_not_listed() {
        let mut reg = FakeRegistry::new();
        let hklm = |p: &str| key(Hive::LocalMachine, p);
        reg.set(hklm(&format!(r"{CLSID}\{{Mixed}}\InprocServer32")), "", s(r"C:\Apps\Live\ext.dll"));
        reg.set(hklm(&format!(r"{CLSID}\{{Mixed}}\LocalServer32")), "", s(r"C:\Apps\Gone\server.exe"));
        reg.set(hklm(&format!(r"{CLSID}\{{Dead}}\LocalServer32")), "", s(r"C:\Apps\Gone\other.exe"));
        let fs = FakeFs::new().file(r"C:\Apps\Live\ext.dll", 1);
        let got = registry_collect(&reg, &fs, &roots(), &InstalledIndex::default(), &KeepList::default(), &all_drives, &[]);
        let location = format!(r"HKLM\{CLSID}\{{Dead}}");
        assert_eq!(got.evidence, vec![dangling(RefKind::ClassesServer, &location, r"C:\Apps\Gone\other.exe", None)]);
        assert_eq!(got.live_targets, vec![PathBuf::from(r"C:\Apps\Live\ext.dll")]);
    }

    #[test]
    fn clsid_with_bare_server_not_listed() {
        let mut reg = FakeRegistry::new();
        let hklm = |p: &str| key(Hive::LocalMachine, p);
        reg.set(hklm(&format!(r"{CLSID}\{{Net}}\InprocServer32")), "", s("mscoree.dll"));
        reg.set(hklm(&format!(r"{CLSID}\{{Net}}\LocalServer32")), "", s(r"C:\Apps\Gone\server.exe"));
        let got = registry_collect(&reg, &FakeFs::new(), &roots(), &InstalledIndex::default(), &KeepList::default(), &all_drives, &[]);
        assert_eq!(got.evidence, vec![], "a server that is not verifiably gone keeps the class");
    }

    #[test]
    fn rundll32_dll_is_live() {
        let mut reg = FakeRegistry::new();
        let run = key(Hive::CurrentUser, RUN);
        let rundll32 = expand_env(r"%SystemRoot%\System32\rundll32.exe");
        reg.set(run.clone(), "Plain", s(&format!(r"{rundll32} C:\Vendor Tools\helper.dll,Start /x")));
        reg.set(run.clone(), "Quoted", s(&format!(r#""{rundll32}" "C:\Vendor Tools\quoted.dll",Run"#)));
        reg.set(run, "Bare", s(r"rundll32 C:\Vendor Tools\bare.dll,Go"));
        let fs = FakeFs::new()
            .file(r"C:\Vendor Tools\helper.dll", 1)
            .file(r"C:\Vendor Tools\quoted.dll", 1)
            .file(r"C:\Vendor Tools\bare.dll", 1);
        let got = registry_collect(&reg, &fs, &roots(), &InstalledIndex::default(), &KeepList::default(), &all_drives, &[]);
        assert_eq!(got.evidence, vec![]);
        let mut live: Vec<String> = got.live_targets.iter().map(|p| p.display().to_string()).collect();
        live.sort();
        assert_eq!(live, vec![r"C:\Vendor Tools\bare.dll", r"C:\Vendor Tools\helper.dll", r"C:\Vendor Tools\quoted.dll"]);
    }

    #[test]
    fn kept_key_target_is_live() {
        let mut reg = FakeRegistry::new();
        let hklm = |p: &str| key(Hive::LocalMachine, p);
        reg.set(key(Hive::CurrentUser, RUN), "Tray", s(r#""C:\Apps\Kept\tray.exe" --min"#));
        reg.set(hklm(&format!(r"{APP_PATHS}\kept.exe")), "", s(r"C:\Apps\Kept\kept.exe"));
        reg.set(hklm(&format!(r"{CLSID}\{{K}}\InprocServer32")), "", s(r"C:\Apps\Kept\ext.dll"));
        reg.set(hklm(&format!(r"{CLSID}\{{K}}\LocalServer32")), "", s(r"C:\Apps\Gone\server.exe"));
        reg.set(hklm(r"SOFTWARE\Classes\keptproto\shell\open\command"), "", s(r#""C:\Apps\Kept\open.exe" "%1""#));
        let fs = FakeFs::new()
            .file(r"C:\Apps\Kept\tray.exe", 1)
            .file(r"C:\Apps\Kept\kept.exe", 1)
            .file(r"C:\Apps\Kept\ext.dll", 1)
            .file(r"C:\Apps\Kept\open.exe", 1);
        let keep = KeepList {
            keys: vec![
                key(Hive::CurrentUser, RUN),
                hklm(&format!(r"{APP_PATHS}\kept.exe")),
                hklm(&format!(r"{CLSID}\{{K}}")),
                hklm(r"SOFTWARE\Classes\keptproto"),
            ],
            ..KeepList::default()
        };
        let got = registry_collect(&reg, &fs, &roots(), &InstalledIndex::default(), &keep, &all_drives, &[]);
        assert_eq!(got.evidence, vec![], "kept keys are never evidence");
        let mut live: Vec<String> = got.live_targets.iter().map(|p| p.display().to_string()).collect();
        live.sort();
        assert_eq!(live, vec![r"C:\Apps\Kept\ext.dll", r"C:\Apps\Kept\kept.exe", r"C:\Apps\Kept\open.exe", r"C:\Apps\Kept\tray.exe"]);
    }

    #[test]
    fn wow64_run_value_expands_to_x86() {
        let mut reg = FakeRegistry::new();
        let run = key(Hive::LocalMachine, &format!(r"SOFTWARE\WOW6432Node\{}", &RUN["Software\\".len()..]));
        reg.set(run.clone(), "Gone32", RegValue::ExpandStr(r#""%ProgramFiles%\Gone Wow\gone.exe" /min"#.into()));
        reg.set(run.clone(), "Plain32", s(r"%ProgramFiles%\Gone Plain\plain.exe"));
        let roots = RegScanRoots { software: vec![], classes: vec![], run: vec![run.clone()], app_paths: vec![] };
        let ev = registry_evidence(&reg, &FakeFs::new(), &roots, &InstalledIndex::default(), &KeepList::default(), &all_drives);
        let x86 = std::env::var("ProgramFiles(x86)").unwrap();
        let location = format!(r"HKLM\{}", run.path);
        assert_eq!(
            ev,
            vec![
                dangling(RefKind::Run, &location, &format!(r"{x86}\Gone Wow\gone.exe"), Some("Gone32")),
                dangling(RefKind::Run, &location, &format!(r"{x86}\Gone Plain\plain.exe"), Some("Plain32")),
            ]
        );
    }

    #[test]
    fn other_hive_user_variables_are_skipped() {
        let mut reg = FakeRegistry::new();
        let theirs = key(Hive::Users, r"S-1-5-21-9\Software\Microsoft\Windows\CurrentVersion\Run");
        reg.set(theirs.clone(), "Local", RegValue::ExpandStr(r"%LOCALAPPDATA%\Gone\g.exe".into()));
        reg.set(theirs.clone(), "Fixed", RegValue::ExpandStr(r"C:\Gone Fixed\f.exe".into()));
        let roots = RegScanRoots { software: vec![], classes: vec![], run: vec![theirs.clone()], app_paths: vec![] };
        let ev = registry_evidence(&reg, &FakeFs::new(), &roots, &InstalledIndex::default(), &KeepList::default(), &all_drives);
        assert_eq!(ev, vec![dangling(RefKind::Run, &format!(r"HKU\{}", theirs.path), r"C:\Gone Fixed\f.exe", Some("Fixed"))]);
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
