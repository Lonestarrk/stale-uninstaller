//! Application state and the eframe shell.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use eframe::egui;
use stale_core::leftovers::ProgramHint;
use stale_core::quarantine::{Failure, QuarantineId, QuarantineManifest, RestoreOutcome};
use stale_core::scan::ScanEvent;
use stale_core::settings::Settings;
use stale_core::uninstall::{QueueItem, QueueOptions, UninstallEvent, UninstallMode};
use stale_core::usage::matching::ProgramUsage;
use stale_core::{Program, ProgramId};

use crate::filter::{bucket_counts, visible_rows, Filters, Sort};
use crate::flow::FlowState;
use crate::leftover_sel::LeftoverSelection;
use crate::ui::{details, filter_panel, leftovers_view, quarantine_view, table, toolbar, uninstall_dialog};
use crate::worker::{LeftoverScanEvent, QuarantineAdminEvent, QuarantineEvent};
use crate::{strings, worker};

pub struct Row {
    pub program: Program,
    pub usage: Option<ProgramUsage>,
    pub size: Option<u64>,
}

impl Row {
    /// When the program was last seen in use, if ever.
    pub fn last_used(&self) -> Option<DateTime<Utc>> {
        self.usage.as_ref().and_then(|u| u.last_used.when)
    }

    /// The registry estimate, else the measured folder size (spec §5).
    pub fn size_bytes(&self) -> Option<u64> {
        self.program.estimated_size_bytes.or(self.size)
    }
}

/// Which list fills the central panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Programs,
    Leftovers,
    Quarantine,
}

/// The summary dialog's state before a queue starts.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingQueue {
    pub ids: Vec<ProgramId>,
    pub mode: UninstallMode,
    pub restore_point: bool,
    pub run_vendor: bool,
}

/// The "Sök rester" window's fields while it is open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchDialog {
    pub name: String,
    /// Optional install folder; blank means none.
    pub dir: String,
}

pub struct StaleApp {
    pub rows: Vec<Row>,
    pub settings: Settings,
    pub scanning: bool,
    pub warnings: Vec<String>,
    pub filters: Filters,
    pub sort: Sort,
    pub selected: Option<ProgramId>,
    pub checked: HashSet<ProgramId>,
    pub view: View,
    pub pending: Option<PendingQueue>,
    pub flow: Option<FlowState>,
    uninstall_rx: Option<Receiver<UninstallEvent>>,
    /// Set to stop waiting for the running uninstaller's child processes; `None` when no queue runs.
    pub stop_waiting: Option<Arc<AtomicBool>>,
    rx: Option<Receiver<ScanEvent>>,
    /// The leftovers view's items and check marks.
    pub leftovers: Option<LeftoverSelection>,
    quarantine_rx: Option<Receiver<QuarantineEvent>>,
    /// `(done, total, what)` while checked leftovers are being moved to quarantine.
    pub removal_progress: Option<(usize, usize, String)>,
    /// The removal has finished: the leftovers view shows `last_manifests` instead of the items.
    pub removal_done: bool,
    pub last_manifests: Vec<QuarantineManifest>,
    /// The "Sök rester" window, when open.
    pub search: Option<SearchDialog>,
    leftover_scan_rx: Option<Receiver<LeftoverScanEvent>>,
    /// The quarantine view's rows, newest first.
    pub quarantine: Vec<QuarantineManifest>,
    /// True while a quarantine list, restore or purge runs (one at a time).
    pub quarantine_busy: bool,
    quarantine_admin_rx: Option<Receiver<QuarantineAdminEvent>>,
    /// The "Töm karantän" confirmation is open.
    pub confirm_purge_all: bool,
    /// What the last partial restore could not put back, shown under its row.
    pub last_restore_failures: Option<(QuarantineId, Vec<Failure>)>,
    /// Every program from the scan, hidden ones included.
    programs: Vec<Program>,
    /// Usage and sizes received so far, kept for hidden programs too.
    usage: HashMap<ProgramId, ProgramUsage>,
    sizes: HashMap<ProgramId, u64>,
}

impl StaleApp {
    /// Loads settings and starts the background scan.
    pub fn new(_cc: &eframe::CreationContext) -> Self {
        let mut app = StaleApp::empty(Settings::load_from(&Settings::path()));
        app.rx = Some(worker::spawn_scan());
        app.scanning = true;
        // For the toolbar's count.
        app.refresh_quarantine();
        app
    }

