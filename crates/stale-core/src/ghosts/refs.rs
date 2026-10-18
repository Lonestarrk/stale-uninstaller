//! Dangling references (services, scheduled tasks, shortcuts) and execution traces (Prefetch,
//! BAM, UserAssist) of executables that no longer exist.

use super::{Evidence, InstalledIndex, RefKind};
use crate::fsys::FsReader;
use crate::keep::KeepList;
use crate::leftovers::other_scan::{task_commands, task_files};
use crate::leftovers::system_root_norm;
use crate::model::Hive;
use crate::pathutil::{drive_letter, first_path_in_command, norm, normed_under};
use crate::registry::{key, RegValue, RegistryReader};
use crate::services::{normalize_image_path, SERVICES_ROOT};
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
}

fn gate<'a>(
    fs: &'a dyn FsReader,
    installed: &'a InstalledIndex,
    keep: &'a KeepList,
    drive_present: &'a dyn Fn(char) -> bool,
) -> Gate<'a> {
    Gate { fs, installed, keep, drive_present, system_root: system_root_norm() }
}

fn dangling(kind: RefKind, location: String, target: PathBuf) -> Evidence {
    Evidence::DanglingRef { kind, location, target, value_name: None }
}

/// Services and drivers whose `ImagePath` executable is missing.
pub fn service_evidence(
    reg: &dyn RegistryReader,
    fs: &dyn FsReader,
    installed: &InstalledIndex,
    keep: &KeepList,
    drive_present: &dyn Fn(char) -> bool,
) -> Vec<Evidence> {
    let g = gate(fs, installed, keep, drive_present);
    let Ok(names) = reg.subkeys(&key(Hive::LocalMachine, SERVICES_ROOT)) else { return Vec::new() };
    names
        .into_iter()
        .filter_map(|name| {
            let values = reg.values(&key(Hive::LocalMachine, &format!(r"{SERVICES_ROOT}\{name}"))).ok()?;
            let raw = values.into_iter().find_map(|(n, v)| match v {
                RegValue::Str(s) | RegValue::ExpandStr(s) if n.eq_ignore_ascii_case("ImagePath") => Some(s),
                _ => None,
            })?;
            let image = normalize_image_path(&raw)?;
            let target = first_path_in_command(&image.to_string_lossy())?;
            g.is_dead(&target).then(|| dangling(RefKind::Service, name, target))
        })
        .collect()
}

/// Scheduled tasks (one evidence per task file) with a missing `<Command>`.
pub fn task_evidence(
    fs: &dyn FsReader,
    tasks_root: &Path,
    installed: &InstalledIndex,
    keep: &KeepList,
    drive_present: &dyn Fn(char) -> bool,
) -> Vec<Evidence> {
    let g = gate(fs, installed, keep, drive_present);
    let mut files = Vec::new();
    task_files(fs, tasks_root, &mut files);
    files
        .into_iter()
        .filter_map(|file| {
            let target = task_commands(&fs.read_text(&file)?).into_iter().find(|c| g.is_dead(c))?;
            let rel = file.strip_prefix(tasks_root).ok()?;
            let name = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join(r"\");
            Some(dangling(RefKind::ScheduledTask, name, target))
        })
        .collect()
}

/// Shortcuts that still exist but whose target is gone.
pub fn shortcut_evidence(
    fs: &dyn FsReader,
    targets: &[ShortcutTarget],
    installed: &InstalledIndex,
    keep: &KeepList,
    drive_present: &dyn Fn(char) -> bool,
) -> Vec<Evidence> {
    let g = gate(fs, installed, keep, drive_present);
    targets
        .iter()
        .filter(|t| fs.exists(&t.lnk) && g.is_dead(&t.path))
        .map(|t| dangling(RefKind::Shortcut, t.lnk.to_string_lossy().into_owned(), t.path.clone()))
        .collect()
}

/// Execution traces of executables that no longer exist, the newest hit per executable.
/// A drive-less (volume-stripped Prefetch) path is looked up on every present drive: it is a
/// ghost only when none has it, and is reported on the first present drive.
pub fn usage_evidence(
    fs: &dyn FsReader,
    unmatched: &[UsageHit],
    installed: &InstalledIndex,
    keep: &KeepList,
    drive_present: &dyn Fn(char) -> bool,
) -> Vec<Evidence> {
    let g = gate(fs, installed, keep, drive_present);
    let drives: Vec<char> = ('A'..='Z').filter(|&d| drive_present(d)).collect();
    let mut newest: BTreeMap<String, (PathBuf, DateTime<Utc>, UsageSource)> = BTreeMap::new();
    for hit in unmatched {
        let raw = hit.exe_path.to_string_lossy();
        // Drive-less, but not a UNC path (`\\server\share\...`).
        let exe = if raw.starts_with('\\') && !raw.starts_with(r"\\") {
            let on = |d: &char| PathBuf::from(format!("{d}:{raw}"));
            if drives.iter().any(|d| fs.exists(&on(d))) {
                continue;
            }
            match drives.first() {
                Some(d) => on(d),
                None => continue,
            }
        } else {
            hit.exe_path.clone()
        };
        if !g.is_dead(&exe) {
            continue;
        }
        let slot = newest.entry(norm(&exe)).or_insert_with(|| (exe.clone(), hit.last_run, hit.source));
        if hit.last_run > slot.1 {
            *slot = (exe, hit.last_run, hit.source);
        }
    }
    newest.into_values().map(|(exe, last_run, source)| Evidence::UsageTrace { exe, last_run, source }).collect()
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

    fn svc(reg: &mut FakeRegistry, name: &str, image: &str) {
        reg.set(key(Hive::LocalMachine, &format!(r"{SERVICES_ROOT}\{name}")), "ImagePath", RegValue::ExpandStr(image.into()));
    }

    fn dang(kind: RefKind, location: &str, target: &str) -> Evidence {
        Evidence::DanglingRef { kind, location: location.into(), target: PathBuf::from(target), value_name: None }
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
