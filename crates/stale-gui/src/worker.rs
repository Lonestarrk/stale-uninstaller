//! Runs the scan on a background thread.

use std::sync::mpsc::{channel, Receiver};

use stale_core::scan::{run_scan, ScanEvent, ScanSources};

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