    /// State with no scan running.
    pub fn empty(settings: Settings) -> Self {
        StaleApp {
            rows: Vec::new(),
            settings,
            scanning: false,
            warnings: Vec::new(),
            filters: Filters::default(),
            sort: Sort::default(),
            selected: None,
            checked: HashSet::new(),
            view: View::Programs,
            pending: None,
            flow: None,
            uninstall_rx: None,
            stop_waiting: None,
            rx: None,
            leftovers: None,
            quarantine_rx: None,
            removal_progress: None,
            removal_done: false,
            last_manifests: Vec::new(),
            search: None,
            leftover_scan_rx: None,
            quarantine: Vec::new(),
            quarantine_busy: false,
            quarantine_admin_rx: None,
            confirm_purge_all: false,
            last_restore_failures: None,
            programs: Vec::new(),
            usage: HashMap::new(),
            sizes: HashMap::new(),
        }
    }

    /// Drops all scan results and starts a new scan.
    pub fn refresh(&mut self) {
        self.clear_scan_data();
        self.rx = Some(worker::spawn_scan());
        self.scanning = true;
    }

    /// Forgets programs, usage, sizes and warnings from the previous scan.
    fn clear_scan_data(&mut self) {
        self.rows.clear();
        self.programs.clear();
        self.usage.clear();
        self.sizes.clear();
        self.warnings.clear();
    }

    /// Folds one scan event into the state (no UI work).
    pub fn apply_event(&mut self, ev: ScanEvent) {
        match ev {
            ScanEvent::Programs(programs) => {
                self.programs = programs;
                self.rebuild_rows();
            }
            ScanEvent::Usage(usage) => {
                for row in &mut self.rows {
                    if let Some(u) = usage.get(&row.program.id) {
                        row.usage = Some(u.clone());
                    }
                }
                self.usage.extend(usage);
            }
            ScanEvent::UnmatchedHits(_) => {}
            ScanEvent::Sizes(sizes) => {
                for row in &mut self.rows {
                    if let Some(&s) = sizes.get(&row.program.id) {
                        row.size = Some(s);
                    }
                }
                self.sizes.extend(sizes);
            }
            ScanEvent::Warning(w) => {
                log::warn!("{w}");
                self.warnings.push(w);
            }
            ScanEvent::Done => self.scanning = false,
        }
    }

    /// Rebuilds `rows` from all programs, dropping hidden ones unless `settings.show_hidden`.
    pub fn rebuild_rows(&mut self) {
        self.rows = self
            .programs
            .iter()
            .filter(|p| self.settings.show_hidden || !p.hidden)
            .map(|p| Row {
                program: p.clone(),
                usage: self.usage.get(&p.id).cloned(),
                size: self.sizes.get(&p.id).copied(),
            })
            .collect();
    }

    /// Flips "show hidden entries", re-applies it to the rows without rescanning and saves.
    pub fn toggle_show_hidden(&mut self) {
        self.toggle_show_hidden_at(&Settings::path());
    }

    fn toggle_show_hidden_at(&mut self, path: &Path) {
        self.settings.show_hidden = !self.settings.show_hidden;
        self.rebuild_rows();
        let ids: HashSet<&ProgramId> = self.rows.iter().map(|r| &r.program.id).collect();
        self.checked.retain(|id| ids.contains(id));
        if self.selected.as_ref().is_some_and(|id| !ids.contains(id)) {
            self.selected = None;
        }
        if let Err(e) = self.settings.save_to(path) {
            log::warn!("could not save settings to {}: {e}", path.display());
        }
    }

    /// Sets the auto-purge age (0 turns it off) and saves.
    pub fn set_quarantine_days(&mut self, days: u32) {
        self.set_quarantine_days_at(days, &Settings::path());
    }

    fn set_quarantine_days_at(&mut self, days: u32, path: &Path) {
        self.settings.quarantine_days = days;
        if let Err(e) = self.settings.save_to(path) {
            log::warn!("could not save settings to {}: {e}", path.display());
        }
    }

