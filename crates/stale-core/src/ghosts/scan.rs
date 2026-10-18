//! The ghost scan: every collector over the live (or fake) sources, then grouping.

use super::folders::orphan_folders;
use super::refs::{service_collect, shortcut_collect, task_collect, usage_collect};
use super::registry::registry_collect;
use super::{ghost_name_of, group, Collected, Evidence, Ghost, InstalledIndex, LiveRefs, NameRoots};
use crate::leftovers::LeftoverSources;
use crate::model::{Program, ProgramId};
use std::collections::HashSet;
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

/// The cached program list joined with a fresh enumeration, by [`ProgramId`]: every fresh
/// program (its current data wins), then the cached ones the fresh list lacks. A program
/// installed since the last scan is then never mistaken for a ghost.
pub fn merge_programs(cached: Vec<Program>, fresh: Vec<Program>) -> Vec<Program> {
    let ids: HashSet<ProgramId> = fresh.iter().map(|p| p.id.clone()).collect();
    let mut out = fresh;
    out.extend(cached.into_iter().filter(|p| !ids.contains(&p.id)));
    out
}

/// Runs every collector (orphan folders, registry, services, scheduled tasks, Start Menu and
/// desktop shortcuts, execution traces) against `src` and groups the evidence into ghosts. The
/// existing targets the reference collectors saw are the live references: a folder holding one
/// is in use and never more than Uncertain.
pub fn scan_ghosts(src: &GhostSources) -> Vec<Ghost> {
    let l = &src.leftovers;
    let (fs, reg, keep) = (&*l.fs, &*l.reg, &l.keep);
    let drive_present: &dyn Fn(char) -> bool = &*src.drive_present;
    let installed = InstalledIndex::from_programs(&src.programs);
    let shortcuts: Vec<ShortcutTarget> = l.start_menu_targets.iter().chain(&l.desktop_targets).cloned().collect();
    let roots = NameRoots::new(&l.fs_roots, &fs.profile_dirs(), &src.programs);
    let mut evidence: Vec<Evidence> = orphan_folders(fs, &l.fs_roots, &installed, keep);
    let folder_names: Vec<String> = evidence.iter().filter_map(|e| ghost_name_of(e, &roots)).collect();
    let collected: Vec<Collected> = vec![
        registry_collect(reg, fs, &l.reg_roots, &installed, keep, drive_present, &folder_names),
        service_collect(reg, fs, &installed, keep, drive_present),
        task_collect(fs, &l.tasks_root, &installed, keep, drive_present),
        shortcut_collect(fs, &shortcuts, &installed, keep, drive_present),
        usage_collect(fs, &src.unmatched_hits, &installed, keep, drive_present),
    ];
    let mut live_targets = Vec::new();
    for c in collected {
        evidence.extend(c.evidence);
        live_targets.extend(c.live_targets);
    }
    let live = LiveRefs::from_targets(live_targets);
    let current_profile = fs.env("USERPROFILE").map(PathBuf::from);
    group(evidence, &installed, keep, &roots, current_profile.as_deref(), &live)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsys::{FakeFs, FsReader};
    use crate::ghosts::group::REASON_IN_USE;
    use crate::ghosts::{to_leftovers, Assessment, Evidence, RefKind};
    use crate::leftovers::Confidence;
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

    /// Sources over `fs` and `reg` with the standard Program Files / ProgramData roots, no
    /// programs and no execution traces.
    fn sources(fs: FakeFs, reg: FakeRegistry) -> GhostSources {
        let fs: Arc<dyn FsReader> = Arc::new(fs);
        let reg: Arc<dyn RegistryReader> = Arc::new(reg);
        GhostSources {
            leftovers: LeftoverSources {
                fs_roots: FsScanRoots::from_env(&*fs),
                reg_roots: RegScanRoots { software: vec![], classes: vec![], run: vec![], app_paths: vec![] },
                tasks_root: PathBuf::from(r"C:\Windows\System32\Tasks"),
                start_menu_targets: Vec::new(),
                desktop_targets: Vec::new(),
                keep: KeepList::default(),
                reg,
                fs,
            },
            programs: Vec::new(),
            unmatched_hits: Vec::new(),
            drive_present: Box::new(|d| d == 'C'),
        }
    }

    fn base_fs() -> FakeFs {
        FakeFs::new()
            .env("ProgramFiles", r"C:\Program Files")
            .env("ProgramFiles(x86)", r"C:\Program Files (x86)")
            .env("ProgramData", r"C:\ProgramData")
            .env("USERPROFILE", r"C:\Users\A")
            .dir(r"C:\Program Files", 0)
            .dir(r"C:\Program Files (x86)", 0)
            .profile(r"C:\Users\A")
    }

    fn service(reg: &mut FakeRegistry, name: &str, image: &str, service_type: u32) {
        let k = key(Hive::LocalMachine, &format!(r"SYSTEM\CurrentControlSet\Services\{name}"));
        reg.set(k.clone(), "ImagePath", RegValue::ExpandStr(image.into()));
        reg.set(k, "Type", RegValue::Dword(service_type));
    }

    fn by_name<'a>(ghosts: &'a [Ghost], name: &str) -> &'a Ghost {
        ghosts.iter().find(|g| g.name == name).unwrap_or_else(|| panic!("no {name} in {ghosts:?}"))
    }

    fn program(id: &str, name: &str) -> Program {
        Program {
            account: None,
            id: crate::model::ProgramId(id.into()),
            key: key(Hive::LocalMachine, &format!(r"Software\{id}")),
            name: name.into(),
            publisher: None,
            version: None,
            install_location: None,
            install_date: None,
            uninstall_string: None,
            quiet_uninstall_string: None,
            display_icon: None,
            is_msi: false,
            estimated_size_bytes: None,
            drive: None,
            hidden: false,
            scope: crate::model::Scope::Machine,
        }
    }

    #[test]
    fn merge_programs_unions_by_id_fresh_first() {
        let cached = vec![program("a", "Old A"), program("b", "B")];
        let fresh = vec![program("a", "New A"), program("c", "C")];
        let names: Vec<String> = merge_programs(cached, fresh).into_iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["New A", "C", "B"]);
        let merged = merge_programs(vec![program("x", "X")], Vec::new());
        // The merged list goes straight into the live sources.
        let src = GhostSources { programs: merged, ..sources(base_fs(), FakeRegistry::new()) };
        assert_eq!(src.programs.len(), 1);
    }

    #[test]
    fn vendor_key_without_path_joins_folder_ghost() {
        let fs = base_fs().file(r"C:\Program Files\Old Tool\data.bin", 5);
        let mut reg = FakeRegistry::new();
        let hkcu = |p: &str| key(Hive::CurrentUser, p);
        reg.set(hkcu(r"Software\OldTool"), "Theme", RegValue::Str("dark".into()));
        reg.set(hkcu(r"Software\Unrelated Vendor"), "Theme", RegValue::Str("dark".into()));
        let mut src = sources(fs, reg);
        src.leftovers.reg_roots.software = vec![hkcu("Software")];
        let ghosts = scan_ghosts(&src);
        assert_eq!(ghosts.len(), 1, "{ghosts:?}");
        let g = &ghosts[0];
        assert_eq!((g.name.as_str(), g.assessment), ("Old Tool", Assessment::LeftoversOnly));
        assert!(g.evidence.contains(&Evidence::RegKeyOrphan { key: hkcu(r"Software\OldTool") }), "{g:?}");
        let key_item = to_leftovers(g, Path::new(r"C:\T"))
            .items
            .into_iter()
            .find(|l| matches!(l.kind, crate::leftovers::LeftoverKind::RegKey { .. }))
            .unwrap();
        assert_eq!(key_item.confidence, Confidence::Low, "a vendor key is Low unless the ghost is probably uninstalled");
        let mut sure = g.clone();
        sure.assessment = Assessment::ProbablyUninstalled;
        let key_item = to_leftovers(&sure, Path::new(r"C:\T"))
            .items
            .into_iter()
            .find(|l| matches!(l.kind, crate::leftovers::LeftoverKind::RegKey { .. }))
            .unwrap();
        assert_eq!(key_item.confidence, Confidence::Medium);
    }

    /// Regression: a running Win32 service's own folder was a pre-checked LeftoversOnly ghost.
    #[test]
    fn running_service_folder_is_in_use_like_lighting_service() {
        let fs = base_fs().file(r"C:\Program Files (x86)\LightingService\LightingService.exe", 13);
        let mut reg = FakeRegistry::new();
        service(&mut reg, "LightingService", r#""C:\Program Files (x86)\LightingService\LightingService.exe""#, 16);
        let ghosts = scan_ghosts(&sources(fs, reg));
        let g = by_name(&ghosts, "LightingService");
        assert_eq!((g.assessment, g.reasons.clone()), (Assessment::Uncertain, vec![REASON_IN_USE.to_string()]));
        assert!(to_leftovers(g, Path::new(r"C:\T")).items.iter().all(|l| l.confidence == Confidence::Low));
    }

    /// Regression: a running kernel driver's folder (`\??\` ImagePath, `Type` 1) was a ghost.
    #[test]
    fn kernel_driver_folder_is_in_use_like_ldplayer9box() {
        let fs = base_fs().file(r"C:\Program Files\ldplayer9box\Ld9BoxSup.sys", 144);
        let mut reg = FakeRegistry::new();
        service(&mut reg, "Ld9BoxSup", r"\??\C:\Program Files\ldplayer9box\Ld9BoxSup.sys", 1);
        // A kernel driver whose file is gone is not evidence either.
        service(&mut reg, "GoneDrv", r"\??\C:\Program Files\Gone Driver Co\gone.sys", 1);
        let ghosts = scan_ghosts(&sources(fs, reg));
        assert_eq!(ghosts.len(), 1, "{ghosts:?}");
        let g = by_name(&ghosts, "ldplayer9box");
        assert_eq!((g.assessment, g.reasons.first().map(String::as_str)), (Assessment::Uncertain, Some(REASON_IN_USE)));
        assert!(!g.evidence.iter().any(|e| matches!(e, Evidence::DanglingRef { .. })), "{g:?}");
    }

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
