//! Runs a full scan: programs, usage, then folder sizes, streamed over a channel.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use crate::devicepath::WinDeviceMap;
use crate::model::{Program, ProgramId};
use crate::offline_hives::{loaded_sids, profile_sids, LiveHiveOps, OfflineHives, OfflineSlot, SlotGuard};
use crate::pathutil::{exe_under, is_shared_root};
use crate::programs::enumerate_programs;
use crate::registry::{RegistryReader, WinRegistry};
use crate::services::service_exes;
use crate::fsys::FsReader;
use crate::shortcuts::{profile_start_menu_dirs, profile_startup_dirs, resolve_lnk, shortcut_targets, ShortcutTarget};
use crate::usage::bam::BamProvider;
use crate::usage::filetime::FileTimeProvider;
use crate::usage::matching::{
    arg_exe_names, is_name_excluded, match_usage, program_main_exes, usage_install_dir, MatchInput, ProgramUsage,
};
use crate::usage::prefetch::PrefetchProvider;
use crate::usage::userassist::UserAssistProvider;
use crate::usage::{UsageHit, UsageProvider};

/// Programs measured per `Sizes` event.
const SIZE_BATCH: usize = 50;

/// Emitted in this order: `Programs`, `Usage`, `UnmatchedHits`, zero or more `Sizes`, `Done`;
/// `Warning`s may appear in between.
#[derive(Debug)]
pub enum ScanEvent {
    Programs(Vec<Program>),
    Usage(HashMap<ProgramId, ProgramUsage>),
    /// Usage hits that belong to no installed program.
    UnmatchedHits(Vec<UsageHit>),
    /// Measured install-folder sizes in bytes, only for programs without a registry estimate.
    Sizes(HashMap<ProgramId, u64>),
    Warning(ScanWarning),
    Done,
}

/// Something the user should know about a scan; the GUI words it in the user's language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanWarning {
    /// `skipped` of the `total` Prefetch files could not be parsed; the rest were used.
    PrefetchUnreadable { skipped: usize, total: usize },
    /// The usage source `source` (e.g. `"BAM"`) produced nothing; `error` is the technical detail.
    SourceFailed { source: String, error: String },
    /// The scan thread could not be started; `error` is the technical detail.
    ThreadFailed { error: String },
}

impl std::fmt::Display for ScanWarning {
    /// English, for the log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScanWarning::PrefetchUnreadable { skipped, total } => {
                write!(f, "Prefetch: {skipped} of {total} files could not be read")
            }
            ScanWarning::SourceFailed { source, error } => write!(f, "{source}: {error}"),
            ScanWarning::ThreadFailed { error } => write!(f, "could not start the scan thread: {error}"),
        }
    }
}

/// Lists `*.exe` files directly in a folder, with sizes.
pub type ListRootExesFn = Box<dyn Fn(&Path) -> Vec<(PathBuf, u64)> + Send>;
/// File-time usage hits for the given exes.
pub type FileTimesFn = Box<dyn Fn(&[PathBuf]) -> Vec<UsageHit> + Send>;
/// Target exe (and arguments) of a `.lnk` shortcut, if it resolves to one.
pub type ResolveLnkFn = Box<dyn Fn(&Path) -> Option<ShortcutTarget> + Send>;
/// Files named exactly `name` (e.g. `Discord.exe`) under a folder, at most 3 levels deep.
pub type FindExesFn = Box<dyn Fn(&Path, &str) -> Vec<PathBuf> + Send>;

/// Loads the signed-out accounts' hive copies for the usage readers; `None` loads nothing.
pub type OfflineHivesFn = Box<dyn Fn() -> Option<SlotGuard> + Send>;

/// Total size of a folder, `None` when it cannot be measured.
pub type DirSizeFn = Box<dyn Fn(&Path) -> Option<u64> + Send>;

/// Everything a scan reads, injectable for tests.
pub struct ScanSources {
    pub reg: Arc<dyn RegistryReader>,
    pub providers: Vec<Box<dyn UsageProvider>>,
    pub shortcut_targets: Vec<ShortcutTarget>,
    pub list_root_exes: ListRootExesFn,
    /// Finds the exes named in a Squirrel-style shortcut's arguments.
    pub find_exes: FindExesFn,
    /// Size of an install folder; `None` when it is missing or unreadable.
    pub dir_size: DirSizeFn,
    /// File-time hits for the given main exes (run after main exes are known).
    pub file_times: FileTimesFn,
    /// `norm()`ed exe paths of installed services.
    pub service_exes: Box<dyn Fn() -> HashSet<String> + Send>,
    /// `norm()`ed exe paths of scheduled-task actions.
    pub task_exes: Box<dyn Fn() -> HashSet<String> + Send>,
    /// `norm()`ed targets of `Run`/`RunOnce` values and Startup-folder shortcuts.
    pub autostart_exes: Box<dyn Fn() -> HashSet<String> + Send>,
    /// Resolves `.lnk` hits (UserAssist records Start Menu launches by shortcut path).
    pub resolve_lnk: ResolveLnkFn,
    /// Makes the signed-out accounts' hives readable to the usage readers (UserAssist, Run values)
    /// until the guard drops; [`gather_usage`] holds it only while it gathers.
    pub offline_hives: OfflineHivesFn,
}