    /// Opens the summary dialog for the checked rows that are visible under the current filters.
    pub fn open_queue_dialog(&mut self, mode: UninstallMode) {
        let visible = visible_rows(&self.rows, &self.filters, self.sort, Utc::now());
        let ids: Vec<ProgramId> = visible
            .into_iter()
            .map(|i| &self.rows[i].program.id)
            .filter(|id| self.checked.contains(*id))
            .cloned()
            .collect();
        self.open_queue_dialog_for(ids, mode);
    }

    /// Opens the summary dialog for `ids` (nothing happens when it is empty).
    pub fn open_queue_dialog_for(&mut self, ids: Vec<ProgramId>, mode: UninstallMode) {
        if ids.is_empty() {
            return;
        }
        self.pending =
            Some(PendingQueue { ids, mode, restore_point: true, run_vendor: mode == UninstallMode::Normal });
    }

    /// True while a queue has been started and has not finished.
    pub fn queue_running(&self) -> bool {
        self.flow.as_ref().is_some_and(|f| !f.finished)
    }

    /// Starts the pending queue on a worker thread and closes the summary dialog.
    pub fn start_queue(&mut self) {
        let Some(pending) = self.pending.take() else { return };
        let programs: Vec<Program> = pending
            .ids
            .iter()
            .filter_map(|id| self.rows.iter().find(|r| &r.program.id == id))
            .map(|r| r.program.clone())
            .collect();
        if programs.is_empty() {
            return;
        }
        let names: Vec<(ProgramId, String)> = programs.iter().map(|p| (p.id.clone(), p.name.clone())).collect();
        let items = programs.into_iter().map(|program| QueueItem { program }).collect();
        let opts = QueueOptions { restore_point: pending.restore_point, run_vendor: pending.run_vendor, mode: pending.mode };
        let (rx, stop_waiting) = worker::spawn_uninstall(items, opts);
        self.uninstall_rx = Some(rx);
        self.stop_waiting = Some(stop_waiting);
        self.flow = Some(FlowState::new(&names));
    }

    /// Closes the finished queue's results.
    pub fn close_flow(&mut self) {
        self.flow = None;
        self.stop_waiting = None;
    }

    /// Applies every uninstall event waiting on the channel; rescans once the queue is done.
    fn drain_uninstall_events(&mut self) {
        let Some(rx) = self.uninstall_rx.take() else { return };
        let Some(flow) = self.flow.as_mut() else { return };
        loop {
            match rx.try_recv() {
                Ok(ev) => {
                    if let UninstallEvent::Warning(w) = &ev {
                        log::warn!("{w}");
                    }
                    flow.apply(ev);
                    if flow.finished {
                        break;
                    }
                }
                Err(TryRecvError::Empty) => {
                    self.uninstall_rx = Some(rx);
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    // Gone without `QueueDone`: the worker panicked (logged by the panic hook).
                    log::error!("uninstall worker disconnected before QueueDone");
                    flow.warnings.push(strings::QUEUE_ABORTED.to_string());
                    flow.finish();
                    break;
                }
            }
        }
        self.stop_waiting = None;
        // The list should drop what was uninstalled. A running scan's results are already stale.
        self.refresh();
    }

    /// Shows the finished queue's leftover reports, named after their programs.
    pub fn open_leftovers_from_flow(&mut self) {
        let Some(flow) = &self.flow else { return };
        let reports: Vec<_> = flow
            .reports
            .iter()
            .filter_map(|(i, report)| flow.items.get(*i).map(|(_, name, _)| (name.clone(), report.clone())))
            .collect();
        self.show_leftovers(LeftoverSelection::from_reports(&reports));
    }

    /// Switches to the leftovers view with a fresh selection.
    fn show_leftovers(&mut self, sel: LeftoverSelection) {
        self.leftovers = Some(sel);
        self.removal_done = false;
        self.last_manifests.clear();
        self.view = View::Leftovers;
    }

    /// Back to the program list; a finished removal's results are dropped on the way.
    pub fn leave_leftovers(&mut self) {
        self.view = View::Programs;
        if self.removal_done {
            self.leftovers = None;
            self.removal_done = false;
            self.last_manifests.clear();
            // Its reports name what was just quarantined: "Visa rester" must not offer them again.
            if !self.queue_running() {
                self.close_flow();
            }
        }
    }

    /// True while checked leftovers are being moved to quarantine.
    pub fn removal_running(&self) -> bool {
        self.quarantine_rx.is_some()
    }

