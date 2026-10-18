//! Runs the scan and the uninstall queue on background threads.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use chrono::Utc;
use stale_core::leftovers::{scan_leftovers, Leftover, LeftoverReport, LeftoverSources, ProgramHint};
use stale_core::quarantine::{quarantine, QuarantineManifest, QuarantineOps, QuarantineProgress};
use stale_core::scan::{run_scan, ScanEvent, ScanSources};
use stale_core::uninstall::{run_queue, QueueItem, QueueOptions, UninstallEvent, UninstallSources};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

use crate::strings;

/// Starts a scan on the "stale-scan" thread; events arrive on the returned channel.
pub fn spawn_scan() -> Receiver<ScanEvent> {
    let (tx, rx) = channel();
    let fallback = tx.clone();
    let spawned = std::thread::Builder::new()
        .name("stale-scan".into())
        .spawn(move || run_scan(ScanSources::live(), tx));
    if let Err(e) = spawned {
        log::error!("could not spawn scan thread: {e}");
        let _ = fallback.send(ScanEvent::Warning(format!("{}: {e}", strings::SCAN_THREAD_FAILED)));
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
        let _ = fallback.send(UninstallEvent::Warning(format!("{}: {e}", strings::UNINSTALL_THREAD_FAILED)));
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
    Failed(String),
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
            Err(_) => QuarantineEvent::Failed(strings::QUARANTINE_ABORTED.to_string()),
        };
        let _ = tx.send(ev);
    });
    if let Err(e) = spawned {
        log::error!("could not spawn quarantine thread: {e}");
        let _ = fallback.send(QuarantineEvent::Failed(format!("{}: {e}", strings::QUARANTINE_THREAD_FAILED)));
    }
    rx
}

/// Result of a standalone leftover search.
#[derive(Clone, Debug)]
pub enum LeftoverScanEvent {
    /// The searched program name and what was found.
    Done(String, LeftoverReport),
    Failed(String),
}

/// Searches for `hint`'s leftovers on the "stale-leftovers" thread.
pub fn spawn_leftover_scan(hint: ProgramHint) -> Receiver<LeftoverScanEvent> {
    let (tx, rx) = channel();
    let fallback = tx.clone();
    let spawned = std::thread::Builder::new().name("stale-leftovers".into()).spawn(move || {
        // Built here, not on the UI thread: it resolves every Start Menu and desktop shortcut.
        let run = catch_unwind(AssertUnwindSafe(|| scan_leftovers(&LeftoverSources::live(), &hint)));
        let ev = match run {
            Ok(report) => LeftoverScanEvent::Done(hint.name, report),
            Err(_) => LeftoverScanEvent::Failed(strings::LEFTOVER_SCAN_ABORTED.to_string()),
        };
        let _ = tx.send(ev);
    });
    if let Err(e) = spawned {
        log::error!("could not spawn leftover scan thread: {e}");
        let _ = fallback.send(LeftoverScanEvent::Failed(format!("{}: {e}", strings::LEFTOVER_SCAN_THREAD_FAILED)));
    }
    rx
}

/// Keeps COM (STA) initialized on the worker thread for the restore point API. If the thread
/// already uses another apartment model (`RPC_E_CHANGED_MODE`) nothing is undone.
struct ComGuard {
    initialized: bool,
}

impl ComGuard {
    fn init() -> Self {
        // SAFETY: plain COM initialization; balanced by `CoUninitialize` in `drop` only on success.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
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
