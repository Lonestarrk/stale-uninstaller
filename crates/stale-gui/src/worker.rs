//! Runs the scan and the uninstall queue on background threads.

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

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