    /// Moves the checked leftovers to quarantine on a worker thread (nothing checked: no-op).
    pub fn remove_checked_leftovers(&mut self) {
        if self.removal_running() {
            return;
        }
        let Some(sel) = &self.leftovers else { return };
        let batches = sel.checked_items();
        if batches.is_empty() {
            return;
        }
        let total = batches.iter().map(|(_, items)| items.len()).sum();
        self.removal_progress = Some((0, total, String::new()));
        self.quarantine_rx = Some(worker::spawn_quarantine(batches));
    }

    /// True while a standalone leftover search runs.
    pub fn leftover_search_running(&self) -> bool {
        self.leftover_scan_rx.is_some()
    }

    /// Searches for leftovers of a program named `name` (optionally installed in `dir`) on a worker.
    pub fn search_leftovers_manual(&mut self, name: String, dir: Option<PathBuf>) {
        if self.leftover_search_running() {
            return;
        }
        let hint = ProgramHint::manual(&name, dir);
        self.leftover_scan_rx = Some(worker::spawn_leftover_scan(hint, self.programs.clone()));
    }

    /// Applies every quarantine event waiting on the channel.
    fn drain_quarantine_events(&mut self) {
        let Some(rx) = self.quarantine_rx.take() else { return };
        loop {
            match rx.try_recv() {
                Ok(QuarantineEvent::Progress { done, total, what }) => {
                    self.removal_progress = Some((done, total, what))
                }
                Ok(QuarantineEvent::Finished(manifests)) => {
                    self.removal_progress = None;
                    self.removal_done = true;
                    self.last_manifests = manifests;
                    self.refresh_quarantine();
                    return;
                }
                Ok(QuarantineEvent::Failed(msg)) => {
                    log::error!("{msg}");
                    self.removal_progress = None;
                    self.warnings.push(msg);
                    return;
                }
                Err(TryRecvError::Empty) => {
                    self.quarantine_rx = Some(rx);
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    log::error!("quarantine worker disconnected before Finished");
                    self.removal_progress = None;
                    self.warnings.push(strings::QUARANTINE_ABORTED.to_string());
                    return;
                }
            }
        }
    }

    /// Switches to the quarantine view and reloads its rows; a finished removal is closed first.
    pub fn show_quarantine(&mut self) {
        if self.view == View::Leftovers {
            self.leave_leftovers();
        }
        self.view = View::Quarantine;
        self.refresh_quarantine();
    }

    /// Lists the quarantines on a worker (no-op while another quarantine action runs).
    pub fn refresh_quarantine(&mut self) {
        self.start_quarantine_admin(worker::spawn_quarantine_list);
    }

    /// Restores quarantine `id` on a worker.
    pub fn restore_quarantine(&mut self, id: QuarantineId) {
        self.start_quarantine_admin(|| worker::spawn_restore(id));
    }

    /// Purges `ids` for good on a worker (nothing given: no-op).
    pub fn purge_quarantine(&mut self, ids: Vec<QuarantineId>) {
        if ids.is_empty() {
            return;
        }
        self.start_quarantine_admin(|| worker::spawn_purge(ids));
    }

    fn start_quarantine_admin(&mut self, spawn: impl FnOnce() -> Receiver<QuarantineAdminEvent>) {
        if self.quarantine_busy {
            return;
        }
        self.quarantine_admin_rx = Some(spawn());
        self.quarantine_busy = true;
    }

    /// Folds one quarantine list/restore/purge result into the state.
    pub fn apply_quarantine_admin(&mut self, ev: QuarantineAdminEvent) {
        match ev {
            QuarantineAdminEvent::Listed(manifests) => self.quarantine = manifests,
            QuarantineAdminEvent::Restored { id, outcome: RestoreOutcome::Restored } => {
                self.quarantine.retain(|m| m.id != id);
                if self.last_restore_failures.as_ref().is_some_and(|(f, _)| *f == id) {
                    self.last_restore_failures = None;
                }
            }
            QuarantineAdminEvent::Restored { id, outcome: RestoreOutcome::Partial(failures) } => {
                self.last_restore_failures = Some((id, failures));
            }
            QuarantineAdminEvent::Purged(id) => self.quarantine.retain(|m| m.id != id),
            QuarantineAdminEvent::PurgedAll => {}
            QuarantineAdminEvent::Failed(msg) => {
                log::error!("{msg}");
                self.warnings.push(msg);
            }
        }
    }

