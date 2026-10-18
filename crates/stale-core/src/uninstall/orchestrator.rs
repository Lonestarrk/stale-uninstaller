//! The uninstall queue: one restore point, then for each program its vendor uninstaller and a
//! leftover scan, reported as a stream of events.

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use super::{
    run_vendor_uninstaller, JobObjectRunner, ProcessRunner, RestorePointApi, RestorePointStatus, SrClient,
    UninstallMode, VendorResult,
};
use crate::fsys::{FsReader, RealFs};
use crate::leftovers::{scan_leftovers, LeftoverReport, LeftoverSources, ProgramHint};
use crate::model::Program;
use crate::registry::{RegistryReader, WinRegistry};

pub struct QueueOptions {
    pub restore_point: bool,
    /// Ignored (treated as false) when `mode` is `Forced`.
    pub run_vendor: bool,
    pub mode: UninstallMode,
}

pub struct QueueItem {
    pub program: Program,
}

#[derive(Clone, Debug)]
pub enum UninstallEvent {
    RestorePoint(RestorePointStatus),
    Started { index: usize },
    Vendor { index: usize, result: VendorResult },
    Leftovers { index: usize, report: LeftoverReport },
    NotUninstalled { index: usize, exit_code: Option<i32> },
    Warning(String),
    QueueDone,
}

pub struct UninstallSources {
    pub restore: Arc<dyn RestorePointApi>,
    pub runner: Arc<dyn ProcessRunner>,
    pub reg: Arc<dyn RegistryReader>,
    pub fs: Arc<dyn FsReader>,
    pub leftovers: Arc<LeftoverSources>,
    /// Set by the GUI to stop waiting for an uninstaller's lingering child processes.
    pub stop_waiting: Arc<AtomicBool>,
}

impl UninstallSources {
    pub fn live() -> UninstallSources {
        UninstallSources::live_with_stop_flag(Arc::new(AtomicBool::new(false)))
    }

    /// Like `live`, with a `stop_waiting` flag the caller already holds (the GUI creates it
    /// before the worker thread builds the sources).
    pub fn live_with_stop_flag(stop_waiting: Arc<AtomicBool>) -> UninstallSources {
        UninstallSources {
            restore: Arc::new(SrClient),
            runner: Arc::new(JobObjectRunner::new(stop_waiting.clone())),
            reg: Arc::new(WinRegistry),
            fs: Arc::new(RealFs),
            // Built once: it resolves every Start Menu shortcut.
            leftovers: Arc::new(LeftoverSources::live()),
            stop_waiting,
        }
    }
}

const MAX_DESCRIPTION: usize = 200;

