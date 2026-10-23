//! Runs the scan, the uninstall queue and quarantine work on background threads.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use stale_core::fsys::{FsReader, RealFs};
use stale_core::ghosts::{merge_programs, scan_ghosts, Ghost, GhostSources, InstalledIndex};
use stale_core::keep::KeepList;
use stale_core::leftovers::fs_scan::FsScanRoots;
use stale_core::own::OwnPaths;
use stale_core::space::{children, fixed_drives, list_entries, measure, Child, SpaceEntry, BROWSE_LIMIT};
use stale_core::leftovers::{scan_leftovers, Leftover, LeftoverReport, LeftoverSources, ProgramHint};
use stale_core::quarantine::{
    list_trusted_manifests, purge, quarantine, restore, QuarantineId, QuarantineManifest, QuarantineOps,
    QuarantineProgress, RestoreOutcome,
};
use stale_core::model::{Program, ProgramId};
use stale_core::provenance::{provenance, InstallSource, Provenance, Signature, WinProbe};
use stale_core::registry::WinRegistry;
use stale_core::scan::{run_scan, ScanEvent, ScanSources, ScanWarning};
use stale_core::uninstall::{run_queue, QueueItem, QueueOptions, UninstallEvent, UninstallSources};
use stale_core::usage::UsageHit;
use windows::Win32::Foundation::RPC_E_TOO_LATE;
use windows::Win32::System::Com::{
    CoInitializeEx, CoInitializeSecurity, CoUninitialize, COINIT_APARTMENTTHREADED, EOAC_NONE,
    RPC_C_AUTHN_LEVEL_DEFAULT, RPC_C_IMP_LEVEL_IMPERSONATE,
};

use crate::leftover_sel::LeftoverSelection;
use crate::space_sel::removal_selection_of;
use crate::warning::{Msg, Warning};

/// Starts a scan on the "stale-scan" thread; events arrive on the returned channel.
/// `data_dir_secured`: the data folder was locked down at start, so signed-out accounts' hives
/// may be copied into it ([`ScanSources::live`]).
pub fn spawn_scan(data_dir_secured: bool) -> Receiver<ScanEvent> {
    let (tx, rx) = channel();
    let fallback = tx.clone();
    let spawned = std::thread::Builder::new()
        .name("stale-scan".into())
        .spawn(move || run_scan(ScanSources::live(data_dir_secured), tx));
    if let Err(e) = spawned {
        log::error!("could not spawn scan thread: {e}");
        let _ = fallback.send(ScanEvent::Warning(ScanWarning::ThreadFailed { error: e.to_string() }));
        let _ = fallback.send(ScanEvent::Done);
    }
    rx
}

/// Runs the uninstall queue on the "stale-uninstall" thread; events arrive on the returned
/// channel. Setting the returned flag stops waiting for an uninstaller's child processes.
pub fn spawn_uninstall(items: Vec<QueueItem>, opts: QueueOptions) -> (Receiver<UninstallEvent>, Arc<AtomicBool>) {
    let (tx, rx) = channel();
    let fallback = tx.clone();
    let stop_waiting = Arc::new(AtomicBool::new(false));
    let flag = stop_waiting.clone();
    let spawned = std::thread::Builder::new().name("stale-uninstall".into()).spawn(move || {
        let _com = ComGuard::init();
        // Built here, not on the UI thread: it resolves every Start Menu shortcut.
        let src = UninstallSources::live_with_stop_flag(flag);
        run_queue(&src, &items, &opts, &tx);
    });
    if let Err(e) = spawned {
        log::error!("could not spawn uninstall thread: {e}");
        // `flow` words it as "could not start the uninstall: <e>".
        let _ = fallback.send(UninstallEvent::Warning(e.to_string()));
        let _ = fallback.send(UninstallEvent::QueueDone);
    }
    (rx, stop_waiting)
}

/// Progress of moving checked leftovers to quarantine.
#[derive(Clone, Debug)]
pub enum QuarantineEvent {
    /// About to handle item `done` (0-based) of `total`, counted over all programs.
    Progress { done: usize, total: usize, what: String },
    /// One manifest per program, in batch order.
    Finished(Vec<QuarantineManifest>),
    Failed(Warning),
}

