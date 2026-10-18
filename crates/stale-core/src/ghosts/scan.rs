//! The ghost scan: every collector over the live (or fake) sources, then grouping.

use super::folders::orphan_folders;
use super::refs::{service_evidence, shortcut_evidence, task_evidence, usage_evidence};
use super::registry::registry_evidence;
use super::{group, Evidence, Ghost, InstalledIndex, NameRoots};
use crate::leftovers::LeftoverSources;
use crate::model::Program;
use crate::shortcuts::ShortcutTarget;
use crate::usage::UsageHit;
use std::path::{Path, PathBuf};

/// Everything the ghost collectors read from, so tests can substitute fakes.
pub struct GhostSources {
    /// File system, registry, scan roots, shortcut targets and keep list.
    pub leftovers: LeftoverSources,
    /// Every installed program, hidden ones included.
    pub programs: Vec<Program>,
    /// Execution traces the usage scan matched to no installed program.
    pub unmatched_hits: Vec<UsageHit>,
    pub drive_present: Box<dyn Fn(char) -> bool + Send + Sync>,
}

impl GhostSources {
    /// The real machine. Which drives are present is checked once per letter, here.
    pub fn live(programs: Vec<Program>, unmatched_hits: Vec<UsageHit>) -> GhostSources {
        let mut present = [false; 26];
        for (i, d) in ('A'..='Z').enumerate() {
            present[i] = Path::new(&format!(r"{d}:\")).exists();
        }
        let drive_present = move |d: char| {
            let d = d.to_ascii_uppercase();
            d.is_ascii_uppercase() && present[(d as u8 - b'A') as usize]
        };
        GhostSources { leftovers: LeftoverSources::live(), programs, unmatched_hits, drive_present: Box::new(drive_present) }
    }
}

/// Runs every collector (orphan folders, registry, services, scheduled tasks, Start Menu and
/// desktop shortcuts, execution traces) against `src` and groups the evidence into ghosts.
pub fn scan_ghosts(src: &GhostSources) -> Vec<Ghost> {
    let l = &src.leftovers;
    let (fs, reg, keep) = (&*l.fs, &*l.reg, &l.keep);
    let drive_present: &dyn Fn(char) -> bool = &*src.drive_present;
    let installed = InstalledIndex::from_programs(&src.programs);
    let shortcuts: Vec<ShortcutTarget> = l.start_menu_targets.iter().chain(&l.desktop_targets).cloned().collect();
    let evidence: Vec<Evidence> = orphan_folders(fs, &l.fs_roots, &installed, keep)
        .into_iter()
        .chain(registry_evidence(reg, fs, &l.reg_roots, &installed, keep, drive_present))
        .chain(service_evidence(reg, fs, &installed, keep, drive_present))
        .chain(task_evidence(fs, &l.tasks_root, &installed, keep, drive_present))
        .chain(shortcut_evidence(fs, &shortcuts, &installed, keep, drive_present))
        .chain(usage_evidence(fs, &src.unmatched_hits, &installed, keep, drive_present))
        .collect();
    let roots = NameRoots::new(&l.fs_roots, &fs.profile_dirs(), &src.programs);
    let current_profile = fs.env("USERPROFILE").map(PathBuf::from);
    group(evidence, &installed, keep, &roots, current_profile.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsys::{FakeFs, FsReader};
    use crate::ghosts::{Assessment, Evidence, RefKind};
    use crate::keep::KeepList;
    use crate::leftovers::fs_scan::FsScanRoots;
    use crate::leftovers::reg_scan::RegScanRoots;
    use crate::model::Hive;
    use crate::registry::{key, FakeRegistry, RegValue, RegistryReader};
    use crate::usage::UsageSource;
    use chrono::{TimeZone, Utc};
    use std::path::PathBuf;
    use std::sync::Arc;

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    #[test]
    fn scan_ghosts_end_to_end_with_fakes() {
        let fs = FakeFs::new()
            .env("ProgramFiles", r"C:\Program Files")
            .env("ProgramData", r"C:\ProgramData")
            .env("USERPROFILE", r"C:\Users\A")
            .file(r"C:\Program Files\Old Tool\data.bin", 30)
            .profile(r"C:\Users\A");
        let mut reg = FakeRegistry::new();
        reg.set(key(Hive::CurrentUser, RUN), "OldTool", RegValue::Str(r#""C:\Program Files\Old Tool\old.exe" --min"#.into()));
        let fs: Arc<dyn FsReader> = Arc::new(fs);
        let reg: Arc<dyn RegistryReader> = Arc::new(reg);
        let last_run = Utc.with_ymd_and_hms(2025, 3, 4, 0, 0, 0).unwrap();
        let src = GhostSources {
            leftovers: LeftoverSources {
                fs_roots: FsScanRoots::from_env(&*fs),
                reg_roots: RegScanRoots { software: vec![], classes: vec![], run: vec![key(Hive::CurrentUser, RUN)], app_paths: vec![] },
                tasks_root: PathBuf::from(r"C:\Windows\System32\Tasks"),
                start_menu_targets: Vec::new(),
                desktop_targets: Vec::new(),
                keep: KeepList::default(),
                reg,
                fs,
            },
            programs: Vec::new(),
            unmatched_hits: vec![UsageHit {
                exe_path: r"C:\Program Files\Old Tool\old.exe".into(),
                last_run,
                source: UsageSource::Prefetch,
                run_count: None,
            }],
            drive_present: Box::new(|d| d == 'C'),
        };
        let ghosts = scan_ghosts(&src);
        assert_eq!(ghosts.len(), 1, "{ghosts:?}");
        let g = &ghosts[0];
        assert_eq!((g.name.as_str(), g.assessment), ("Old Tool", Assessment::ProbablyUninstalled));
        assert_eq!((g.total_size, g.last_used), (30, Some(last_run)));
        assert!(g.evidence.iter().any(|e| matches!(e, Evidence::Folder { .. })), "{g:?}");
        assert!(g.evidence.iter().any(|e| matches!(e, Evidence::DanglingRef { kind: RefKind::Run, .. })), "{g:?}");
        assert!(g.evidence.iter().any(|e| matches!(e, Evidence::UsageTrace { .. })), "{g:?}");
    }
}