    /// Applies every quarantine list/restore/purge result waiting on the channel.
    fn drain_quarantine_admin(&mut self) {
        let Some(rx) = self.quarantine_admin_rx.take() else { return };
        loop {
            match rx.try_recv() {
                Ok(ev) => self.apply_quarantine_admin(ev),
                Err(TryRecvError::Empty) => {
                    self.quarantine_admin_rx = Some(rx);
                    return;
                }
                // The worker sends `Failed` itself when it panics, so a disconnect just means done.
                Err(TryRecvError::Disconnected) => {
                    self.quarantine_busy = false;
                    return;
                }
            }
        }
    }

    /// Opens the leftovers view once the standalone search is done.
    fn drain_leftover_scan(&mut self) {
        let Some(rx) = self.leftover_scan_rx.take() else { return };
        match rx.try_recv() {
            Ok(LeftoverScanEvent::Done(name, report)) => {
                self.show_leftovers(LeftoverSelection::from_reports(&[(name, report)]))
            }
            Ok(LeftoverScanEvent::Failed(msg)) => {
                log::error!("{msg}");
                self.warnings.push(msg);
            }
            Err(TryRecvError::Empty) => self.leftover_scan_rx = Some(rx),
            Err(TryRecvError::Disconnected) => {
                log::error!("leftover scan worker disconnected before Done");
                self.warnings.push(strings::LEFTOVER_SCAN_ABORTED.to_string());
            }
        }
    }

    /// Applies every event waiting on the channel.
    fn drain_events(&mut self) {
        let Some(rx) = self.rx.take() else { return };
        loop {
            match rx.try_recv() {
                Ok(ev) => self.apply_event(ev),
                Err(TryRecvError::Empty) => {
                    self.rx = Some(rx);
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    // The worker is gone. Without `Done` first it died mid-scan (a panic,
                    // logged by the panic hook): say so instead of showing partial results silently.
                    if self.scanning {
                        log::error!("scan worker disconnected before Done");
                        self.warnings.push(strings::SCAN_ABORTED.to_string());
                        self.scanning = false;
                    }
                    return;
                }
            }
        }
    }
}

impl eframe::App for StaleApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        self.drain_uninstall_events();
        self.drain_quarantine_events();
        self.drain_leftover_scan();
        self.drain_quarantine_admin();
        if self.scanning
            || self.queue_running()
            || self.removal_running()
            || self.leftover_search_running()
            || self.quarantine_busy
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = Utc::now();
        egui::Panel::top("top").show(ui, |ui| {
            toolbar::show(ui, self);
            if !self.warnings.is_empty() {
                ui.collapsing(strings::WARNINGS, |ui| {
                    for w in &self.warnings {
                        ui.label(w);
                    }
                });
            }
        });
        let counts = bucket_counts(&self.rows, now);
        egui::Panel::left("filters")
            .default_size(170.0)
            .show(ui, |ui| filter_panel::show(ui, &mut self.filters, &counts));
        egui::Panel::bottom("details").resizable(true).default_size(160.0).show(ui, |ui| {
            let row = self.selected.as_ref().and_then(|id| self.rows.iter().find(|r| &r.program.id == id));
            match row {
                Some(row) => details::details_panel(ui, row),
                None => {
                    ui.label(strings::NO_SELECTION);
                }
            }
        });
        uninstall_dialog::show(ui, self);
        leftovers_view::search_window(ui, self);
        egui::CentralPanel::default().show(ui, |ui| {
            match self.view {
                View::Programs => {}
                View::Leftovers => return leftovers_view::show(ui, self),
                View::Quarantine => return quarantine_view::show(ui, self),
            }
            if self.rows.is_empty() {
                if !self.scanning {
                    ui.label(strings::NO_PROGRAMS);
                }
                return;
            }
            let visible = visible_rows(&self.rows, &self.filters, self.sort, now);
            let state =
                table::TableState { sort: &mut self.sort, selected: &mut self.selected, checked: &mut self.checked };
            table::show(ui, &self.rows, &visible, state, now);
        });
    }
}