/// Quarantines each program's leftovers in turn on the "stale-quarantine" thread.
pub fn spawn_quarantine(batches: Vec<(String, Vec<Leftover>)>) -> Receiver<QuarantineEvent> {
    let (tx, rx) = channel();
    let fallback = tx.clone();
    let spawned = std::thread::Builder::new().name("stale-quarantine".into()).spawn(move || {
        let total: usize = batches.iter().map(|(_, items)| items.len()).sum();
        let run = catch_unwind(AssertUnwindSafe(|| {
            let ops = QuarantineOps::live();
            let mut manifests = Vec::with_capacity(batches.len());
            let mut offset = 0;
            for (name, items) in &batches {
                let progress = |p: QuarantineProgress| {
                    if let QuarantineProgress::Item { done, what, .. } = p {
                        let _ = tx.send(QuarantineEvent::Progress { done: offset + done, total, what });
                    }
                };
                manifests.push(quarantine(&ops, name, items, Utc::now(), &progress));
                offset += items.len();
            }
            manifests
        }));
        let ev = match run {
            Ok(manifests) => QuarantineEvent::Finished(manifests),
            // The panic hook has logged the details.
            Err(_) => QuarantineEvent::Failed(Warning::Fixed(Msg::QuarantineAborted)),
        };
        let _ = tx.send(ev);
    });
    if let Err(e) = spawned {
        log::error!("could not spawn quarantine thread: {e}");
        let _ = fallback.send(QuarantineEvent::Failed(Warning::Detail(Msg::QuarantineThreadFailed, e.to_string())));
    }
    rx
}

/// Results of listing, restoring and purging quarantines.
#[derive(Clone, Debug)]
pub enum QuarantineAdminEvent {
    /// Every trusted quarantine, newest first.
    Listed(Vec<QuarantineManifest>),
    Restored { id: QuarantineId, outcome: RestoreOutcome },
    Purged(QuarantineId),
    /// Sent after the last `Purged` of a purge.
    PurgedAll,
    Failed(Warning),
}

/// Runs `job` with live quarantine ops on the "stale-quarantine-admin" thread.
fn spawn_quarantine_admin<F>(job: F) -> Receiver<QuarantineAdminEvent>
where
    F: FnOnce(&QuarantineOps, &Sender<QuarantineAdminEvent>) + Send + 'static,
{
    let (tx, rx) = channel();
    let fallback = tx.clone();
    let spawned = std::thread::Builder::new().name("stale-quarantine-admin".into()).spawn(move || {
        // Built here, not on the UI thread: it creates and locks down folders.
        let run = catch_unwind(AssertUnwindSafe(|| job(&QuarantineOps::live(), &tx)));
        if run.is_err() {
            // The panic hook has logged the details.
            let _ = tx.send(QuarantineAdminEvent::Failed(Warning::Fixed(Msg::QuarantineAdminAborted)));
        }
    });
    if let Err(e) = spawned {
        log::error!("could not spawn quarantine admin thread: {e}");
        let _ = fallback.send(QuarantineAdminEvent::Failed(Warning::Detail(Msg::QuarantineAdminThreadFailed, e.to_string())));
    }
    rx
}

/// Quarantines whose folders are owned by Administrators or SYSTEM.
fn trusted_manifests(ops: &QuarantineOps) -> Vec<QuarantineManifest> {
    list_trusted_manifests(&ops.manifest_dir, &*ops.trusted_dir, &|p| ops.mover.is_reparse(p))
}

fn find_manifest(ops: &QuarantineOps, id: &QuarantineId) -> Option<QuarantineManifest> {
    trusted_manifests(ops).into_iter().find(|m| &m.id == id)
}

/// Lists the quarantines.
pub fn spawn_quarantine_list() -> Receiver<QuarantineAdminEvent> {
    spawn_quarantine_admin(|ops, tx| {
        let _ = tx.send(QuarantineAdminEvent::Listed(trusted_manifests(ops)));
    })
}

