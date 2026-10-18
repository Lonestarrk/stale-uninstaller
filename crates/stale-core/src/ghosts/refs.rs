//! Dangling references (services, scheduled tasks, shortcuts) and execution traces (Prefetch,
//! BAM, UserAssist) of executables that no longer exist.

use super::{Collected, Evidence, InstalledIndex, RefKind};
use crate::fsys::FsReader;
use crate::keep::KeepList;
use crate::leftovers::other_scan::{task_commands, task_files};
use crate::leftovers::system_root_norm;
use crate::model::Hive;
use crate::pathutil::{drive_letter, expand_env, norm, normed_under};
use crate::registry::{key, RegValue, RegistryReader};
use crate::services::{normalize_image_path_with, SERVICES_ROOT};
use crate::shortcuts::ShortcutTarget;
use crate::usage::{UsageHit, UsageSource};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

struct Gate<'a> {
    fs: &'a dyn FsReader,
    installed: &'a InstalledIndex,
    keep: &'a KeepList,
    drive_present: &'a dyn Fn(char) -> bool,
    system_root: String,
}

impl Gate<'_> {
    /// An absolute path on a present drive, outside Windows, that no installed program or keep
    /// entry claims and that is missing on disk.
    fn is_dead(&self, p: &Path) -> bool {
        let n = norm(p);
        p.is_absolute()
            && drive_letter(p).is_some_and(|d| (self.drive_present)(d))
            && n != self.system_root
            && !normed_under(&n, &self.system_root)
            && !self.installed.covers_path(p)
            && !self.keep.is_path_kept(p)
            && !self.fs.exists(p)
    }

    /// An absolute path on a drive that exists: a live reference's target.
    fn is_live(&self, p: &Path) -> bool {
        p.is_absolute() && drive_letter(p).is_some() && self.fs.exists(p)
    }
}

/// `SERVICE_WIN32_OWN_PROCESS | SERVICE_WIN32_SHARE_PROCESS`: a service run as a process. Kernel
/// and file system drivers (`Type` 1 and 2) never count as ghost evidence.
const SERVICE_WIN32: u32 = 0x10 | 0x20;

fn gate<'a>(
    fs: &'a dyn FsReader,
    installed: &'a InstalledIndex,
    keep: &'a KeepList,
    drive_present: &'a dyn Fn(char) -> bool,
) -> Gate<'a> {
    Gate { fs, installed, keep, drive_present, system_root: system_root_norm() }
}

fn dangling(kind: RefKind, location: String, target: PathBuf) -> Evidence {
    Evidence::DanglingRef { kind, location, target, value_name: None, partly_live: false }
}

/// [`service_collect`]'s evidence.
pub fn service_evidence(
    reg: &dyn RegistryReader,
    fs: &dyn FsReader,
    installed: &InstalledIndex,
    keep: &KeepList,
    drive_present: &dyn Fn(char) -> bool,
) -> Vec<Evidence> {
    service_collect(reg, fs, installed, keep, drive_present).evidence
}

/// Win32 services (`Type` 16/32) whose `ImagePath` executable is missing. The existing
/// `ImagePath` of every service and driver (kernel and file system drivers included), and an
/// existing `Parameters\ServiceDll`, is a live target.
pub fn service_collect(
    reg: &dyn RegistryReader,
    fs: &dyn FsReader,
    installed: &InstalledIndex,
    keep: &KeepList,
    drive_present: &dyn Fn(char) -> bool,
) -> Collected {
    let g = gate(fs, installed, keep, drive_present);
    let mut out = Collected::default();
    let Ok(names) = reg.subkeys(&key(Hive::LocalMachine, SERVICES_ROOT)) else { return out };
    for name in names {
        let Ok(values) = reg.values(&key(Hive::LocalMachine, &format!(r"{SERVICES_ROOT}\{name}"))) else { continue };
        let mut raw = None;
        let mut service_type = 0u32;
        for (n, v) in values {
            match v {
                RegValue::Str(s) | RegValue::ExpandStr(s) if n.eq_ignore_ascii_case("ImagePath") => raw = Some(s),
                RegValue::Dword(t) if n.eq_ignore_ascii_case("Type") => service_type = t,
                _ => {}
            }
        }
        // A shared-process service's DLL (`Parameters\ServiceDll`) is loaded as long as it runs.
        if let Some(dll) = service_dll(reg, &name).filter(|p| g.is_live(p)) {
            out.live_targets.push(dll);
        }
        // Unquoted paths with spaces are resolved against the disk, never cut at the first space.
        let Some(target) = raw.as_deref().and_then(|r| normalize_image_path_with(r, &|p: &Path| fs.exists(p) && !fs.is_dir(p))) else {
            continue;
        };
        if g.is_live(&target) {
            out.live_targets.push(target);
        } else if service_type & SERVICE_WIN32 != 0 && g.is_dead(&target) {
            out.evidence.push(dangling(RefKind::Service, name, target));
        }
    }
    out
}