#[cfg(test)]
mod tests {
    use stale_core::quarantine::{Failure, QuarantineId, RestoreOutcome};
    use stale_core::usage::matching::LastUsed;
    use stale_core::{Hive, RegKeyPath, Scope};

    use crate::worker::QuarantineAdminEvent;

    use super::*;

    fn program(id: &str, hidden: bool) -> Program {
        Program {
            id: ProgramId(id.to_string()),
            key: RegKeyPath { hive: Hive::LocalMachine, path: id.to_string() },
            name: format!("App {id}"),
            publisher: None,
            version: None,
            install_location: None,
            install_date: None,
            uninstall_string: None,
            quiet_uninstall_string: None,
            display_icon: None,
            is_msi: false,
            estimated_size_bytes: None,
            drive: Some('C'),
            hidden,
            scope: Scope::Machine,
        }
    }

    fn usage() -> ProgramUsage {
        ProgramUsage { last_used: LastUsed { when: None, source: None, confident: true }, exes: Vec::new() }
    }

    #[test]
    fn apply_programs_creates_rows_then_usage_and_sizes_attach_by_id() {
        let mut app = StaleApp::empty(Settings::default());
        app.scanning = true;
        app.apply_event(ScanEvent::Programs(vec![program("a", false), program("b", false)]));
        assert_eq!(app.rows.len(), 2);
        assert!(app.rows.iter().all(|r| r.usage.is_none() && r.size.is_none()));

        app.apply_event(ScanEvent::Usage(HashMap::from([(ProgramId("a".into()), usage())])));
        app.apply_event(ScanEvent::Sizes(HashMap::from([(ProgramId("b".into()), 1234)])));
        app.apply_event(ScanEvent::Warning("w".into()));
        let a = app.rows.iter().find(|r| r.program.id.0 == "a").unwrap();
        let b = app.rows.iter().find(|r| r.program.id.0 == "b").unwrap();
        assert_eq!(a.usage, Some(usage()));
        assert_eq!(a.size, None);
        assert_eq!(b.usage, None);
        assert_eq!(b.size, Some(1234));
        assert_eq!(app.warnings, vec!["w".to_string()]);
        assert!(app.scanning);

        app.apply_event(ScanEvent::Done);
        assert!(!app.scanning);
    }

    /// An app that is scanning and reads `events` from a worker that has already exited.
    fn app_after_worker(events: Vec<ScanEvent>) -> StaleApp {
        let (tx, rx) = std::sync::mpsc::channel();
        for ev in events {
            tx.send(ev).unwrap();
        }
        drop(tx);
        let mut app = StaleApp::empty(Settings::default());
        app.rx = Some(rx);
        app.scanning = true;
        app
    }

    #[test]
    fn disconnect_before_done_warns() {
        let mut app = app_after_worker(vec![ScanEvent::Programs(vec![program("a", false)])]);
        app.drain_events();
        assert!(!app.scanning);
        assert_eq!(app.rows.len(), 1, "events before the disconnect are applied");
        assert_eq!(app.warnings, vec![strings::SCAN_ABORTED.to_string()]);
        assert!(app.rx.is_none());
    }

    #[test]
    fn done_then_disconnect_does_not_warn() {
        let mut app = app_after_worker(vec![ScanEvent::Programs(vec![program("a", false)]), ScanEvent::Done]);
        app.drain_events();
        assert!(!app.scanning);
        assert!(app.warnings.is_empty());
        assert!(app.rx.is_none());
    }