/// Restores quarantine `id`, then sends the fresh list.
pub fn spawn_restore(id: QuarantineId) -> Receiver<QuarantineAdminEvent> {
    spawn_quarantine_admin(move |ops, tx| {
        let ev = match find_manifest(ops, &id) {
            Some(m) => {
                let outcome = restore(ops, &m);
                log::info!("restore of quarantine {}: {outcome:?}", id.0);
                QuarantineAdminEvent::Restored { id, outcome }
            }
            None => QuarantineAdminEvent::Failed(Warning::Detail(Msg::QuarantineEntryMissing, id.0.clone())),
        };
        let _ = tx.send(ev);
        let _ = tx.send(QuarantineAdminEvent::Listed(trusted_manifests(ops)));
    })
}

/// Purges each of `ids` (`Purged` or `Failed` per id), then sends `PurgedAll` and the fresh list.
pub fn spawn_purge(ids: Vec<QuarantineId>) -> Receiver<QuarantineAdminEvent> {
    spawn_quarantine_admin(move |ops, tx| {
        let manifests = trusted_manifests(ops);
        for id in ids {
            let ev = match manifests.iter().find(|m| m.id == id) {
                Some(m) => match purge(ops, m) {
                    Ok(()) => {
                        log::info!("purged quarantine {}", id.0);
                        QuarantineAdminEvent::Purged(id)
                    }
                    Err(e) => {
                        log::warn!("purge of quarantine {}: {e}", id.0);
                        QuarantineAdminEvent::Failed(Warning::PurgeFailed { program: m.program_name.clone(), error: e })
                    }
                },
                None => QuarantineAdminEvent::Failed(Warning::Detail(Msg::QuarantineEntryMissing, id.0.clone())),
            };
            let _ = tx.send(ev);
        }
        let _ = tx.send(QuarantineAdminEvent::PurgedAll);
        let _ = tx.send(QuarantineAdminEvent::Listed(trusted_manifests(ops)));
    })
}

/// Result of a standalone leftover search.
#[derive(Clone, Debug)]
pub enum LeftoverScanEvent {
    /// The searched program name and what was found.
    Done(String, LeftoverReport),
    Failed(Warning),
}

/// `hint` knowing the locations of every program in `programs` (the installed programs when the
/// search started), so their folders are never preselected. A manual hint belongs to no listed
/// program, so all of them count as others.
pub fn hint_with_others(hint: ProgramHint, programs: &[Program]) -> ProgramHint {
    hint.with_other_programs(programs, &ProgramId(String::new()))
}

/// Searches for `hint`'s leftovers on the "stale-leftovers" thread; `programs` are the installed
/// programs (see [`hint_with_others`]).
pub fn spawn_leftover_scan(hint: ProgramHint, programs: Vec<Program>) -> Receiver<LeftoverScanEvent> {
    let (tx, rx) = channel();
    let fallback = tx.clone();
    let spawned = std::thread::Builder::new().name("stale-leftovers".into()).spawn(move || {
        let hint = hint_with_others(hint, &programs);
        // Built here, not on the UI thread: it resolves every Start Menu and desktop shortcut.
        let run = catch_unwind(AssertUnwindSafe(|| scan_leftovers(&LeftoverSources::live(), &hint)));
        let ev = match run {
            Ok(report) => LeftoverScanEvent::Done(hint.name, report),
            Err(_) => LeftoverScanEvent::Failed(Warning::Fixed(Msg::LeftoverScanAborted)),
        };
        let _ = tx.send(ev);
    });
    if let Err(e) = spawned {
        log::error!("could not spawn leftover scan thread: {e}");
        let _ = fallback.send(LeftoverScanEvent::Failed(Warning::Detail(Msg::LeftoverScanThreadFailed, e.to_string())));
    }
    rx
}

/// Result of a ghost program scan.
#[derive(Clone, Debug)]
pub enum GhostEvent {
    Done(Vec<Ghost>),
    Failed(Warning),
}