impl ScanSources {
    /// The real machine: registry, BAM, UserAssist, Prefetch, Start Menu shortcuts, file system.
    /// UserAssist and Run values cover every account (signed-out ones through their hive copies),
    /// shortcuts every profile's Start Menu.
    pub fn live() -> ScanSources {
        let reg: Arc<dyn RegistryReader> = Arc::new(WinRegistry);
        // Usage matching rejects other profiles' Temp folders as install folders.
        let fs = crate::fsys::RealFs::live();
        crate::leftovers::init_profile_roots(&fs);
        let profiles = fs.profile_dirs();
        // UserAssist and Run values also see the signed-out accounts' hives while usage is gathered.
        let slot = OfflineSlot::default();
        let usage_reg: Arc<dyn RegistryReader> = Arc::new(slot.reader(reg.clone()));
        let hives_reg = reg.clone();
        let system_root = crate::paths::windows_dir().to_string_lossy().into_owned();
        let services_reg = reg.clone();
        let autostart_reg = usage_reg.clone();
        ScanSources {
            providers: vec![
                Box::new(BamProvider { reg: reg.clone(), map: Arc::new(WinDeviceMap::new()) }),
                Box::new(UserAssistProvider { reg: usage_reg }),
                Box::new(PrefetchProvider { dir: PathBuf::from(system_root).join("Prefetch") }),
            ],
            reg,
            shortcut_targets: shortcut_targets(&profile_start_menu_dirs(&profiles)),
            list_root_exes: Box::new(crate::size::list_root_exes),
            find_exes: Box::new(crate::size::find_exes),
            dir_size: Box::new(crate::size::dir_size),
            file_times: Box::new(|exes: &[PathBuf]| {
                FileTimeProvider { exes: exes.to_vec() }.collect().unwrap_or_default()
            }),
            service_exes: Box::new(move || service_exes(&*services_reg)),
            task_exes: Box::new(|| crate::leftovers::other_scan::task_exes(&crate::fsys::RealFs::default(), &crate::paths::tasks_root())),
            autostart_exes: Box::new(move || {
                let mut exes = crate::leftovers::reg_scan::run_value_exes(&*autostart_reg);
                exes.extend(
                    shortcut_targets(&profile_startup_dirs(&profiles))
                        .iter()
                        .flat_map(|t| crate::usage::matching::autostart_targets(&t.path, &t.arguments)),
                );
                exes
            }),
            resolve_lnk: Box::new(resolve_lnk),
            offline_hives: Box::new(move || {
                let profiles = profile_sids(&*hives_reg, &crate::paths::windows_dir());
                let hives = OfflineHives::load(&profiles, &loaded_sids(&*hives_reg), &LiveHiveOps::new());
                Some(slot.fill(hives))
            }),
        }
    }
}

/// Install location worth looking into: present and not a shared root.
fn own_folder(p: &Program) -> Option<&Path> {
    p.install_location.as_deref().filter(|l| !is_shared_root(l))
}

/// Replaces each `.lnk` hit's path with the shortcut's target exe; unresolvable ones are dropped.
fn resolve_lnk_hits(hits: Vec<UsageHit>, resolve: &dyn Fn(&Path) -> Option<ShortcutTarget>) -> Vec<UsageHit> {
    hits.into_iter()
        .filter_map(|hit| {
            if !hit.exe_path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk")) {
                return Some(hit);
            }
            let exe_path = resolve(&hit.exe_path)?.path;
            Some(UsageHit { exe_path, ..hit })
        })
        .collect()
}