    #[test]
    fn drain_keeps_receiver_while_worker_runs() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut app = StaleApp::empty(Settings::default());
        app.rx = Some(rx);
        app.scanning = true;
        tx.send(ScanEvent::Warning("w".into())).unwrap();
        app.drain_events();
        assert!(app.scanning && app.rx.is_some());
        assert_eq!(app.warnings, vec!["w".to_string()]);
    }

    #[test]
    fn clear_scan_data_forgets_previous_scan() {
        let mut app = StaleApp::empty(Settings::default());
        app.apply_event(ScanEvent::Programs(vec![program("a", false)]));
        app.apply_event(ScanEvent::Sizes(HashMap::from([(ProgramId("a".into()), 7)])));
        app.apply_event(ScanEvent::Warning("w".into()));
        app.clear_scan_data();
        assert!(app.rows.is_empty() && app.warnings.is_empty());
        app.rebuild_rows();
        assert!(app.rows.is_empty(), "no programs left to rebuild from");
        app.apply_event(ScanEvent::Programs(vec![program("a", false)]));
        assert_eq!(app.rows[0].size, None, "old sizes are gone");
    }

    #[test]
    fn hidden_rows_dropped_unless_setting() {
        let mut app = StaleApp::empty(Settings { show_hidden: false, ..Settings::default() });
        app.apply_event(ScanEvent::Programs(vec![program("a", false), program("h", true)]));
        app.apply_event(ScanEvent::Sizes(HashMap::from([(ProgramId("a".into()), 7), (ProgramId("h".into()), 9)])));
        assert_eq!(app.rows.len(), 1);
        assert_eq!(app.rows[0].program.id.0, "a");

        app.settings.show_hidden = true;
        app.rebuild_rows();
        assert_eq!(app.rows.len(), 2);
        let a = app.rows.iter().find(|r| r.program.id.0 == "a").unwrap();
        assert_eq!(a.size, Some(7), "re-applying the filter keeps attached data");
        let h = app.rows.iter().find(|r| r.program.id.0 == "h").unwrap();
        assert_eq!(h.size, Some(9), "data received while hidden is kept");
    }

    #[test]
    fn open_queue_dialog_uses_only_visible_checked_rows() {
        let mut app = StaleApp::empty(Settings { show_hidden: false, ..Settings::default() });
        app.apply_event(ScanEvent::Programs(vec![
            program("keep", false),
            program("other", false),
            program("hidden", true),
        ]));
        for id in ["keep", "other", "hidden"] {
            app.checked.insert(ProgramId(id.into()));
        }
        app.filters.search = "keep".into();
        app.open_queue_dialog(UninstallMode::Normal);
        let pending = app.pending.expect("dialog opened");
        assert_eq!(pending.ids, vec![ProgramId("keep".into())]);
        assert_eq!(pending.mode, UninstallMode::Normal);
        assert!(pending.restore_point && pending.run_vendor);
    }

    #[test]
    fn toggling_show_hidden_reapplies_without_rescan() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut app = StaleApp::empty(Settings { show_hidden: false, ..Settings::default() });
        app.apply_event(ScanEvent::Programs(vec![program("h", true), program("v", false)]));
        assert_eq!(app.rows.len(), 1);

        app.toggle_show_hidden_at(&path);
        assert_eq!(app.rows.len(), 2);
        assert!(app.settings.show_hidden);
        assert!(Settings::load_from(&path).show_hidden, "setting is saved");
        app.checked.insert(ProgramId("h".into()));
        app.selected = Some(ProgramId("h".into()));

        app.toggle_show_hidden_at(&path);
        assert_eq!(app.rows.len(), 1);
        assert!(app.checked.is_empty(), "checked id that disappeared is pruned");
        assert_eq!(app.selected, None);
    }

    #[test]
    fn open_leftovers_from_flow_pairs_reports_with_names() {
        use stale_core::leftovers::{Confidence, Leftover, LeftoverKind, LeftoverReport};
        let dir = |p: &str| Leftover {
            kind: LeftoverKind::Dir { path: p.into(), size: Some(1) },
            confidence: Confidence::High,
            note: None,
        };
        let mut app = StaleApp::empty(Settings::default());
        let mut flow = FlowState::new(&[(ProgramId("a".into()), "App A".into()), (ProgramId("b".into()), "App B".into())]);
        // Reports are keyed by item index and may arrive in any order: pair by index.
        flow.reports.push((1, LeftoverReport { items: vec![dir(r"C:\B")] }));
        flow.reports.push((0, LeftoverReport { items: vec![dir(r"C:\A"), dir(r"C:\A2")] }));
        app.flow = Some(flow);

        app.open_leftovers_from_flow();
        assert_eq!(app.view, View::Leftovers);
        let sel = app.leftovers.expect("selection built");
        let groups: Vec<(&str, usize)> = sel.groups.iter().map(|g| (g.program_name.as_str(), g.items.len())).collect();
        assert_eq!(groups, vec![("App B", 1), ("App A", 2)]);
    }

    #[test]
    fn remove_checked_leftovers_with_nothing_checked_is_noop() {
        use stale_core::leftovers::{Confidence, Leftover, LeftoverKind, LeftoverReport};
        let low = Leftover {
            kind: LeftoverKind::Dir { path: r"C:\Users\x\A".into(), size: Some(1) },
            confidence: Confidence::Low,
            note: Some("user-folder".into()),
        };
        let mut app = StaleApp::empty(Settings::default());
        app.leftovers = Some(LeftoverSelection::from_reports(&[("A".into(), LeftoverReport { items: vec![low] })]));
        app.remove_checked_leftovers();
        assert!(app.quarantine_rx.is_none(), "no worker spawned");
        assert!(app.removal_progress.is_none() && !app.removal_done);

        app.leftovers = None;
        app.remove_checked_leftovers();
        assert!(app.quarantine_rx.is_none());
    }

    fn manifest(id: &str) -> QuarantineManifest {
        QuarantineManifest {
            id: QuarantineId(id.to_string()),
            program_name: format!("App {id}"),
            created: DateTime::from_timestamp(1_792_000_000, 0).unwrap(),
            moved: vec![],
            keys: vec![],
            values: vec![],
            services: vec![],
            tasks: vec![],
            failures: vec![],
            notes: vec![],
            pending: vec![],
        }
    }

    #[test]
    fn quarantine_list_event_replaces_rows() {
        let mut app = StaleApp::empty(Settings::default());
        app.apply_quarantine_admin(QuarantineAdminEvent::Listed(vec![manifest("a"), manifest("b")]));
        assert_eq!(app.quarantine, vec![manifest("a"), manifest("b")]);
        app.apply_quarantine_admin(QuarantineAdminEvent::Listed(vec![manifest("c")]));
        assert_eq!(app.quarantine, vec![manifest("c")]);
        app.apply_quarantine_admin(QuarantineAdminEvent::Failed("nope".into()));
        assert_eq!(app.quarantine, vec![manifest("c")], "a failure keeps the rows");
        assert_eq!(app.warnings, vec!["nope".to_string()]);
    }

    #[test]
    fn restored_entry_is_removed_from_list_when_fully_restored_and_kept_when_partial() {
        let mut app = StaleApp::empty(Settings::default());
        app.apply_quarantine_admin(QuarantineAdminEvent::Listed(vec![manifest("a"), manifest("b"), manifest("c")]));

        let failures = vec![Failure { what: r"C:\Apps\B".into(), error: "occupied".into() }];
        app.apply_quarantine_admin(QuarantineAdminEvent::Restored {
            id: QuarantineId("b".into()),
            outcome: RestoreOutcome::Partial(failures.clone()),
        });
        assert_eq!(app.quarantine.len(), 3, "partially restored entry is kept");
        assert_eq!(app.last_restore_failures, Some((QuarantineId("b".into()), failures)));

        app.apply_quarantine_admin(QuarantineAdminEvent::Restored {
            id: QuarantineId("b".into()),
            outcome: RestoreOutcome::Restored,
        });
        assert_eq!(app.quarantine, vec![manifest("a"), manifest("c")]);
        assert_eq!(app.last_restore_failures, None, "its failures are gone with it");

        app.apply_quarantine_admin(QuarantineAdminEvent::Purged(QuarantineId("a".into())));
        app.apply_quarantine_admin(QuarantineAdminEvent::PurgedAll);
        assert_eq!(app.quarantine, vec![manifest("c")]);
    }

    #[test]
    fn quarantine_days_setting_is_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut app = StaleApp::empty(Settings::default());
        app.set_quarantine_days_at(7, &path);
        assert_eq!(app.settings.quarantine_days, 7);
        assert_eq!(Settings::load_from(&path).quarantine_days, 7);
    }

    #[test]
    fn quarantine_admin_disconnect_clears_busy() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut app = StaleApp::empty(Settings::default());
        app.quarantine_admin_rx = Some(rx);
        app.quarantine_busy = true;
        tx.send(QuarantineAdminEvent::Listed(vec![manifest("a")])).unwrap();
        app.drain_quarantine_admin();
        assert!(app.quarantine_busy && app.quarantine_admin_rx.is_some(), "still running");
        drop(tx);
        app.drain_quarantine_admin();
        assert!(!app.quarantine_busy && app.quarantine_admin_rx.is_none());
        assert_eq!(app.quarantine.len(), 1);
    }
}