/// Looks for ghost programs on the "stale-ghosts" thread; `programs` are every installed program
/// (hidden ones included) as of the last program scan and `unmatched` the execution traces the
/// usage scan matched to none. The uninstall list is read again on the thread and joined with
/// `programs` ([`merge_programs`]), so a program installed since the last scan is no ghost.
pub fn spawn_ghost_scan(programs: Vec<Program>, unmatched: Vec<UsageHit>) -> Receiver<GhostEvent> {
    let (tx, rx) = channel();
    let fallback = tx.clone();
    let spawned = std::thread::Builder::new().name("stale-ghosts".into()).spawn(move || {
        // Built here, not on the UI thread: it resolves every Start Menu and desktop shortcut.
        let run = catch_unwind(AssertUnwindSafe(|| {
            let fresh = stale_core::programs::enumerate_programs(&stale_core::registry::WinRegistry);
            scan_ghosts(&GhostSources::live(merge_programs(programs, fresh), unmatched))
        }));
        let ev = match run {
            Ok(ghosts) => GhostEvent::Done(ghosts),
            // The panic hook has logged the details.
            Err(_) => GhostEvent::Failed(Warning::Fixed(Msg::GhostScanAborted)),
        };
        let _ = tx.send(ev);
    });
    if let Err(e) = spawned {
        log::error!("could not spawn ghost scan thread: {e}");
        let _ = fallback.send(GhostEvent::Failed(Warning::Detail(Msg::GhostScanThreadFailed, e.to_string())));
    }
    rx
}

/// What the space scan reports. Every event is sent with the scan's generation, so the app can
/// drop what a cancelled scan still sends.
#[derive(Clone, Debug)]
pub enum SpaceEvent {
    /// Every entry, without sizes.
    Entries(Vec<SpaceEntry>),
    /// Entry `idx`'s size and whether it is only a lower bound.
    Size(usize, Option<u64>, bool),
    Done,
    Failed(Warning),
}

/// How many entries are measured at once.
const SPACE_MEASURE_THREADS: usize = 4;

/// Runs the space scan ("Sök rester") on the "stale-space" thread: lists the entries, then
/// measures them on a few threads and streams the sizes. `programs` are the installed programs as
/// of the last program scan; the uninstall list is read again and joined with them, as for the
/// ghost scan. Setting the returned flag, or dropping the receiver, stops the measuring once the
/// entries being measured are done.
pub fn spawn_space_scan(generation: u64, programs: Vec<Program>) -> (Receiver<(u64, SpaceEvent)>, Arc<AtomicBool>) {
    let (tx, rx) = channel();
    let fallback = tx.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let spawned = std::thread::Builder::new().name("stale-space".into()).spawn(move || {
        let run = catch_unwind(AssertUnwindSafe(|| space_scan(generation, programs, &flag, &tx)));
        if run.is_err() {
            // The panic hook has logged the details.
            let _ = tx.send((generation, SpaceEvent::Failed(Warning::Fixed(Msg::SpaceScanAborted))));
        }
    });
    if let Err(e) = spawned {
        log::error!("could not spawn space scan thread: {e}");
        let _ = fallback.send((generation, SpaceEvent::Failed(Warning::Detail(Msg::SpaceScanThreadFailed, e.to_string()))));
    }
    (rx, cancel)
}

/// What the space scan and the drill-down judge ownership by: `programs` joined with a fresh
/// read of the uninstall list ([`merge_programs`]).
fn live_installed(programs: Vec<Program>) -> InstalledIndex {
    let fresh = stale_core::programs::enumerate_programs(&WinRegistry);
    InstalledIndex::from_programs(&merge_programs(programs, fresh))
}

fn space_scan(generation: u64, programs: Vec<Program>, cancel: &AtomicBool, tx: &Sender<(u64, SpaceEvent)>) {
    let started = Instant::now();
    let fs = RealFs::live();
    stale_core::leftovers::init_profile_roots(&fs);
    let installed = live_installed(programs);
    let keep = KeepList::load_from(&stale_core::paths::keep_list_path());
    let roots = FsScanRoots::from_env(&fs);
    let entries = list_entries(&fs, &fixed_drives(), &roots, &installed, &keep, &OwnPaths::live());
    log::info!("space scan listed {} entries in {:.1} s", entries.len(), started.elapsed().as_secs_f64());
    if tx.send((generation, SpaceEvent::Entries(entries.clone()))).is_err() {
        return;
    }
    let report = |i: usize, size: Option<u64>, partial: bool| tx.send((generation, SpaceEvent::Size(i, size, partial))).is_ok();
    measure_all(&fs, &entries, SPACE_MEASURE_THREADS, cancel, &report);
    if cancel.load(Ordering::Relaxed) {
        log::info!("space scan cancelled after {:.1} s", started.elapsed().as_secs_f64());
        return;
    }
    log::info!("space scan measured {} entries in {:.1} s", entries.len(), started.elapsed().as_secs_f64());
    let _ = tx.send((generation, SpaceEvent::Done));
}