/// Exes under `install` named in the arguments of name-excluded shortcut targets there
/// (Squirrel's `Update.exe --processStart App.exe`).
fn find_arg_exes(install: &Path, targets: &[ShortcutTarget], find: &dyn Fn(&Path, &str) -> Vec<PathBuf>) -> Vec<PathBuf> {
    let mut names: Vec<String> = targets
        .iter()
        .filter(|t| exe_under(&t.path, install) && is_name_excluded(&t.path))
        .flat_map(|t| arg_exe_names(&t.arguments))
        .collect();
    names.sort();
    names.dedup();
    names.iter().flat_map(|n| find(install, n)).collect()
}

/// Everything usage matching needs besides the programs and shortcut targets.
pub struct UsageInputs {
    pub root_exes: HashMap<ProgramId, Vec<(PathBuf, u64)>>,
    pub arg_exes: HashMap<ProgramId, Vec<PathBuf>>,
    /// Provider hits (`.lnk` hits resolved) plus file-time hits for the main exes.
    pub hits: Vec<UsageHit>,
    pub service_exes: HashSet<String>,
    pub task_exes: HashSet<String>,
    pub autostart_exes: HashSet<String>,
    /// Provider failures and partial-result warnings, for the user.
    pub warnings: Vec<ScanWarning>,
}

impl UsageInputs {
    pub fn match_input<'a>(&'a self, programs: &'a [Program], shortcut_targets: &'a [ShortcutTarget]) -> MatchInput<'a> {
        MatchInput {
            programs,
            hits: &self.hits,
            shortcut_targets,
            root_exes: &self.root_exes,
            arg_exes: &self.arg_exes,
            service_exes: &self.service_exes,
            task_exes: &self.task_exes,
            autostart_exes: &self.autostart_exes,
        }
    }
}

/// Reads root exes and Squirrel argument exes (in each program's [`usage_install_dir`]), every
/// provider's hits and the service exes. The offline hives (`src.offline_hives`) are loaded
/// for the providers and autostart values and released before it returns.
pub fn gather_usage(src: &ScanSources, programs: &[Program]) -> UsageInputs {
    let dirs: Vec<(&ProgramId, PathBuf)> =
        programs.iter().filter_map(|p| Some((&p.id, usage_install_dir(p, &src.shortcut_targets)?))).collect();
    let root_exes: HashMap<ProgramId, Vec<(PathBuf, u64)>> =
        dirs.iter().map(|(id, dir)| ((*id).clone(), (src.list_root_exes)(dir))).collect();
    let arg_exes: HashMap<ProgramId, Vec<PathBuf>> = dirs
        .iter()
        .filter_map(|(id, dir)| {
            let found = find_arg_exes(dir, &src.shortcut_targets, &*src.find_exes);
            (!found.is_empty()).then(|| ((*id).clone(), found))
        })
        .collect();
    let (service_exes, task_exes) = ((src.service_exes)(), (src.task_exes)());
    // The same main exes matching uses, so a service never gets a file-time fallback.
    let mains: Vec<PathBuf> = programs
        .iter()
        .flat_map(|p| {
            let roots = root_exes.get(&p.id).map(Vec::as_slice).unwrap_or_default();
            let args = arg_exes.get(&p.id).map(Vec::as_slice).unwrap_or_default();
            program_main_exes(p, &src.shortcut_targets, roots, args, &service_exes).0
        })
        .collect();

    let offline = (src.offline_hives)();
    let (mut hits, mut warnings) = (Vec::new(), Vec::new());
    for provider in &src.providers {
        let (result, provider_warnings) = provider.collect_with_warnings();
        warnings.extend(provider_warnings);
        match result {
            Ok(h) => hits.extend(h),
            Err(error) => warnings.push(ScanWarning::SourceFailed { source: provider.name().to_string(), error }),
        }
    }
    let mut hits = resolve_lnk_hits(hits, &*src.resolve_lnk);
    hits.extend((src.file_times)(&mains));
    let autostart_exes = (src.autostart_exes)();
    // The copies are closed and deleted here, before matching and the scan's `Done`.
    drop(offline);
    UsageInputs { root_exes, arg_exes, hits, service_exes, task_exes, autostart_exes, warnings }
}

/// Blocking. Stops early when the receiver hangs up.
pub fn run_scan(src: ScanSources, tx: Sender<ScanEvent>) {
    let _ = scan(src, &tx);
}