/// `Services\<name>\Parameters\ServiceDll`, expanded and unquoted.
fn service_dll(reg: &dyn RegistryReader, name: &str) -> Option<PathBuf> {
    let values = reg.values(&key(Hive::LocalMachine, &format!(r"{SERVICES_ROOT}\{name}\Parameters"))).ok()?;
    let raw = values.into_iter().find_map(|(n, v)| match v {
        RegValue::Str(s) | RegValue::ExpandStr(s) if n.eq_ignore_ascii_case("ServiceDll") => Some(s),
        _ => None,
    })?;
    let dll = expand_env(raw.trim());
    let dll = dll.trim().trim_matches('"').trim();
    (!dll.is_empty()).then(|| PathBuf::from(dll))
}

/// [`task_collect`]'s evidence.
pub fn task_evidence(
    fs: &dyn FsReader,
    tasks_root: &Path,
    installed: &InstalledIndex,
    keep: &KeepList,
    drive_present: &dyn Fn(char) -> bool,
) -> Vec<Evidence> {
    task_collect(fs, tasks_root, installed, keep, drive_present).evidence
}

/// Scheduled tasks (one evidence per task file) with a missing `<Command>`; every existing
/// command is a live target.
pub fn task_collect(
    fs: &dyn FsReader,
    tasks_root: &Path,
    installed: &InstalledIndex,
    keep: &KeepList,
    drive_present: &dyn Fn(char) -> bool,
) -> Collected {
    let g = gate(fs, installed, keep, drive_present);
    let mut files = Vec::new();
    task_files(fs, tasks_root, &mut files);
    let mut out = Collected::default();
    for file in files {
        let Some(xml) = fs.read_text(&file) else { continue };
        let (mut dead, mut partly_live) = (None, false);
        for command in task_commands(&xml) {
            if g.is_live(&command) {
                out.live_targets.push(command);
                partly_live = true;
            } else if dead.is_none() && g.is_dead(&command) {
                dead = Some(command);
            }
        }
        let (Some(target), Ok(rel)) = (dead, file.strip_prefix(tasks_root)) else { continue };
        let name = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join(r"\");
        out.evidence.push(Evidence::DanglingRef {
            kind: RefKind::ScheduledTask,
            location: name,
            target,
            value_name: None,
            partly_live,
        });
    }
    out
}

/// [`shortcut_collect`]'s evidence.
pub fn shortcut_evidence(
    fs: &dyn FsReader,
    targets: &[ShortcutTarget],
    installed: &InstalledIndex,
    keep: &KeepList,
    drive_present: &dyn Fn(char) -> bool,
) -> Vec<Evidence> {
    shortcut_collect(fs, targets, installed, keep, drive_present).evidence
}

/// Shortcuts that still exist but whose target is gone; existing targets are live.
pub fn shortcut_collect(
    fs: &dyn FsReader,
    targets: &[ShortcutTarget],
    installed: &InstalledIndex,
    keep: &KeepList,
    drive_present: &dyn Fn(char) -> bool,
) -> Collected {
    let g = gate(fs, installed, keep, drive_present);
    let mut out = Collected::default();
    for t in targets {
        if g.is_live(&t.path) {
            out.live_targets.push(t.path.clone());
        } else if fs.exists(&t.lnk) && g.is_dead(&t.path) {
            out.evidence.push(dangling(RefKind::Shortcut, t.lnk.to_string_lossy().into_owned(), t.path.clone()));
        }
    }
    out
}

/// [`usage_collect`]'s evidence.
pub fn usage_evidence(
    fs: &dyn FsReader,
    unmatched: &[UsageHit],
    installed: &InstalledIndex,
    keep: &KeepList,
    drive_present: &dyn Fn(char) -> bool,
) -> Vec<Evidence> {
    usage_collect(fs, unmatched, installed, keep, drive_present).evidence
}

/// Execution traces of executables that no longer exist, the newest hit per executable; the
/// executables that still exist are live targets.
/// A drive-less (volume-stripped Prefetch) path is looked up on every present drive: it is a
/// ghost only when none has it, and is reported on the first present drive.
pub fn usage_collect(
    fs: &dyn FsReader,
    unmatched: &[UsageHit],
    installed: &InstalledIndex,
    keep: &KeepList,
    drive_present: &dyn Fn(char) -> bool,
) -> Collected {
    let g = gate(fs, installed, keep, drive_present);
    let drives: Vec<char> = ('A'..='Z').filter(|&d| drive_present(d)).collect();
    let mut live = Vec::new();
    let mut newest: BTreeMap<String, (PathBuf, DateTime<Utc>, UsageSource)> = BTreeMap::new();
    for hit in unmatched {
        let raw = hit.exe_path.to_string_lossy();
        // Drive-less, but not a UNC path (`\\server\share\...`).
        let exe = if raw.starts_with('\\') && !raw.starts_with(r"\\") {
            let on = |d: &char| PathBuf::from(format!("{d}:{raw}"));
            if let Some(found) = drives.iter().map(on).find(|p| fs.exists(p)) {
                live.push(found);
                continue;
            }
            match drives.first() {
                Some(d) => on(d),
                None => continue,
            }
        } else {
            hit.exe_path.clone()
        };
        if g.is_live(&exe) {
            live.push(exe);
            continue;
        }
        if !g.is_dead(&exe) {
            continue;
        }
        let slot = newest.entry(norm(&exe)).or_insert_with(|| (exe.clone(), hit.last_run, hit.source));
        if hit.last_run > slot.1 {
            *slot = (exe, hit.last_run, hit.source);
        }
    }
    let evidence =
        newest.into_values().map(|(exe, last_run, source)| Evidence::UsageTrace { exe, last_run, source }).collect();
    Collected { evidence, live_targets: live }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsys::FakeFs;
    use crate::model::{Program, ProgramId, RegKeyPath, Scope};
    use crate::registry::FakeRegistry;
    use chrono::TimeZone;

    fn all(_: char) -> bool {
        true
    }

    fn none() -> (InstalledIndex, KeepList) {
        (InstalledIndex::default(), KeepList::default())
    }

    /// A Win32 own-process service (`Type` 16).
    fn svc(reg: &mut FakeRegistry, name: &str, image: &str) {
        svc_typed(reg, name, image, 16);
    }

    fn svc_typed(reg: &mut FakeRegistry, name: &str, image: &str, service_type: u32) {
        let k = key(Hive::LocalMachine, &format!(r"{SERVICES_ROOT}\{name}"));
        reg.set(k.clone(), "ImagePath", RegValue::ExpandStr(image.into()));
        reg.set(k, "Type", RegValue::Dword(service_type));
    }

    #[test]
    fn kernel_driver_is_never_evidence() {
        let mut reg = FakeRegistry::new();
        svc_typed(&mut reg, "GoneDrv", r"\??\C:\GoneVendor\gone.sys", 1);
        svc_typed(&mut reg, "GoneFs", r"\??\C:\GoneVendor\fs.sys", 2);
        svc_typed(&mut reg, "GoneShared", r"C:\GoneVendor\shared.exe -k x", 32);
        svc_typed(&mut reg, "UserTemplate", r"C:\GoneVendor\user.exe", 0x50);
        // An existing driver payload is a live target, never evidence.
        svc_typed(&mut reg, "Ld9BoxSup", r"\??\C:\Program Files\ldplayer9box\Ld9BoxSup.sys", 1);
        let fs = FakeFs::new().file(r"C:\Program Files\ldplayer9box\Ld9BoxSup.sys", 1);
        let (i, k) = none();
        let got = service_collect(&reg, &fs, &i, &k, &all);
        let mut evidence = got.evidence.clone();
        evidence.sort_by_key(|e| format!("{e:?}"));
        assert_eq!(
            evidence,
            vec![
                dang(RefKind::Service, "GoneShared", r"C:\GoneVendor\shared.exe"),
                dang(RefKind::Service, "UserTemplate", r"C:\GoneVendor\user.exe"),
            ]
        );
        assert_eq!(got.live_targets, vec![PathBuf::from(r"C:\Program Files\ldplayer9box\Ld9BoxSup.sys")]);
    }

    #[test]
    fn service_dll_is_live() {
        let _env = crate::env_lock();
        let mut reg = FakeRegistry::new();
        svc_typed(&mut reg, "VendorHost", r"%SystemRoot%\System32\svchost.exe -k vendor", 32);
        let params = key(Hive::LocalMachine, &format!(r"{SERVICES_ROOT}\VendorHost\Parameters"));
        reg.set(params, "ServiceDll", RegValue::ExpandStr(r"C:\Program Files\Vendor\vsvc.dll".into()));
        let fs = FakeFs::new().file(r"C:\Program Files\Vendor\vsvc.dll", 1);
        let (i, k) = none();
        let got = service_collect(&reg, &fs, &i, &k, &all);
        assert_eq!(got.evidence, vec![]);
        assert!(got.live_targets.contains(&PathBuf::from(r"C:\Program Files\Vendor\vsvc.dll")), "{:?}", got.live_targets);
    }

    #[test]
    fn truncated_prefix_is_not_reported_dead() {
        let mut reg = FakeRegistry::new();
        svc(&mut reg, "VendorAgent", r"C:\ProgramData\Vendor\Some Dir\agent.bin --run");
        svc(&mut reg, "VendorNt", r"\??\C:\ProgramData\Vendor\Some Dir\x.sys");
        let fs = FakeFs::new().file(r"C:\ProgramData\Vendor\Some Dir\agent.bin", 1).file(r"C:\ProgramData\Vendor\Some Dir\x.sys", 1);
        let (i, k) = none();
        let got = service_collect(&reg, &fs, &i, &k, &all);
        assert_eq!(got.evidence, vec![], "C:\\ProgramData\\Vendor\\Some is not the image");
        let mut live = got.live_targets.clone();
        live.sort();
        assert_eq!(
            live,
            vec![PathBuf::from(r"C:\ProgramData\Vendor\Some Dir\agent.bin"), PathBuf::from(r"C:\ProgramData\Vendor\Some Dir\x.sys")]
        );
    }

    #[test]
    fn directory_prefix_is_not_accepted() {
        // `C:\ProgramData\Vendor` is a folder: never the image of `Vendor Suite\gone.exe`.
        let mut reg = FakeRegistry::new();
        svc(&mut reg, "VendorSuite", r"C:\ProgramData\Vendor Suite\gone.exe -k run");
        let fs = FakeFs::new().dir(r"C:\ProgramData\Vendor", 0);
        let (i, k) = none();
        let got = service_collect(&reg, &fs, &i, &k, &all);
        assert_eq!(got.live_targets, Vec::<PathBuf>::new());
        assert_eq!(got.evidence, vec![dang(RefKind::Service, "VendorSuite", r"C:\ProgramData\Vendor Suite\gone.exe")]);
    }

    #[test]
    fn collectors_report_existing_targets_as_live() {
        let root = r"C:\Windows\System32\Tasks";
        let fs = FakeFs::new()
            .text(&format!(r"{root}\Vendor\Updater"), r#"<Command>"C:\Live\up.exe"</Command><Command>C:\Gone\b.exe</Command>"#)
            .file(r"C:\Live\up.exe", 1)
            .file(r"C:\Live\app.exe", 1)
            .file(r"C:\Menu\Live.lnk", 1)
            .file(r"D:\RAN\RAN.EXE", 1);
        let (i, k) = none();
        let tasks = task_collect(&fs, Path::new(root), &i, &k, &all);
        assert_eq!(tasks.live_targets, vec![PathBuf::from(r"C:\Live\up.exe")]);
        // The task also runs a live command: partly live.
        let partly = Evidence::DanglingRef {
            kind: RefKind::ScheduledTask,
            location: r"Vendor\Updater".into(),
            target: PathBuf::from(r"C:\Gone\b.exe"),
            value_name: None,
            partly_live: true,
        };
        assert_eq!(tasks.evidence, vec![partly]);
        let links = shortcut_collect(&fs, &[lnk(r"C:\Menu\Live.lnk", r"C:\Live\app.exe")], &i, &k, &all);
        assert_eq!((links.evidence, links.live_targets), (vec![], vec![PathBuf::from(r"C:\Live\app.exe")]));
        let hits = [hit(r"\RAN\RAN.EXE", 1, UsageSource::Prefetch), hit(r"C:\Live\app.exe", 1, UsageSource::Bam)];
        let usage = usage_collect(&fs, &hits, &i, &k, &|c| c == 'C' || c == 'D');
        assert_eq!(usage.evidence, vec![]);
        assert_eq!(usage.live_targets, vec![PathBuf::from(r"D:\RAN\RAN.EXE"), PathBuf::from(r"C:\Live\app.exe")]);
    }

    fn dang(kind: RefKind, location: &str, target: &str) -> Evidence {
        Evidence::DanglingRef { kind, location: location.into(), target: PathBuf::from(target), value_name: None, partly_live: false }
    }

    #[test]
    fn missing_service_image_is_evidence() {
        let mut reg = FakeRegistry::new();
        svc(&mut reg, "GoneSvc", r#""C:\Gone\svc.exe" -k run"#);
        svc(&mut reg, "AliveSvc", r"C:\Alive\svc.exe");
        svc(&mut reg, "UsbSvc", r"E:\Usb\svc.exe");
        let fs = FakeFs::new().file(r"C:\Alive\svc.exe", 1);
        let (i, k) = none();
        let no_e = |c: char| c != 'E';
        let ev = service_evidence(&reg, &fs, &i, &k, &no_e);
        assert_eq!(ev, vec![dang(RefKind::Service, "GoneSvc", r"C:\Gone\svc.exe")]);
    }

    #[test]
    fn system32_service_is_skipped() {
        let _env = crate::env_lock();
        let mut reg = FakeRegistry::new();
        svc(&mut reg, "Drv", r"System32\drivers\gone.sys");
        svc(&mut reg, "Host", r"%SystemRoot%\System32\svchost.exe -k netsvcs");
        let (i, k) = none();
        assert_eq!(service_evidence(&reg, &FakeFs::new(), &i, &k, &all), vec![]);
    }

    #[test]
    fn missing_task_command_is_evidence() {
        let root = r"C:\Windows\System32\Tasks";
        let fs = FakeFs::new()
            .text(&format!(r"{root}\Vendor\Updater"), r#"<Command>"C:\Gone\a.exe"</Command><Command>C:\Gone\b.exe</Command>"#)
            .text(&format!(r"{root}\Alive"), r"<Command>C:\Alive\a.exe</Command>")
            .text(&format!(r"{root}\Sys"), r"<Command>C:\Windows\System32\gone.exe</Command>")
            .file(r"C:\Alive\a.exe", 1);
        let (i, k) = none();
        let ev = task_evidence(&fs, Path::new(root), &i, &k, &all);
        assert_eq!(ev, vec![dang(RefKind::ScheduledTask, r"Vendor\Updater", r"C:\Gone\a.exe")]);
    }

    fn lnk(lnk: &str, path: &str) -> ShortcutTarget {
        ShortcutTarget { path: path.into(), arguments: String::new(), lnk: lnk.into() }
    }

    #[test]
    fn dangling_shortcut_is_evidence_and_existing_target_is_not() {
        let targets = [
            lnk(r"C:\Menu\Gone.lnk", r"C:\Gone\g.exe"),
            lnk(r"C:\Menu\Alive.lnk", r"C:\Alive\a.exe"),
            lnk(r"C:\Menu\Stale.lnk", r"C:\Gone\s.exe"),
            lnk(r"C:\Menu\Sys.lnk", r"C:\Windows\System32\gone.exe"),
        ];
        let fs = FakeFs::new()
            .file(r"C:\Menu\Gone.lnk", 1)
            .file(r"C:\Menu\Alive.lnk", 1)
            .file(r"C:\Menu\Sys.lnk", 1)
            .file(r"C:\Alive\a.exe", 1);
        let (i, k) = none();
        let ev = shortcut_evidence(&fs, &targets, &i, &k, &all);
        assert_eq!(ev, vec![dang(RefKind::Shortcut, r"C:\Menu\Gone.lnk", r"C:\Gone\g.exe")]);
    }

    fn hit(path: &str, day: u32, source: UsageSource) -> UsageHit {
        UsageHit { exe_path: path.into(), last_run: Utc.with_ymd_and_hms(2026, 1, day, 0, 0, 0).unwrap(), source, run_count: None }
    }

    #[test]
    fn usage_trace_for_missing_exe_newest_only() {
        let hits = [
            hit(r"C:\Gone\g.exe", 3, UsageSource::Bam),
            hit(r"c:\gone\G.EXE", 9, UsageSource::UserAssist),
            hit(r"C:\Alive\a.exe", 5, UsageSource::Bam),
            hit(r"E:\Usb\u.exe", 5, UsageSource::Bam),
        ];
        let fs = FakeFs::new().file(r"C:\Alive\a.exe", 1);
        let (i, k) = none();
        let no_e = |c: char| c != 'E';
        let ev = usage_evidence(&fs, &hits, &i, &k, &no_e);
        assert_eq!(ev.len(), 1, "{ev:?}");
        let Evidence::UsageTrace { exe, last_run, source } = &ev[0] else { panic!("{ev:?}") };
        assert_eq!(norm(exe), r"C:\GONE\G.EXE");
        assert_eq!(*last_run, Utc.with_ymd_and_hms(2026, 1, 9, 0, 0, 0).unwrap());
        assert_eq!(*source, UsageSource::UserAssist);
    }

    #[test]
    fn driveless_prefetch_hit_resolved_against_present_drives() {
        let hits = [hit(r"\PROGRAM FILES\GONE\G.EXE", 1, UsageSource::Prefetch), hit(r"\TOOLS\T.EXE", 1, UsageSource::Prefetch)];
        let fs = FakeFs::new().file(r"D:\TOOLS\T.EXE", 1);
        let (i, k) = none();
        let c_d = |c: char| c == 'C' || c == 'D';
        let ev = usage_evidence(&fs, &hits, &i, &k, &c_d);
        assert_eq!(ev.len(), 1, "{ev:?}");
        let Evidence::UsageTrace { exe, source, .. } = &ev[0] else { panic!("{ev:?}") };
        assert_eq!(exe, &PathBuf::from(r"C:\PROGRAM FILES\GONE\G.EXE"));
        assert_eq!(*source, UsageSource::Prefetch);
    }

    #[test]
    fn unc_hit_is_not_resolved_against_drives() {
        let hits = [hit(r"\\server\share\gone.exe", 1, UsageSource::Bam)];
        let (i, k) = none();
        assert_eq!(usage_evidence(&FakeFs::new(), &hits, &i, &k, &all), vec![]);
    }

    #[test]
    fn installed_target_is_not_evidence() {
        let p = Program {
            account: None,
            id: ProgramId("Suite".into()),
            key: RegKeyPath { hive: Hive::LocalMachine, path: r"Software\Suite".into() },
            name: "Suite".into(),
            publisher: None,
            version: None,
            install_location: Some(PathBuf::from(r"D:\Suite")),
            install_date: None,
            uninstall_string: None,
            quiet_uninstall_string: None,
            display_icon: None,
            is_msi: false,
            estimated_size_bytes: None,
            drive: None,
            hidden: false,
            scope: Scope::Machine,
        };
        let i = InstalledIndex::from_programs(&[p]);
        let k = KeepList::default();
        let fs = FakeFs::new().file(r"C:\Menu\S.lnk", 1);
        let t = lnk(r"C:\Menu\S.lnk", r"D:\Suite\s.exe");
        assert_eq!(shortcut_evidence(&fs, &[t], &i, &k, &all), vec![]);
        assert_eq!(usage_evidence(&fs, &[hit(r"D:\Suite\s.exe", 1, UsageSource::Bam)], &i, &k, &all), vec![]);
    }

    #[test]
    fn kept_target_is_not_evidence() {
        let i = InstalledIndex::default();
        let k = KeepList { paths: vec![PathBuf::from(r"D:\Kept")], ..KeepList::default() };
        let mut reg = FakeRegistry::new();
        svc(&mut reg, "Kept", r"D:\Kept\svc.exe");
        assert_eq!(service_evidence(&reg, &FakeFs::new(), &i, &k, &all), vec![]);
        let fs = FakeFs::new().text(r"C:\T\Kept", r"<Command>D:\Kept\a.exe</Command>");
        assert_eq!(task_evidence(&fs, Path::new(r"C:\T"), &i, &k, &all), vec![]);
        assert_eq!(usage_evidence(&fs, &[hit(r"D:\Kept\a.exe", 1, UsageSource::Bam)], &i, &k, &all), vec![]);
    }
}