/// Measures `entries` on `threads` threads, each taking the next unmeasured index, and hands
/// every size to `report`. No new entry is taken once `cancel` is set; a `report` returning false
/// (the receiver is gone) sets it.
pub fn measure_all(
    fs: &dyn FsReader,
    entries: &[SpaceEntry],
    threads: usize,
    cancel: &AtomicBool,
    report: &(dyn Fn(usize, Option<u64>, bool) -> bool + Sync),
) {
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| loop {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(entry) = entries.get(i) else { return };
                let (size, partial) = measure(fs, entry);
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                if !report(i, size, partial) {
                    cancel.store(true, Ordering::Relaxed);
                    return;
                }
            });
        }
    });
}

/// One folder's children for the drill-down.
#[derive(Clone, Debug)]
pub struct BrowseResult {
    pub dir: PathBuf,
    pub children: Vec<Child>,
    pub more: usize,
}

/// Lists and measures `dir`'s children on the "stale-browse" thread (it can take long: every
/// child folder is measured); sends exactly one result.
pub fn spawn_browse(dir: PathBuf, programs: Vec<Program>) -> Receiver<Result<BrowseResult, Warning>> {
    let (tx, rx) = channel();
    let fallback = tx.clone();
    let spawned = std::thread::Builder::new().name("stale-browse".into()).spawn(move || {
        let run = catch_unwind(AssertUnwindSafe(|| {
            let fs = RealFs::live();
            let installed = live_installed(programs);
            let keep = KeepList::load_from(&stale_core::paths::keep_list_path());
            let (children, more) = children(&fs, &dir, &installed, &keep, &OwnPaths::live(), BROWSE_LIMIT);
            BrowseResult { dir, children, more }
        }));
        // A panic has been logged by the panic hook.
        let _ = tx.send(run.map_err(|_| Warning::Fixed(Msg::BrowseAborted)));
    });
    if let Err(e) = spawned {
        log::error!("could not spawn browse thread: {e}");
        let _ = fallback.send(Err(Warning::Detail(Msg::BrowseAborted, e.to_string())));
    }
    rx
}

/// Lists what removing the checked space entries `entries` would move (a temp folder's old
/// files are read from disk) on the "stale-space-removal" thread; sends exactly one result.
pub fn spawn_space_removal_list(entries: Vec<SpaceEntry>) -> Receiver<Result<LeftoverSelection, Warning>> {
    let (tx, rx) = channel();
    let fallback = tx.clone();
    let spawned = std::thread::Builder::new().name("stale-space-removal".into()).spawn(move || {
        let run = catch_unwind(AssertUnwindSafe(|| removal_selection_of(&RealFs::default(), &entries, Utc::now())));
        // A panic has been logged by the panic hook.
        let _ = tx.send(run.map_err(|_| Warning::Fixed(Msg::RemovalListAborted)));
    });
    if let Err(e) = spawned {
        log::error!("could not spawn space removal thread: {e}");
        let _ = fallback.send(Err(Warning::Detail(Msg::RemovalListAborted, e.to_string())));
    }
    rx
}

/// Reads `program`'s provenance (signature and version info of `main_exe`, install source) on
/// a "stale-provenance" thread: `WinVerifyTrust` can take a second. Sends exactly one result;
/// a panic sends what is known without the exe.
pub fn spawn_provenance(program: Program, main_exe: Option<PathBuf>) -> Receiver<Provenance> {
    let (tx, rx) = channel();
    let fallback = Provenance {
        signature: Signature::Unknown,
        description: None,
        company: None,
        install_date: program.install_date,
        source: InstallSource::Unknown,
    };
    let spawned = std::thread::Builder::new().name("stale-provenance".into()).spawn({
        let fallback = fallback.clone();
        move || {
            let run = catch_unwind(AssertUnwindSafe(|| provenance(&program, main_exe.as_deref(), &WinProbe, &WinRegistry)));
            let _ = tx.send(run.unwrap_or(fallback));
        }
    });
    if let Err(e) = spawned {
        log::error!("could not spawn provenance thread: {e}");
        let (tx, rx) = channel();
        let _ = tx.send(fallback);
        return rx;
    }
    rx
}