/// `Err` means the receiver is gone.
fn scan(src: ScanSources, tx: &Sender<ScanEvent>) -> Result<(), std::sync::mpsc::SendError<ScanEvent>> {
    let programs = enumerate_programs(&*src.reg);
    tx.send(ScanEvent::Programs(programs.clone()))?;

    let inputs = gather_usage(&src, &programs);
    for w in &inputs.warnings {
        log::warn!("{w}");
        tx.send(ScanEvent::Warning(w.clone()))?;
    }
    let result = match_usage(&inputs.match_input(&programs, &src.shortcut_targets));
    tx.send(ScanEvent::Usage(result.by_program))?;
    tx.send(ScanEvent::UnmatchedHits(result.unmatched))?;

    // Spec §5: the registry estimate wins; only programs without one are measured, and only
    // in a folder of their own.
    let mut batch = HashMap::new();
    for p in programs.iter().filter(|p| !p.hidden && p.estimated_size_bytes.is_none()) {
        let Some(loc) = own_folder(p) else { continue };
        let Some(size) = (src.dir_size)(loc) else { continue };
        batch.insert(p.id.clone(), size);
        if batch.len() == SIZE_BATCH {
            tx.send(ScanEvent::Sizes(std::mem::take(&mut batch)))?;
        }
    }
    if !batch.is_empty() {
        tx.send(ScanEvent::Sizes(batch))?;
    }
    tx.send(ScanEvent::Done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Hive;
    use crate::registry::{key, FakeRegistry, RegValue};
    use crate::usage::{UsageHit, UsageSource};
    use chrono::{TimeZone, Utc};
    use std::sync::mpsc::channel;

    const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

    fn add_program(reg: &mut FakeRegistry, id: &str, install: &str, hidden: bool) {
        let k = key(Hive::LocalMachine, &format!(r"{UNINSTALL}\{id}"));
        reg.set(k.clone(), "DisplayName", RegValue::Str(id.into()));
        reg.set(k.clone(), "InstallLocation", RegValue::Str(install.into()));
        if hidden {
            reg.set(k, "SystemComponent", RegValue::Dword(1));
        }
    }

    struct FixedProvider(Result<Vec<UsageHit>, String>);

    /// Returns no hits and one warning.
    struct WarningProvider;

    impl UsageProvider for WarningProvider {
        fn name(&self) -> &'static str {
            "Warn"
        }
        fn collect(&self) -> Result<Vec<UsageHit>, String> {
            Ok(vec![])
        }
        fn collect_with_warnings(&self) -> (Result<Vec<UsageHit>, String>, Vec<ScanWarning>) {
            (Ok(vec![]), vec![ScanWarning::PrefetchUnreadable { skipped: 2, total: 9 }])
        }
    }

    impl UsageProvider for FixedProvider {
        fn name(&self) -> &'static str {
            "Fixed"
        }
        fn collect(&self) -> Result<Vec<UsageHit>, String> {
            self.0.clone()
        }
    }

    fn hit(path: &str) -> UsageHit {
        UsageHit {
            exe_path: PathBuf::from(path),
            last_run: Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap(),
            source: UsageSource::Bam,
            run_count: None,
        }
    }

    fn sources(provider: FixedProvider) -> ScanSources {
        let mut reg = FakeRegistry::new();
        add_program(&mut reg, "Alpha", r"D:\Apps\Alpha", false);
        add_program(&mut reg, "Beta", r"D:\Apps\Beta", false);
        add_program(&mut reg, "Hidden", r"D:\Apps\Hidden", true);
        ScanSources {
            reg: Arc::new(reg),
            providers: vec![Box::new(provider)],
            shortcut_targets: vec![],
            list_root_exes: Box::new(|dir: &Path| vec![(dir.join("main.exe"), 100)]),
            find_exes: Box::new(|_: &Path, _: &str| vec![]),
            dir_size: Box::new(|_: &Path| Some(1234)),
            file_times: Box::new(|_: &[PathBuf]| vec![]),
            service_exes: Box::new(HashSet::new),
            task_exes: Box::new(HashSet::new),
            autostart_exes: Box::new(HashSet::new),
            resolve_lnk: Box::new(|_: &Path| None),
            offline_hives: Box::new(|| None),
        }
    }

    const OFFLINE_SID: &str = "S-1-5-21-9-1002";
    const UA_COUNT: &str =
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\UserAssist\{CEBFF5CD-ACE2-4F4F-9178-9926F41749EA}\Count";

    /// The UserAssist value of five launches of `exe`, the last on 2026-01-02.
    fn userassist_value(hive: &mut FakeRegistry, exe: &str) {
        let mut data = vec![0u8; 72];
        data[4..8].copy_from_slice(&5u32.to_le_bytes());
        let ft = (Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap().timestamp() as u64 + 11_644_473_600) * 10_000_000;
        data[60..68].copy_from_slice(&ft.to_le_bytes());
        hive.set(key(Hive::Users, UA_COUNT), &crate::usage::userassist::rot13(exe), RegValue::Binary(data));
    }

    /// `src` with its usage readers going through an `OfflineSlot` that `offline_hives` fills with
    /// `hive` as the signed-out account [`OFFLINE_SID`]; the slot is returned too.
    fn with_offline_hive(mut src: ScanSources, hive: FakeRegistry) -> (ScanSources, crate::offline_hives::OfflineSlot) {
        use crate::offline_hives::{fake::FakeHiveOps, OfflineHives, OfflineSlot};
        let slot = OfflineSlot::default();
        let usage_reg: Arc<dyn RegistryReader> = Arc::new(slot.reader(src.reg.clone()));
        src.providers = vec![Box::new(UserAssistProvider { reg: usage_reg.clone() })];
        src.autostart_exes = Box::new(move || crate::leftovers::reg_scan::run_value_exes(&*usage_reg));
        let ops = std::sync::Mutex::new(Some(FakeHiveOps::default().with(r"C:\Users\b", hive)));
        let fill = slot.clone();
        src.offline_hives = Box::new(move || {
            let ops = ops.lock().unwrap().take()?;
            let profiles = [(OFFLINE_SID.to_string(), PathBuf::from(r"C:\Users\b"))];
            Some(fill.fill(OfflineHives::load(&profiles, &HashSet::new(), &ops)))
        });
        (src, slot)
    }

    #[test]
    fn overlay_reads_offline_userassist() {
        let mut hive = FakeRegistry::new();
        userassist_value(&mut hive, r"D:\Apps\Alpha\main.exe");
        let (src, slot) = with_offline_hive(sources(FixedProvider(Ok(vec![]))), hive);
        let programs = enumerate_programs(&*src.reg);
        let inputs = gather_usage(&src, &programs);
        assert!(!slot.is_filled(), "the hives are released before gather_usage returns");
        let hit = inputs.hits.iter().find(|h| h.exe_path == Path::new(r"D:\Apps\Alpha\main.exe")).expect("offline hit");
        assert_eq!((hit.source, hit.run_count), (UsageSource::UserAssist, Some(5)));
        let usage = match_usage(&inputs.match_input(&programs, &src.shortcut_targets)).by_program;
        let alpha = usage.keys().find(|id| id.0.ends_with("Alpha")).expect("Alpha has usage");
        assert_eq!(usage[alpha].last_used.source, Some(UsageSource::UserAssist));
    }

    #[test]
    fn run_values_of_offline_users_mark_autostart() {
        let mut hive = FakeRegistry::new();
        let run_key = key(Hive::Users, r"Software\Microsoft\Windows\CurrentVersion\Run");
        hive.set(run_key, "Beta", RegValue::Str(r#""D:\Apps\Beta\main.exe" --tray"#.into()));
        let (src, slot) = with_offline_hive(sources(FixedProvider(Ok(vec![]))), hive);
        let programs = enumerate_programs(&*src.reg);
        let inputs = gather_usage(&src, &programs);
        assert!(!slot.is_filled());
        let beta = crate::pathutil::norm(Path::new(r"D:\Apps\Beta\main.exe"));
        assert!(inputs.autostart_exes.contains(&beta), "{:?}", inputs.autostart_exes);
    }

    #[test]
    fn program_list_ignores_offline_hives() {
        let mut hive = FakeRegistry::new();
        let ghost = key(Hive::Users, r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Ghost");
        hive.set(ghost, "DisplayName", RegValue::Str("Ghost".into()));
        userassist_value(&mut hive, r"D:\Apps\Alpha\main.exe");
        let (src, slot) = with_offline_hive(sources(FixedProvider(Ok(vec![]))), hive);
        let events = run(src);
        assert!(!slot.is_filled());
        let ScanEvent::Programs(programs) = &events[0] else { unreachable!() };
        assert!(programs.iter().all(|p| p.name != "Ghost"), "{programs:?}");
        let ScanEvent::Usage(usage) = &events[1] else { unreachable!() };
        let alpha = usage.keys().find(|id| id.0.ends_with("Alpha")).expect("the overlay was in use for usage");
        assert_eq!(usage[alpha].last_used.source, Some(UsageSource::UserAssist));
    }

    #[test]
    fn start_menu_of_every_profile_feeds_main_exe_detection() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        let menu = b.join(r"AppData\Roaming\Microsoft\Windows\Start Menu\Programs");
        std::fs::create_dir_all(menu.join("Startup")).unwrap();
        std::fs::create_dir_all(&a).unwrap();
        crate::shortcuts::tests::write_lnk(&menu.join("Gamma.lnk"), Path::new(r"D:\Apps\Gamma\gamma.exe"));
        let dirs = crate::shortcuts::profile_start_menu_dirs(&[a.clone(), b.clone()]);
        assert!(dirs.contains(&menu), "{dirs:?}");
        assert!(!dirs.iter().any(|d| d.starts_with(&a)), "missing folders are left out: {dirs:?}");
        let startup = crate::shortcuts::profile_startup_dirs(&[a, b.clone(), b]);
        assert_eq!(startup.iter().filter(|d| d.starts_with(tmp.path())).collect::<Vec<_>>(), [&menu.join("Startup")]);

        let mut reg = FakeRegistry::new();
        add_program(&mut reg, "Gamma", r"D:\Apps\Gamma", false);
        let mut src = sources(FixedProvider(Ok(vec![hit(r"D:\Apps\Gamma\gamma.exe")])));
        src.reg = Arc::new(reg);
        src.list_root_exes = Box::new(|dir: &Path| vec![(dir.join("big.exe"), 900), (dir.join("gamma.exe"), 100)]);
        src.shortcut_targets = shortcut_targets(&dirs);
        let events = run(src);
        let ScanEvent::Usage(usage) = &events[1] else { unreachable!() };
        let gamma = usage.keys().find(|id| id.0.ends_with("Gamma")).unwrap();
        let main: Vec<_> = usage[gamma].exes.iter().filter(|e| e.is_main).map(|e| e.exe_path.clone()).collect();
        assert_eq!(main, [PathBuf::from(r"D:\Apps\Gamma\gamma.exe")]);
    }

    fn run(src: ScanSources) -> Vec<ScanEvent> {
        let (tx, rx) = channel();
        run_scan(src, tx);
        rx.into_iter().collect()
    }

    fn kind(e: &ScanEvent) -> &'static str {
        match e {
            ScanEvent::Programs(_) => "Programs",
            ScanEvent::Usage(_) => "Usage",
            ScanEvent::UnmatchedHits(_) => "UnmatchedHits",
            ScanEvent::Sizes(_) => "Sizes",
            ScanEvent::Warning(_) => "Warning",
            ScanEvent::Done => "Done",
        }
    }

    #[test]
    fn scan_emits_programs_then_usage_then_sizes_then_done() {
        let events = run(sources(FixedProvider(Ok(vec![hit(r"D:\Apps\Alpha\main.exe"), hit(r"E:\stray.exe")]))));
        let kinds: Vec<_> = events.iter().map(kind).collect();
        assert_eq!(kinds, ["Programs", "Usage", "UnmatchedHits", "Sizes", "Done"]);
        let ScanEvent::Usage(usage) = &events[1] else { unreachable!() };
        let alpha = usage.keys().find(|id| id.0.ends_with("Alpha")).expect("Alpha has usage");
        assert_eq!(usage[alpha].last_used.source, Some(UsageSource::Bam));
        let ScanEvent::UnmatchedHits(unmatched) = &events[2] else { unreachable!() };
        assert_eq!(unmatched, &vec![hit(r"E:\stray.exe")]);
        let ScanEvent::Sizes(sizes) = &events[3] else { unreachable!() };
        assert_eq!(sizes.len(), 2, "hidden program is not measured");
        assert!(sizes.values().all(|&s| s == 1234));
    }

    #[test]
    fn scan_passes_main_exes_to_file_times() {
        let mut src = sources(FixedProvider(Ok(vec![])));
        let (tx_exes, rx_exes) = channel();
        src.file_times = Box::new(move |exes: &[PathBuf]| {
            tx_exes.send(exes.to_vec()).unwrap();
            vec![UsageHit { source: UsageSource::FileTime, ..hit(r"D:\Apps\Beta\main.exe") }]
        });
        let events = run(src);
        let mut exes = rx_exes.recv().unwrap();
        exes.sort();
        assert!(exes.contains(&PathBuf::from(r"D:\Apps\Alpha\main.exe")));
        assert!(exes.contains(&PathBuf::from(r"D:\Apps\Beta\main.exe")));
        let ScanEvent::Usage(usage) = &events[1] else { unreachable!() };
        let beta = usage.keys().find(|id| id.0.ends_with("Beta")).unwrap();
        assert_eq!(usage[beta].last_used.source, Some(UsageSource::FileTime));
    }

    #[test]
    fn file_times_skip_service_main_exes_only() {
        let mut src = sources(FixedProvider(Ok(vec![])));
        src.list_root_exes = Box::new(|dir: &Path| vec![(dir.join("agent.exe"), 200), (dir.join("main.exe"), 100)]);
        // An autostart task's own main exe stays the main exe; a service never is.
        src.task_exes = Box::new(|| HashSet::from([crate::pathutil::norm(Path::new(r"D:\Apps\Alpha\agent.exe"))]));
        src.service_exes = Box::new(|| HashSet::from([crate::pathutil::norm(Path::new(r"D:\Apps\Beta\agent.exe"))]));
        let (tx_exes, rx_exes) = channel();
        src.file_times = Box::new(move |exes: &[PathBuf]| {
            tx_exes.send(exes.to_vec()).unwrap();
            vec![]
        });
        run(src);
        let exes = rx_exes.recv().unwrap();
        assert!(exes.contains(&PathBuf::from(r"D:\Apps\Alpha\agent.exe")), "{exes:?}");
        assert!(exes.contains(&PathBuf::from(r"D:\Apps\Beta\main.exe")), "{exes:?}");
        assert!(!exes.contains(&PathBuf::from(r"D:\Apps\Beta\agent.exe")), "{exes:?}");
    }

    #[test]
    fn lnk_hits_are_resolved_before_matching() {
        let mut reg = FakeRegistry::new();
        add_program(&mut reg, "Foo", r"C:\Apps\Foo", false);
        let lnk = UsageHit { source: UsageSource::UserAssist, run_count: Some(3), ..hit(r"C:\Users\X\Start Menu\Foo.lnk") };
        let gone = UsageHit { source: UsageSource::UserAssist, ..hit(r"C:\Users\X\Start Menu\Gone.LNK") };
        let mut src = sources(FixedProvider(Ok(vec![lnk.clone(), gone])));
        src.reg = Arc::new(reg);
        src.resolve_lnk =
            Box::new(|p: &Path| p.ends_with("Foo.lnk").then(|| ShortcutTarget::new(r"C:\Apps\Foo\foo.exe")));
        let events = run(src);
        let ScanEvent::Usage(usage) = &events[1] else { unreachable!() };
        let foo = usage.keys().find(|id| id.0.ends_with("Foo")).expect("Foo has usage");
        let exes = &usage[foo].exes;
        assert_eq!(exes.len(), 1);
        assert_eq!(exes[0].exe_path, PathBuf::from(r"C:\Apps\Foo\foo.exe"));
        assert_eq!((exes[0].last_run, exes[0].source, exes[0].run_count), (lnk.last_run, lnk.source, lnk.run_count));
        let ScanEvent::UnmatchedHits(unmatched) = &events[2] else { unreachable!() };
        assert!(unmatched.is_empty(), "unresolvable .lnk hit is dropped: {unmatched:?}");
    }

    #[test]
    fn squirrel_shortcut_arguments_pick_main_exe_via_find_exes() {
        let mut reg = FakeRegistry::new();
        add_program(&mut reg, "Discord", r"C:\Users\X\AppData\Local\Discord", false);
        let main = PathBuf::from(r"C:\Users\X\AppData\Local\Discord\app-1.0\Discord.exe");
        let mut src = sources(FixedProvider(Ok(vec![hit(main.to_str().unwrap())])));
        src.reg = Arc::new(reg);
        src.shortcut_targets = vec![ShortcutTarget {
            path: PathBuf::from(r"C:\Users\X\AppData\Local\Discord\Update.exe"),
            arguments: "--processStart Discord.exe".into(),
            lnk: PathBuf::new(),
        }];
        let (tx_find, rx_find) = channel();
        let found = main.clone();
        src.find_exes = Box::new(move |dir: &Path, name: &str| {
            tx_find.send((dir.to_path_buf(), name.to_string())).unwrap();
            vec![found.clone()]
        });
        let (tx_exes, rx_exes) = channel();
        src.file_times = Box::new(move |exes: &[PathBuf]| {
            tx_exes.send(exes.to_vec()).unwrap();
            vec![]
        });
        let events = run(src);
        let calls: Vec<_> = rx_find.try_iter().collect();
        assert_eq!(calls, [(PathBuf::from(r"C:\Users\X\AppData\Local\Discord"), "DISCORD.EXE".to_string())]);
        assert_eq!(rx_exes.recv().unwrap(), std::slice::from_ref(&main));
        let ScanEvent::Usage(usage) = &events[1] else { unreachable!() };
        let discord = usage.keys().find(|id| id.0.ends_with("Discord")).unwrap();
        assert_eq!(usage[discord].last_used.source, Some(UsageSource::Bam));
        assert!(usage[discord].exes[0].is_main);
    }

    /// Observed live (Steam): no InstallLocation, only an uninstaller in the install folder; the
    /// root exes must still be listed there so `steam.exe` becomes the main exe.
    #[test]
    fn root_exes_are_listed_in_the_uninstallers_folder_without_install_location() {
        let mut reg = FakeRegistry::new();
        let k = key(Hive::LocalMachine, &format!(r"{UNINSTALL}\Steam"));
        reg.set(k.clone(), "DisplayName", RegValue::Str("Steam".into()));
        reg.set(k, "UninstallString", RegValue::Str(r"D:\Apps\Steam\uninstall.exe".into()));
        let mut src = sources(FixedProvider(Ok(vec![hit(r"D:\Apps\Steam\main.exe")])));
        src.reg = Arc::new(reg);
        let events = run(src);
        let ScanEvent::Usage(usage) = &events[1] else { unreachable!() };
        let steam = usage.keys().find(|id| id.0.ends_with("Steam")).unwrap();
        assert_eq!(usage[steam].last_used.source, Some(UsageSource::Bam));
        assert!(usage[steam].exes[0].is_main, "{:?}", usage[steam].exes);
    }

    #[test]
    fn scan_continues_when_provider_fails() {
        let events = run(sources(FixedProvider(Err("boom".into()))));
        let failed = ScanWarning::SourceFailed { source: "Fixed".into(), error: "boom".into() };
        assert!(events.iter().any(|e| matches!(e, ScanEvent::Warning(w) if *w == failed)));
        assert!(matches!(events.last(), Some(ScanEvent::Done)));
    }

    #[test]
    fn provider_warnings_are_sent_before_usage() {
        let mut src = sources(FixedProvider(Ok(vec![])));
        src.providers.push(Box::new(WarningProvider));
        let events = run(src);
        let kinds: Vec<_> = events.iter().map(kind).collect();
        assert_eq!(kinds, ["Programs", "Warning", "Usage", "UnmatchedHits", "Sizes", "Done"]);
        assert!(matches!(&events[1], ScanEvent::Warning(ScanWarning::PrefetchUnreadable { skipped: 2, total: 9 })));
    }

    #[test]
    fn scan_stops_when_receiver_hangs_up() {
        let (tx, rx) = channel();
        drop(rx);
        run_scan(sources(FixedProvider(Ok(vec![]))), tx); // must return, not panic
    }

    #[test]
    fn programs_with_estimated_size_are_not_measured() {
        let mut reg = FakeRegistry::new();
        add_program(&mut reg, "Estimated", r"D:\Apps\Estimated", false);
        let k = key(Hive::LocalMachine, &format!(r"{UNINSTALL}\Estimated"));
        reg.set(k, "EstimatedSize", RegValue::Dword(2048));
        add_program(&mut reg, "Measured", r"D:\Apps\Measured", false);
        add_program(&mut reg, "Gone", r"D:\Apps\Gone", false);
        let mut src = sources(FixedProvider(Ok(vec![])));
        src.reg = Arc::new(reg);
        let (tx_calls, rx_calls) = channel();
        src.dir_size = Box::new(move |dir: &Path| {
            tx_calls.send(dir.to_path_buf()).unwrap();
            (!dir.ends_with("Gone")).then_some(77)
        });
        let events = run(src);
        let mut calls: Vec<PathBuf> = rx_calls.try_iter().collect();
        calls.sort();
        assert_eq!(calls, [PathBuf::from(r"D:\Apps\Gone"), PathBuf::from(r"D:\Apps\Measured")]);
        let sizes: HashMap<ProgramId, u64> = events
            .into_iter()
            .filter_map(|e| match e {
                ScanEvent::Sizes(s) => Some(s),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(sizes.len(), 1, "only Some results are sent: {sizes:?}");
        assert!(sizes.iter().all(|(id, &s)| id.0.ends_with("Measured") && s == 77));
    }

    #[test]
    fn sizes_are_sent_in_batches_of_at_most_fifty() {
        let mut reg = FakeRegistry::new();
        for i in 0..120 {
            add_program(&mut reg, &format!("P{i:03}"), &format!(r"D:\Apps\P{i:03}"), false);
        }
        let mut src = sources(FixedProvider(Ok(vec![])));
        src.reg = Arc::new(reg);
        let batches: Vec<usize> = run(src)
            .into_iter()
            .filter_map(|e| match e {
                ScanEvent::Sizes(s) => Some(s.len()),
                _ => None,
            })
            .collect();
        assert_eq!(batches, [50, 50, 20]);
    }
}
