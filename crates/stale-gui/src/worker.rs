//! Runs the scan, the uninstall queue and quarantine work on background threads.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use chrono::Utc;
use stale_core::leftovers::{scan_leftovers, Leftover, LeftoverReport, LeftoverSources, ProgramHint};
use stale_core::quarantine::{
    list_trusted_manifests, purge, quarantine, restore, QuarantineId, QuarantineManifest, QuarantineOps,
    QuarantineProgress, RestoreOutcome,
};
use stale_core::model::{Program, ProgramId};
use stale_core::scan::{run_scan, ScanEvent, ScanSources};
use stale_core::uninstall::{run_queue, QueueItem, QueueOptions, UninstallEvent, UninstallSources};
use windows::Win32::Foundation::RPC_E_TOO_LATE;
use windows::Win32::System::Com::{
    CoInitializeEx, CoInitializeSecurity, CoUninitialize, COINIT_APARTMENTTHREADED, EOAC_NONE,
    RPC_C_AUTHN_LEVEL_DEFAULT, RPC_C_IMP_LEVEL_IMPERSONATE,
};

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

/// Results of listing, restoring and purging quarantines.
#[derive(Clone, Debug)]
pub enum QuarantineAdminEvent {
    /// Every trusted quarantine, newest first.
    Listed(Vec<QuarantineManifest>),
    Restored { id: QuarantineId, outcome: RestoreOutcome },
    Purged(QuarantineId),
    /// Sent after the last `Purged` of a purge.
    PurgedAll,
    Failed(String),
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
            let _ = tx.send(QuarantineAdminEvent::Failed(strings::QUARANTINE_ADMIN_ABORTED.to_string()));
        }
    });
    if let Err(e) = spawned {
        log::error!("could not spawn quarantine admin thread: {e}");
        let _ = fallback.send(QuarantineAdminEvent::Failed(format!("{}: {e}", strings::QUARANTINE_ADMIN_THREAD_FAILED)));
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
            None => QuarantineAdminEvent::Failed(format!("{}: {}", strings::QUARANTINE_ENTRY_MISSING, id.0)),
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
                        QuarantineAdminEvent::Failed(format!(
                            "{} ({}): {}",
                            strings::PURGE_FAILED,
                            m.program_name,
                            strings::purge_error(&e)
                        ))
                    }
                },
                None => QuarantineAdminEvent::Failed(format!("{}: {}", strings::QUARANTINE_ENTRY_MISSING, id.0)),
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
    Failed(String),
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
    fn hint_with_others_counts_every_listed_program() {
        let program = Program {
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
