//! Runs a full scan: programs, usage, then folder sizes, streamed over a channel.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use crate::devicepath::WinDeviceMap;
use crate::model::{Program, ProgramId};
use crate::pathutil::{exe_under, is_shared_root};
use crate::programs::enumerate_programs;
use crate::registry::{RegistryReader, WinRegistry};
use crate::services::service_exes;
use crate::shortcuts::{resolve_lnk, shortcut_targets, start_menu_dirs, ShortcutTarget};
use crate::usage::bam::BamProvider;
use crate::usage::filetime::FileTimeProvider;
use crate::usage::matching::{arg_exe_names, is_name_excluded, main_exes, match_usage, MatchInput, ProgramUsage};
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
    Warning(String),
    Done,
}

/// Lists `*.exe` files directly in a folder, with sizes.
pub type ListRootExesFn = Box<dyn Fn(&Path) -> Vec<(PathBuf, u64)> + Send>;
/// File-time usage hits for the given exes.
pub type FileTimesFn = Box<dyn Fn(&[PathBuf]) -> Vec<UsageHit> + Send>;
/// Target exe (and arguments) of a `.lnk` shortcut, if it resolves to one.
pub type ResolveLnkFn = Box<dyn Fn(&Path) -> Option<ShortcutTarget> + Send>;
/// Files named exactly `name` (e.g. `Discord.exe`) under a folder, at most 3 levels deep.
pub type FindExesFn = Box<dyn Fn(&Path, &str) -> Vec<PathBuf> + Send>;

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
    /// Resolves `.lnk` hits (UserAssist records Start Menu launches by shortcut path).
    pub resolve_lnk: ResolveLnkFn,
}

impl ScanSources {
    /// The real machine: registry, BAM, UserAssist, Prefetch, Start Menu shortcuts, file system.
    pub fn live() -> ScanSources {
        let reg: Arc<dyn RegistryReader> = Arc::new(WinRegistry);
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        let services_reg = reg.clone();
        ScanSources {
            providers: vec![
                Box::new(BamProvider { reg: reg.clone(), map: Arc::new(WinDeviceMap::new()) }),
                Box::new(UserAssistProvider { reg: reg.clone() }),
                Box::new(PrefetchProvider { dir: PathBuf::from(system_root).join("Prefetch") }),
            ],
            reg,
            shortcut_targets: shortcut_targets(&start_menu_dirs()),
            list_root_exes: Box::new(crate::size::list_root_exes),
            find_exes: Box::new(crate::size::find_exes),
            dir_size: Box::new(crate::size::dir_size),
            file_times: Box::new(|exes: &[PathBuf]| {
                FileTimeProvider { exes: exes.to_vec() }.collect().unwrap_or_default()
            }),
            service_exes: Box::new(move || service_exes(&*services_reg)),
            resolve_lnk: Box::new(resolve_lnk),
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

/// Blocking. Stops early when the receiver hangs up.
pub fn run_scan(src: ScanSources, tx: Sender<ScanEvent>) {
    let _ = scan(src, &tx);
}

/// `Err` means the receiver is gone.
fn scan(src: ScanSources, tx: &Sender<ScanEvent>) -> Result<(), std::sync::mpsc::SendError<ScanEvent>> {
    let programs = enumerate_programs(&*src.reg);
    tx.send(ScanEvent::Programs(programs.clone()))?;

    let root_exes: HashMap<ProgramId, Vec<(PathBuf, u64)>> = programs
        .iter()
        .filter_map(|p| Some((p.id.clone(), (src.list_root_exes)(own_folder(p)?))))
        .collect();
    let arg_exes: HashMap<ProgramId, Vec<PathBuf>> = programs
        .iter()
        .filter_map(|p| {
            let found = find_arg_exes(own_folder(p)?, &src.shortcut_targets, &*src.find_exes);
            (!found.is_empty()).then(|| (p.id.clone(), found))
        })
        .collect();
    let mains: Vec<PathBuf> = programs
        .iter()
        .flat_map(|p| {
            let roots = root_exes.get(&p.id).map(Vec::as_slice).unwrap_or_default();
            let args = arg_exes.get(&p.id).map(Vec::as_slice).unwrap_or_default();
            main_exes(p, &src.shortcut_targets, roots, args)
        })
        .collect();

    let mut hits = Vec::new();
    for provider in &src.providers {
        let (result, warnings) = provider.collect_with_warnings();
        for w in warnings {
            log::warn!("{w}");
            tx.send(ScanEvent::Warning(w))?;
        }
        match result {
            Ok(h) => hits.extend(h),
            Err(e) => {
                let msg = format!("{}: {}", provider.name(), e);
                log::warn!("{msg}");
                tx.send(ScanEvent::Warning(msg))?;
            }
        }
    }
    let mut hits = resolve_lnk_hits(hits, &*src.resolve_lnk);
    hits.extend((src.file_times)(&mains));

    let services = (src.service_exes)();
    let result = match_usage(&MatchInput {
        programs: &programs,
        hits: &hits,
        shortcut_targets: &src.shortcut_targets,
        root_exes: &root_exes,
        arg_exes: &arg_exes,
        service_exes: &services,
    });
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
        fn collect_with_warnings(&self) -> (Result<Vec<UsageHit>, String>, Vec<String>) {
            (Ok(vec![]), vec!["Prefetch: 2 av 9 filer kunde inte läsas".to_string()])
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
            resolve_lnk: Box::new(|_: &Path| None),
        }
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

    #[test]
    fn scan_continues_when_provider_fails() {
        let events = run(sources(FixedProvider(Err("boom".into()))));
        assert!(events.iter().any(|e| matches!(e, ScanEvent::Warning(w) if w.contains("boom") && w.contains("Fixed"))));
        assert!(matches!(events.last(), Some(ScanEvent::Done)));
    }

    #[test]
    fn provider_warnings_are_sent_before_usage() {
        let mut src = sources(FixedProvider(Ok(vec![])));
        src.providers.push(Box::new(WarningProvider));
        let events = run(src);
        let kinds: Vec<_> = events.iter().map(kind).collect();
        assert_eq!(kinds, ["Programs", "Warning", "Usage", "UnmatchedHits", "Sizes", "Done"]);
        assert!(matches!(&events[1], ScanEvent::Warning(w) if w == "Prefetch: 2 av 9 filer kunde inte läsas"));
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
