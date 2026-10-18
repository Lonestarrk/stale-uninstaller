//! Combines the folder, registry, service, task and shortcut scanners into one report.

use super::fs_scan::{scan_folders, FsScanRoots};
use super::other_scan::{desktop_dirs, scan_services, scan_shortcuts, scan_tasks};
use super::reg_scan::{scan_registry, RegScanRoots};
use super::{mark_used_by_others, Confidence, Leftover, LeftoverKind, LeftoverReport, ProgramHint};
use crate::keep::KeepList;
use crate::fsys::{FsReader, RealFs};
use crate::pathutil::norm;
use crate::registry::{RegistryReader, WinRegistry};
use crate::shortcuts::{shortcut_targets, start_menu_dirs, ShortcutTarget};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Everything the leftover scanners read from, so tests can substitute fakes.
pub struct LeftoverSources {
    pub reg: Arc<dyn RegistryReader>,
    pub fs: Arc<dyn FsReader>,
    pub fs_roots: FsScanRoots,
    pub reg_roots: RegScanRoots,
    pub tasks_root: PathBuf,
    pub start_menu_targets: Vec<ShortcutTarget>,
    pub desktop_targets: Vec<ShortcutTarget>,
    pub keep: KeepList,
}

impl LeftoverSources {
    pub fn live() -> LeftoverSources {
        let reg: Arc<dyn RegistryReader> = Arc::new(WinRegistry);
        let fs: Arc<dyn FsReader> = Arc::new(RealFs);
        let system_root = fs.env("SystemRoot").unwrap_or_else(|| r"C:\Windows".to_string());
        LeftoverSources {
            fs_roots: FsScanRoots::from_env(&*fs),
            reg_roots: RegScanRoots::standard(&*reg),
            tasks_root: PathBuf::from(system_root).join("System32").join("Tasks"),
            start_menu_targets: shortcut_targets(&start_menu_dirs()),
            desktop_targets: shortcut_targets(&desktop_dirs(&*fs)),
            keep: KeepList::load_from(&crate::paths::keep_list_path()),
            reg,
            fs,
        }
    }
}

pub(crate) fn dedupe_key(kind: &LeftoverKind) -> String {
    match kind {
        LeftoverKind::Dir { path, .. } | LeftoverKind::File { path, .. } => norm(path),
        LeftoverKind::RegKey { key } => format!(r"{:?}\{}", key.hive, key.path).to_lowercase(),
        LeftoverKind::RegValue { key, name } => {
            format!(r"{:?}\{}\\{}", key.hive, key.path, name).to_lowercase()
        }
        LeftoverKind::Shortcut { lnk, .. } => norm(lnk),
        LeftoverKind::Service { name, .. } | LeftoverKind::ScheduledTask { name, .. } => name.to_lowercase(),
    }
}

pub(crate) fn kind_group(kind: &LeftoverKind) -> u8 {
    match kind {
        LeftoverKind::Dir { .. } | LeftoverKind::File { .. } => 0,
        LeftoverKind::RegKey { .. } | LeftoverKind::RegValue { .. } => 1,
        LeftoverKind::Shortcut { .. } | LeftoverKind::Service { .. } | LeftoverKind::ScheduledTask { .. } => 2,
    }
}

pub(crate) fn rank(c: Confidence) -> u8 {
    match c {
        Confidence::High => 0,
        Confidence::Medium => 1,
        Confidence::Low => 2,
    }
}