/// Runs the queue, sending events to `tx`; returns early if the receiver is gone.
pub fn run_queue(src: &UninstallSources, items: &[QueueItem], opts: &QueueOptions, tx: &Sender<UninstallEvent>) {
    let run_vendor = opts.run_vendor && opts.mode != UninstallMode::Forced;
    if opts.restore_point {
        let names: Vec<&str> = items.iter().map(|i| i.program.name.as_str()).collect();
        let description: String =
            format!("Stale Uninstaller: {}", names.join(", ")).chars().take(MAX_DESCRIPTION).collect();
        let status = src.restore.create(&description);
        if tx.send(UninstallEvent::RestorePoint(status)).is_err() {
            return;
        }
    }
    for (index, item) in items.iter().enumerate() {
        if tx.send(UninstallEvent::Started { index }).is_err() {
            return;
        }
        if run_vendor {
            let result = run_vendor_uninstaller(&item.program, &*src.runner, &*src.reg, &*src.fs);
            if tx.send(UninstallEvent::Vendor { index, result: result.clone() }).is_err() {
                return;
            }
            match result {
                VendorResult::NotUninstalled { exit_code } => {
                    if tx.send(UninstallEvent::NotUninstalled { index, exit_code }).is_err() {
                        return;
                    }
                    continue;
                }
                VendorResult::NoUninstaller => {
                    let warning = format!("{}: ingen avinstallerare hittades, tvingad borttagning", item.program.name);
                    if tx.send(UninstallEvent::Warning(warning)).is_err() {
                        return;
                    }
                }
                VendorResult::Uninstalled { .. } => {}
            }
        }
        let report = scan_leftovers(&src.leftovers, &ProgramHint::from_program(&item.program));
        if tx.send(UninstallEvent::Leftovers { index, report }).is_err() {
            return;
        }
    }
    let _ = tx.send(UninstallEvent::QueueDone);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsys::FakeFs;
    use crate::leftovers::fs_scan::FsScanRoots;
    use crate::leftovers::reg_scan::RegScanRoots;
    use crate::model::{Hive, ProgramId, RegKeyPath, Scope};
    use crate::registry::FakeRegistry;
    use crate::uninstall::FakeRestorePoint;
    use std::path::PathBuf;
    use std::sync::mpsc::channel;
    use std::sync::Mutex;

    const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

    fn program(name: &str, uninstall: Option<&str>) -> Program {
        Program {
            id: ProgramId(name.into()),
            key: RegKeyPath { hive: Hive::LocalMachine, path: format!(r"{UNINSTALL}\{name}") },
            name: name.into(),
            publisher: None,
            version: None,
            install_location: Some(PathBuf::from(format!(r"C:\{name}"))),
            install_date: None,
            uninstall_string: uninstall.map(str::to_string),
            quiet_uninstall_string: None,
            display_icon: None,
            is_msi: false,
            estimated_size_bytes: None,
            drive: Some('C'),
            hidden: false,
            scope: Scope::Machine,
        }
    }

    fn item(name: &str, uninstall: Option<&str>) -> QueueItem {
        QueueItem { program: program(name, uninstall) }
    }

    /// Exit 1602 for any command naming Alpha, 0 otherwise; records every command.
    struct KeyedRunner(Mutex<Vec<Vec<String>>>);

    impl ProcessRunner for KeyedRunner {
        fn run_and_wait_tree(&self, cmd: &[String]) -> Result<i32, String> {
            self.0.lock().unwrap().push(cmd.to_vec());
            Ok(if cmd[0].contains("Alpha") { 1602 } else { 0 })
        }
    }

    /// Alpha stays installed after its uninstaller runs; Beta's key is gone.
    fn sources(restore: RestorePointStatus) -> (UninstallSources, Arc<KeyedRunner>) {
        let fs = FakeFs::new()
            .env("ProgramFiles", r"C:\Program Files")
            .env("ProgramData", r"C:\ProgramData")
            .dir(r"C:\Program Files", 0)
            .profile(r"C:\Users\A")
            .dir(r"C:\Alpha", 1)
            .file(r"C:\Alpha\Alpha.exe", 10)
            .file(r"C:\Alpha\unins.exe", 5)
            .file(r"C:\Beta\unins.exe", 5);
        let fs: Arc<dyn FsReader> = Arc::new(fs);
        let mut reg = FakeRegistry::new();
        reg.add_key(program("Alpha", None).key);
        let reg: Arc<dyn RegistryReader> = Arc::new(reg);
        let leftovers = LeftoverSources {
            fs_roots: FsScanRoots::from_env(&*fs),
            reg_roots: RegScanRoots::standard(&*reg),
            tasks_root: PathBuf::from(r"C:\Windows\System32\Tasks"),
            start_menu_targets: Vec::new(),
            desktop_targets: Vec::new(),
            reg: reg.clone(),
            fs: fs.clone(),
        };
        let runner = Arc::new(KeyedRunner(Mutex::new(Vec::new())));
        let src = UninstallSources {
            restore: Arc::new(FakeRestorePoint(restore)),
            runner: runner.clone(),
            reg,
            fs,
            leftovers: Arc::new(leftovers),
            stop_waiting: Arc::new(AtomicBool::new(false)),
        };
        (src, runner)
    }

    fn collect(src: &UninstallSources, items: &[QueueItem], opts: &QueueOptions) -> Vec<UninstallEvent> {
        let (tx, rx) = channel();
        run_queue(src, items, opts, &tx);
        drop(tx);
        rx.iter().collect()
    }

    fn normal() -> QueueOptions {
        QueueOptions { restore_point: true, run_vendor: true, mode: UninstallMode::Normal }
    }

    /// A short tag per event, to assert order.
    fn tags(events: &[UninstallEvent]) -> Vec<String> {
        events
            .iter()
            .map(|e| match e {
                UninstallEvent::RestorePoint(_) => "restore".into(),
                UninstallEvent::Started { index } => format!("started{index}"),
                UninstallEvent::Vendor { index, .. } => format!("vendor{index}"),
                UninstallEvent::Leftovers { index, .. } => format!("leftovers{index}"),
                UninstallEvent::NotUninstalled { index, .. } => format!("notuninstalled{index}"),
                UninstallEvent::Warning(_) => "warning".into(),
                UninstallEvent::QueueDone => "done".into(),
            })
            .collect()
    }

    const BETA: &str = r"C:\Beta\unins.exe /S";

    #[test]
    fn queue_emits_restore_point_then_per_item_events_then_done() {
        let (src, _) = sources(RestorePointStatus::Created(7));
        let events = collect(&src, &[item("Beta", Some(BETA))], &normal());
        assert_eq!(tags(&events), ["restore", "started0", "vendor0", "leftovers0", "done"]);
        assert!(matches!(events[0], UninstallEvent::RestorePoint(RestorePointStatus::Created(7))));
        assert!(matches!(events[2], UninstallEvent::Vendor { result: VendorResult::Uninstalled { exit_code: 0 }, .. }));
    }

    #[test]
    fn cancelled_vendor_uninstall_skips_leftovers_but_queue_continues() {
        let (src, _) = sources(RestorePointStatus::Created(1));
        let items = [item("Alpha", Some(r"C:\Alpha\unins.exe")), item("Beta", Some(BETA))];
        let events = collect(&src, &items, &normal());
        assert_eq!(
            tags(&events),
            ["restore", "started0", "vendor0", "notuninstalled0", "started1", "vendor1", "leftovers1", "done"]
        );
        assert!(matches!(events[3], UninstallEvent::NotUninstalled { index: 0, exit_code: Some(1602) }));
    }

    #[test]
    fn forced_mode_never_runs_vendor() {
        let (src, runner) = sources(RestorePointStatus::Disabled);
        // run_vendor is true on purpose: Forced must override it.
        let opts = QueueOptions { restore_point: false, run_vendor: true, mode: UninstallMode::Forced };
        let events = collect(&src, &[item("Beta", Some(BETA))], &opts);
        assert_eq!(tags(&events), ["started0", "leftovers0", "done"]);
        assert!(runner.0.lock().unwrap().is_empty());
    }

    #[test]
    fn no_uninstaller_falls_to_forced_with_warning() {
        let (src, runner) = sources(RestorePointStatus::Created(1));
        let events = collect(&src, &[item("Gamma", None)], &normal());
        assert_eq!(tags(&events), ["restore", "started0", "vendor0", "warning", "leftovers0", "done"]);
        match &events[3] {
            UninstallEvent::Warning(w) => assert_eq!(w, "Gamma: ingen avinstallerare hittades, tvingad borttagning"),
            e => panic!("{e:?}"),
        }
        assert!(runner.0.lock().unwrap().is_empty());
    }

    #[test]
    fn restore_point_skipped_is_information_not_failure() {
        for status in [RestorePointStatus::SkippedFrequency, RestorePointStatus::Failed("x".into())] {
            let (src, _) = sources(status.clone());
            let events = collect(&src, &[item("Beta", Some(BETA))], &normal());
            assert_eq!(tags(&events), ["restore", "started0", "vendor0", "leftovers0", "done"]);
            assert!(matches!(&events[0], UninstallEvent::RestorePoint(s) if *s == status));
        }
    }

    #[test]
    fn dropped_receiver_stops_the_queue() {
        let (src, runner) = sources(RestorePointStatus::Created(1));
        let (tx, rx) = channel();
        drop(rx);
        run_queue(&src, &[item("Beta", Some(BETA))], &normal(), &tx);
        assert!(runner.0.lock().unwrap().is_empty());
    }
}