/// Keeps COM (STA) initialized on the worker thread for the restore point API, with process-wide
/// impersonate-level COM security. If the thread already uses another apartment model
/// (`RPC_E_CHANGED_MODE`) nothing is undone.
struct ComGuard {
    initialized: bool,
}

impl ComGuard {
    fn init() -> Self {
        // SAFETY: plain COM initialization; balanced by `CoUninitialize` in `drop` only on success.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        // The restore point API calls into a COM server that impersonates the caller; without
        // explicit process security COM picks identify-level defaults. Once per process: a later
        // call (or one after COM chose defaults) returns RPC_E_TOO_LATE, which is fine.
        // SAFETY: plain COM call with no pointers passed; COM is initialized on this thread above.
        let security = unsafe {
            CoInitializeSecurity(
                None,
                -1,
                None,
                None,
                RPC_C_AUTHN_LEVEL_DEFAULT,
                RPC_C_IMP_LEVEL_IMPERSONATE,
                None,
                EOAC_NONE,
                None,
            )
        };
        if let Err(e) = security {
            if e.code() != RPC_E_TOO_LATE {
                log::warn!("CoInitializeSecurity: {e}");
            }
        }
        ComGuard { initialized: hr.is_ok() }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.initialized {
            // SAFETY: matches the successful `CoInitializeEx` in `init` on this same thread.
            unsafe { CoUninitialize() }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stale_core::model::{Hive, RegKeyPath, Scope};
    use std::path::PathBuf;

    #[test]
    fn measure_all_reports_every_entry_once_and_stops_when_cancelled() {
        use stale_core::fsys::FakeFs;
        use stale_core::space::SpaceKind;
        use std::sync::Mutex;
        let mut fs = FakeFs::new();
        let mut entries = Vec::new();
        for i in 0..20u64 {
            let path = format!(r"D:\E{i}");
            fs = fs.dir(&path, i + 1);
            entries.push(SpaceEntry { path: PathBuf::from(path), is_dir: true, kind: SpaceKind::DriveRoot, size: None, partial: false, modified: None, note: None });
        }
        let seen = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);
        measure_all(&fs, &entries, 4, &cancel, &|i, size, partial| {
            seen.lock().unwrap().push((i, size, partial));
            true
        });
        let mut got = seen.into_inner().unwrap();
        got.sort();
        assert_eq!(got, (0..20).map(|i| (i, Some(i as u64 + 1), false)).collect::<Vec<_>>());
        // A gone receiver stops the measuring: at most one more entry per thread.
        let count = AtomicUsize::new(0);
        let cancel = AtomicBool::new(false);
        measure_all(&fs, &entries, 2, &cancel, &|_, _, _| {
            count.fetch_add(1, Ordering::SeqCst);
            false
        });
        assert!(cancel.load(Ordering::SeqCst));
        assert!(count.load(Ordering::SeqCst) <= 2);
        // Cancelled before it starts: nothing is measured.
        let cancel = AtomicBool::new(true);
        measure_all(&fs, &entries, 4, &cancel, &|_, _, _| panic!("measured after cancel"));
    }

    #[test]
    fn hint_with_others_counts_every_listed_program() {
        let program = Program {
            account: None,
            id: ProgramId("x".into()),
            key: RegKeyPath { hive: Hive::LocalMachine, path: r"Software\X".into() },
            name: "X".into(),
            publisher: None,
            version: None,
            install_location: Some(PathBuf::from(r"C:\Apps\X")),
            install_date: None,
            uninstall_string: None,
            quiet_uninstall_string: None,
            display_icon: None,
            is_msi: false,
            estimated_size_bytes: None,
            drive: Some('C'),
            hidden: false,
            scope: Scope::Machine,
        };
        let hint = hint_with_others(ProgramHint::manual("Y", None), &[program]);
        assert_eq!(hint.other_locations, vec![PathBuf::from(r"C:\APPS\X")]);
    }
}