/// Runs every scanner, merges duplicates (higher confidence wins, first on a tie) and sorts
/// High, Medium, Low, then folders/files, registry, shortcuts/services/tasks. Shortcuts were
/// resolved when `src` was built, so only those whose `.lnk` still exists are kept (an
/// uninstaller may have removed them since).
pub fn scan_leftovers(src: &LeftoverSources, hint: &ProgramHint) -> LeftoverReport {
    let all = scan_folders(&*src.fs, &src.fs_roots, hint)
        .into_iter()
        .chain(scan_registry(&*src.reg, &src.reg_roots, hint))
        .chain(scan_services(&*src.reg, hint))
        .chain(scan_tasks(&*src.fs, &src.tasks_root, hint))
        .chain(
            scan_shortcuts(&src.start_menu_targets, &src.desktop_targets, hint)
                .into_iter()
                .filter(|l| matches!(&l.kind, LeftoverKind::Shortcut { lnk, .. } if src.fs.exists(lnk))),
        );

    let mut items: Vec<Leftover> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for item in all {
        let key = dedupe_key(&item.kind);
        match index.get(&key) {
            Some(&i) => {
                if rank(item.confidence) < rank(items[i].confidence) {
                    items[i] = item;
                }
            }
            None => {
                index.insert(key, items.len());
                items.push(item);
            }
        }
    }
    mark_used_by_others(&mut items, &hint.other_locations);
    items.sort_by_key(|l| (rank(l.confidence), kind_group(&l.kind)));
    filter_kept(LeftoverReport { items }, &src.keep)
}

/// Drops what the user asked to keep: folders and files under a kept path, registry keys and
/// values under a kept key, shortcuts targeting, and services and tasks running, something under
/// a kept path.
pub fn filter_kept(report: LeftoverReport, keep: &KeepList) -> LeftoverReport {
    let kept = |kind: &LeftoverKind| match kind {
        LeftoverKind::Dir { path, .. } | LeftoverKind::File { path, .. } => keep.is_path_kept(path),
        LeftoverKind::RegKey { key } | LeftoverKind::RegValue { key, .. } => keep.is_key_kept(key),
        LeftoverKind::Shortcut { target, .. } => keep.is_path_kept(target),
        LeftoverKind::Service { image, .. } => keep.is_path_kept(image),
        LeftoverKind::ScheduledTask { command, .. } => keep.is_path_kept(command),
    };
    LeftoverReport { items: report.items.into_iter().filter(|l| !kept(&l.kind)).collect() }
}

/// "Markera säkra": true for High and Medium, false for Low.
pub fn mark_safe(report: &LeftoverReport) -> Vec<bool> {
    report.items.iter().map(Leftover::default_checked).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsys::FakeFs;
    use crate::model::Hive;
    use crate::registry::{key, FakeRegistry};

    fn sources(fs: FakeFs, reg: FakeRegistry, shortcuts: Vec<ShortcutTarget>) -> LeftoverSources {
        let fs = fs.env("ProgramFiles", r"C:\Program Files").env("ProgramData", r"C:\ProgramData");
        let fs: Arc<dyn FsReader> = Arc::new(fs.dir(r"C:\Program Files", 0).profile(r"C:\Users\A"));
        let reg: Arc<dyn RegistryReader> = Arc::new(reg);
        LeftoverSources {
            fs_roots: FsScanRoots::from_env(&*fs),
            reg_roots: RegScanRoots::standard(&*reg),
            tasks_root: PathBuf::from(r"C:\Windows\System32\Tasks"),
            start_menu_targets: shortcuts,
            desktop_targets: Vec::new(),
            keep: KeepList::default(),
            reg,
            fs,
        }
    }

    fn lnk(target: &str, lnk: &str) -> ShortcutTarget {
        let mut t = ShortcutTarget::new(target);
        t.lnk = PathBuf::from(lnk);
        t
    }

    fn registry_with_vendor_key() -> FakeRegistry {
        let mut reg = FakeRegistry::new();
        reg.add_key(key(Hive::CurrentUser, r"Software\Unity Hub"));
        reg
    }

    fn full_sources() -> LeftoverSources {
        let fs = FakeFs::new()
            .dir(r"C:\Program Files\Unity Hub", 10)
            .dir(r"C:\Users\A\AppData\Roaming\UnityHub", 5)
            .dir(r"C:\Users\A\Documents\Unity Hub", 1)
            .file(r"C:\Menu\Unity Hub.lnk", 1);
        sources(fs, registry_with_vendor_key(), vec![lnk(r"C:\Program Files\Unity Hub\Unity Hub.exe", r"C:\Menu\Unity Hub.lnk")])
    }

    fn hint() -> ProgramHint {
        ProgramHint::manual("Unity Hub", Some(PathBuf::from(r"C:\Program Files\Unity Hub")))
    }

    #[test]
    fn report_is_sorted_by_confidence_then_kind() {
        let report = scan_leftovers(&full_sources(), &hint());
        let order: Vec<_> = report.items.iter().map(|l| (rank(l.confidence), kind_group(&l.kind))).collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(order, sorted);
        // High dir, High shortcut, and at least one non-High item exist, so ordering is exercised.
        assert!(order.contains(&(0, 0)) && order.contains(&(0, 2)), "{order:?}");
        assert!(order.iter().any(|(c, _)| *c > 0), "{order:?}");
        assert!(order.iter().any(|(_, k)| *k == 1), "{order:?}");
    }

    #[test]
    fn duplicates_across_scanners_are_merged() {
        let fs = FakeFs::new().dir(r"C:\Program Files\Unity Hub", 10);
        let src = sources(fs, FakeRegistry::new(), Vec::new());
        let report = scan_leftovers(&src, &hint());
        let wanted = norm(std::path::Path::new(r"C:\Program Files\Unity Hub"));
        let dirs: Vec<_> = report
            .items
            .iter()
            .filter(|l| matches!(&l.kind, LeftoverKind::Dir { path, .. } if norm(path) == wanted))
            .collect();
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0].confidence, Confidence::High);
    }

    #[test]
    fn shared_root_install_location_yields_no_dir_items_but_name_matches_elsewhere() {
        let fs = FakeFs::new()
            .dir(r"C:\Program Files\Unity Hub", 10)
            .dir(r"C:\Users\A\AppData\Roaming\UnityHub", 5);
        let src = sources(fs, FakeRegistry::new(), Vec::new());
        let h = ProgramHint::manual("Unity Hub", Some(PathBuf::from(r"C:\Program Files")));
        let report = scan_leftovers(&src, &h);
        let mut dirs: Vec<(String, Confidence)> = report
            .items
            .iter()
            .filter_map(|l| match &l.kind {
                LeftoverKind::Dir { path, .. } => Some((path.to_string_lossy().into_owned(), l.confidence)),
                _ => None,
            })
            .collect();
        dirs.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            dirs,
            vec![
                (r"C:\Program Files\Unity Hub".to_string(), Confidence::Medium),
                (r"C:\Users\A\AppData\Roaming\UnityHub".to_string(), Confidence::Medium),
            ]
        );
    }

    #[test]
    fn deleted_shortcut_is_not_listed() {
        let fs = FakeFs::new().dir(r"C:\Program Files\Unity Hub", 10).file(r"C:\Menu\Kept.lnk", 1);
        let target = r"C:\Program Files\Unity Hub\Unity Hub.exe";
        let src = sources(fs, FakeRegistry::new(), vec![lnk(target, r"C:\Menu\Kept.lnk"), lnk(target, r"C:\Menu\Deleted.lnk")]);
        let shortcuts: Vec<PathBuf> = scan_leftovers(&src, &hint())
            .items
            .into_iter()
            .filter_map(|l| match l.kind {
                LeftoverKind::Shortcut { lnk, .. } => Some(lnk),
                _ => None,
            })
            .collect();
        assert_eq!(shortcuts, vec![PathBuf::from(r"C:\Menu\Kept.lnk")]);
    }

    #[test]
    fn mark_safe_unchecks_low_only() {
        let report = scan_leftovers(&full_sources(), &hint());
        assert!(report.items.iter().any(|l| l.confidence == Confidence::Low), "{report:?}");
        let marks = mark_safe(&report);
        assert_eq!(marks.len(), report.items.len());
        for (m, l) in marks.iter().zip(&report.items) {
            assert_eq!(*m, l.confidence != Confidence::Low);
        }
    }

    #[test]
    fn manual_hint_without_dir_matches_by_name_only() {
        let report = scan_leftovers(&full_sources(), &ProgramHint::manual("Unity Hub", None));
        assert!(!report.items.is_empty());
        assert!(report.items.iter().all(|l| l.confidence != Confidence::High), "{report:?}");
        assert!(report.items.iter().all(|l| !matches!(l.kind, LeftoverKind::Shortcut { .. })));
    }
}
