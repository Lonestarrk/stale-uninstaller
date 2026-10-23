//! Application state and the eframe shell.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use eframe::egui;
use stale_core::ghosts::{Evidence, Ghost};
use stale_core::keep::KeepList;
use stale_core::leftovers::{LeftoverReport, ProgramHint};
use stale_core::space::SpaceEntry;
use stale_core::provenance::{InstallSource, Provenance, Signature};
use stale_core::quarantine::{plan_destinations, DestinationPlan, Failure, QuarantineId, QuarantineManifest, RestoreOutcome};
use stale_core::scan::ScanEvent;
use stale_core::session;
use stale_core::settings::Settings;
use stale_core::uninstall::{QueueItem, QueueOptions, UninstallEvent, UninstallMode};
use stale_core::usage::matching::ProgramUsage;
use stale_core::usage::UsageHit;
use stale_core::pathutil::first_path_in_command;
use stale_core::{Program, ProgramId, RegKeyPath, Scope};

use crate::filter::{bucket_counts, visible_rows, Filters, Sort};
use crate::flow::FlowState;
use crate::ghost_sel::GhostSelection;
use crate::leftover_sel::LeftoverGroup;
use crate::leftover_sel::LeftoverSelection;
use crate::space_sel::{BrowseState, SpaceSelection};
use crate::ui::{details, filter_panel, ghosts_view, leftovers_view, quarantine_view, space_view, table, toolbar, uninstall_dialog};
use crate::warning::{Msg, Warning};
use crate::worker::{BrowseResult, GhostEvent, LeftoverScanEvent, QuarantineAdminEvent, QuarantineEvent, SpaceEvent};
use crate::{strings, worker};

/// Provenance probes allowed in flight at once; clicking through the list never starts more.
pub const MAX_PROVENANCE_PROBES: usize = 4;

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

    /// File name of the subcomponent exe the date came from, when it was not the main exe.
    pub fn last_used_via(&self) -> Option<&str> {
        self.usage.as_ref().and_then(|u| u.last_used.via.as_deref())
    }

    /// Whether the last-used date comes from an automatic start.
    pub fn last_used_autostart(&self) -> bool {
        self.usage.as_ref().is_some_and(|u| u.last_used.autostart)
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
    Ghosts,
    /// "Sök rester": folders no program owns and Windows' space hogs.
    Space,
}

/// One queued program as the summary dialog shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct QueueEntry {
    pub name: String,
    pub scope: Scope,
    /// Owning account of an `OtherUser` program, when resolved.
    pub account: Option<String>,
    /// The program's uninstaller executable, from its uninstall command.
    pub uninstall_path: Option<String>,
    /// Set when other programs probably rely on this one (reason key from the core).
    pub dependency: Option<&'static str>,
    /// The user ticked "Jag förstår" for a dependency; reset whenever the dialog opens.
    pub acknowledged: bool,
}

/// The summary dialog's state before a queue starts.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingQueue {
    pub ids: Vec<ProgramId>,
    /// What the summary shows per id (same order), so the dialog does not look the programs up.
    pub entries: Vec<QueueEntry>,
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

/// A ghost kept with "Behåll": what "Ångra" puts back.
#[derive(Clone, Debug, PartialEq)]
pub struct KeptGhost {
    pub ghost: Ghost,
    /// Its row in the ghosts view, and whether it was checked.
    index: usize,
    checked: bool,
    /// Its removal choices (the details popup's check marks).
    group: LeftoverGroup,
    /// The keep list before the ghost was kept.
    keep_before: KeepList,
}

/// The space view's "Ta bort markerade…" confirmation. What it removes is built from the check
/// marks when "Ta bort" is clicked ([`StaleApp::space_confirm_selection`]).
#[derive(Clone, Debug, PartialEq)]
pub enum SpaceConfirm {
    /// The list's checked entries, as listed on a worker; `None` until the listing is done.
    List(Option<LeftoverSelection>),
    /// The drill-down window's checked children.
    Browse,
}

/// One entry of the keep list, as the "Behållna" window lists it.
#[derive(Clone, Debug, PartialEq)]
pub enum KeepEntry {
    Name(String),
    Path(PathBuf),
    Key(RegKeyPath),
}

pub struct StaleApp {
    /// The data folder was locked down at start: scans may copy signed-out accounts' hives.
    pub data_dir_secured: bool,
    pub rows: Vec<Row>,
    pub settings: Settings,
    pub scanning: bool,
    pub warnings: Vec<Warning>,
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
    /// The group whose items the leftovers popup shows.
    pub leftover_popup: Option<usize>,
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
    /// The quarantine whose "Ta bort permanent" waits for confirmation.
    pub confirm_purge_one: Option<QuarantineId>,
    /// "Ta bort markerade" waits for confirmation: files fall back to C: or space is short.
    pub removal_plan: Option<DestinationPlan>,
    /// What the last partial restore could not put back, shown under its row.
    pub last_restore_failures: Option<(QuarantineId, Vec<Failure>)>,
    /// Execution traces the last scan matched to no installed program (for the ghost scan).
    pub unmatched_hits: Vec<UsageHit>,
    /// The ghosts view's rows, once a ghost scan has finished.
    pub ghosts: Option<GhostSelection>,
    ghost_rx: Option<Receiver<GhostEvent>>,
    /// A ghost scan was asked for while the program scan ran; it starts once that is done.
    ghost_scan_waiting: bool,
    /// Bumped whenever the program list is replaced.
    programs_generation: u64,
    /// The `programs_generation` the shown (or running) ghost scan was started with.
    ghosts_generation: Option<u64>,
    /// What the user chose to keep ("Behåll"); saved to `paths::keep_list_path()`.
    pub keep: KeepList,
    /// The "Behållna" window (the keep list, editable) is open.
    pub keep_window_open: bool,
    /// The last ghost kept with "Behåll", for its "Ångra" notice.
    pub last_kept: Option<KeptGhost>,
    /// The view the leftovers view's back button returns to: `Programs`, `Ghosts` or `Space`.
    pub leftovers_origin: View,
    /// The space view's entries, once listed.
    pub space: Option<SpaceSelection>,
    space_rx: Option<Receiver<(u64, SpaceEvent)>>,
    /// Set to stop the running space scan's measuring.
    space_cancel: Option<Arc<AtomicBool>>,
    /// Bumped for every space scan; events of an older scan are dropped.
    space_generation: u64,
    /// The last space scan measured every entry.
    space_complete: bool,
    /// The space view's removal confirmation, while open.
    pub space_confirm: Option<SpaceConfirm>,
    space_confirm_rx: Option<Receiver<Result<LeftoverSelection, Warning>>>,
    /// A removal from the space list started: the list is scanned again when the view shows.
    space_rescan: bool,
    /// A leftover search that finished while the leftovers view was busy, shown once it is not.
    waiting_search: Option<(String, LeftoverReport)>,
    /// The drill-down window, when open.
    pub space_browse: Option<BrowseState>,
    browse_rx: Option<Receiver<Result<BrowseResult, Warning>>>,
    /// The ghost whose details popup is open (an index into the ghost selection).
    pub ghost_popup: Option<usize>,
    /// The ghosts view's "Ta bort markerade" confirmation is open.
    pub ghost_confirm: bool,
    /// Every program from the scan, hidden ones included.
    programs: Vec<Program>,
    /// Usage and sizes received so far, kept for hidden programs too.
    usage: HashMap<ProgramId, ProgramUsage>,
    sizes: HashMap<ProgramId, u64>,
    /// Provenance read so far, per program (cleared with the scan data).
    provenance: HashMap<ProgramId, Provenance>,
    /// Provenance probes still running, at most one per program.
    provenance_rx: HashMap<ProgramId, Receiver<Provenance>>,
}

/// The interface zoom when none is saved.
pub const DEFAULT_UI_SCALE: f32 = 1.5;
/// The sizes offered in the settings menu.
pub const UI_SCALES: [f32; 5] = [1.0, 1.25, 1.5, 1.75, 2.0];

impl StaleApp {
    /// Loads settings and starts the background scan. `data_dir_secured`: `secure_data_dir`
    /// succeeded at start (signed-out accounts' hives may then be read for "last used").
    pub fn new(cc: &eframe::CreationContext, data_dir_secured: bool) -> Self {
        crate::ui::apply_style(&cc.egui_ctx);
        let mut app = StaleApp::empty(Settings::load_from(&Settings::path()));
        cc.egui_ctx.set_zoom_factor(app.ui_scale());
        app.keep = KeepList::load_from(&stale_core::paths::keep_list_path());
        app.data_dir_secured = data_dir_secured;
        let id = session::identity();
        if session::runs_as_other_account(&id) {
            let session = id.session_user.clone().unwrap_or_default();
            app.warnings.push(Warning::OtherAccount { session, token: id.token_user.clone() });
        }
        app.rx = Some(worker::spawn_scan(app.data_dir_secured));
        app.scanning = true;
        // For the toolbar's count.
        app.refresh_quarantine();
        app
    }

    /// State with no scan running.
    pub fn empty(settings: Settings) -> Self {
        StaleApp {
            data_dir_secured: false,
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
            leftover_popup: None,
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
            confirm_purge_one: None,
            removal_plan: None,
            last_restore_failures: None,
            unmatched_hits: Vec::new(),
            ghosts: None,
            ghost_rx: None,
            ghost_scan_waiting: false,
            programs_generation: 0,
            ghosts_generation: None,
            keep: KeepList::default(),
            keep_window_open: false,
            last_kept: None,
            leftovers_origin: View::Programs,
            space: None,
            space_rx: None,
            space_cancel: None,
            space_generation: 0,
            space_complete: false,
            space_confirm: None,
            space_confirm_rx: None,
            space_rescan: false,
            waiting_search: None,
            space_browse: None,
            browse_rx: None,
            ghost_popup: None,
            ghost_confirm: false,
            programs: Vec::new(),
            usage: HashMap::new(),
            sizes: HashMap::new(),
            provenance: HashMap::new(),
            provenance_rx: HashMap::new(),
        }
    }

    /// Drops all scan results and starts a new scan.
    pub fn refresh(&mut self) {
        self.clear_scan_data();
        self.rx = Some(worker::spawn_scan(self.data_dir_secured));
        self.scanning = true;
    }

    /// Forgets programs, usage, sizes and warnings from the previous scan.
    fn clear_scan_data(&mut self) {
        self.rows.clear();
        self.programs.clear();
        self.usage.clear();
        self.sizes.clear();
        self.unmatched_hits.clear();
        self.warnings.clear();
        self.provenance.clear();
        // Dropping the receivers discards results of probes still running.
        self.provenance_rx.clear();
    }

    /// The selected program's provenance once read; `None` while it is pending.
    pub fn provenance_for(&self, id: &ProgramId) -> Option<&Provenance> {
        self.provenance.get(id)
    }

    /// Starts reading the selected program's provenance on a worker thread, once its main exe
    /// is known (usage has arrived), unless it is cached or already being read, and only while
    /// fewer than [`MAX_PROVENANCE_PROBES`] probes run.
    fn request_provenance(&mut self) {
        self.request_provenance_with(worker::spawn_provenance);
    }

    fn request_provenance_with(&mut self, spawn: impl FnOnce(Program, Option<PathBuf>) -> Receiver<Provenance>) {
        let Some(id) = self.selected.as_ref() else { return };
        if self.provenance.contains_key(id) || self.provenance_rx.contains_key(id) {
            return;
        }
        // Runs every frame: the current selection is probed as soon as a slot frees.
        if self.provenance_rx.len() >= MAX_PROVENANCE_PROBES {
            return;
        }
        let Some(row) = self.rows.iter().find(|r| &r.program.id == id) else { return };
        let Some(usage) = row.usage.as_ref() else { return };
        let rx = spawn(row.program.clone(), usage.main_exe.clone());
        self.provenance_rx.insert(id.clone(), rx);
    }

    /// Caches finished provenance probes. A probe that ended without a result caches what is
    /// known without the exe, so it is not started again.
    fn drain_provenance(&mut self) {
        let mut done = Vec::new();
        for (id, rx) in &self.provenance_rx {
            match rx.try_recv() {
                Ok(p) => done.push((id.clone(), p)),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    log::error!("provenance worker for {} ended without a result", id.0);
                    let install_date = self.programs.iter().find(|p| &p.id == id).and_then(|p| p.install_date);
                    let fallback = Provenance {
                        signature: Signature::Unknown,
                        description: None,
                        company: None,
                        install_date,
                        source: InstallSource::Unknown,
                    };
                    done.push((id.clone(), fallback));
                }
            }
        }
        for (id, p) in done {
            self.provenance_rx.remove(&id);
            self.provenance.insert(id, p);
        }
    }

    /// Folds one scan event into the state (no UI work).
    pub fn apply_event(&mut self, ev: ScanEvent) {
        match ev {
            ScanEvent::Programs(programs) => {
                self.programs = programs;
                self.programs_generation += 1;
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
            ScanEvent::UnmatchedHits(hits) => self.unmatched_hits = hits,
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
                self.warnings.push(Warning::Scan(w));
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

    /// Switches the user interface language (from the next frame on) and saves it.
    pub fn set_language(&mut self, lang: strings::Lang) {
        self.set_language_at(lang, &Settings::path());
    }

    fn set_language_at(&mut self, lang: strings::Lang, path: &Path) {
        strings::set_lang(lang);
        self.settings.language = Some(lang.code().to_string());
        if let Err(e) = self.settings.save_to(path) {
            log::warn!("could not save settings to {}: {e}", path.display());
        }
    }

    /// The interface zoom factor: the saved one, else [`DEFAULT_UI_SCALE`].
    pub fn ui_scale(&self) -> f32 {
        self.settings.ui_scale.unwrap_or(DEFAULT_UI_SCALE)
    }

    /// Saves `scale` as the interface zoom (the caller applies it to the egui context).
    pub fn set_ui_scale_at(&mut self, scale: f32, path: &Path) {
        self.settings.ui_scale = Some(scale);
        if let Err(e) = self.settings.save_to(path) {
            log::warn!("could not save settings to {}: {e}", path.display());
        }
    }

    /// Saves a zoom changed with the keyboard (Ctrl + / - / 0); true when it differed from the
    /// stored scale. Called every frame, saves only on a change. Returns the zoom to show: `zoom`
    /// kept within the valid range (0.75..=3.0), which the caller applies when it differs.
    fn sync_zoom_at(&mut self, zoom: f32, path: &Path) -> f32 {
        let clamped = ((zoom * 100.0).round() / 100.0).clamp(0.75, 3.0);
        if (clamped - self.ui_scale()).abs() >= 0.005 {
            self.set_ui_scale_at(clamped, path);
        }
        clamped
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
        let entries = ids
            .iter()
            .map(|id| match self.rows.iter().find(|r| &r.program.id == id) {
                Some(r) => QueueEntry {
                    name: r.program.name.clone(),
                    scope: r.program.scope,
                    account: r.program.account.clone(),
                    uninstall_path: r
                        .program
                        .uninstall_string
                        .as_deref()
                        .and_then(first_path_in_command)
                        .map(|p| p.display().to_string()),
                    dependency: stale_core::dependencies::is_shared_dependency(&r.program),
                    acknowledged: false,
                },
                None => QueueEntry {
                    name: id.0.clone(),
                    scope: Scope::Machine,
                    account: None,
                    uninstall_path: None,
                    dependency: None,
                    acknowledged: false,
                },
            })
            .collect();
        self.pending =
            Some(PendingQueue { ids, entries, mode, restore_point: true, run_vendor: mode == UninstallMode::Normal });
    }

    /// True while a queue has been started and has not finished.
    pub fn queue_running(&self) -> bool {
        self.flow.as_ref().is_some_and(|f| !f.finished)
    }

    /// Starts the pending queue on a worker thread and closes the summary dialog.
    pub fn start_queue(&mut self) {
        // Checked before taking it: a refused start leaves the dialog open.
        let Some(pending) = self.pending.as_ref() else { return };
        let flagged: Vec<(bool, bool)> = pending.entries.iter().map(|e| (e.dependency.is_some(), e.acknowledged)).collect();
        if !crate::flow::dependencies_acknowledged(&flagged) {
            return;
        }
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
                    match &ev {
                        UninstallEvent::Warning(w) => log::warn!("{w}"),
                        UninstallEvent::NoUninstaller { index } => {
                            log::warn!("queue item {index}: no uninstaller, forced removal")
                        }
                        _ => {}
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
                    flow.warnings.push(Warning::Fixed(Msg::QueueAborted));
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
        self.leftover_popup = None;
        self.removal_done = false;
        self.last_manifests.clear();
        self.view = View::Leftovers;
        self.leftovers_origin = View::Programs;
    }

    /// Back to where the leftovers came from (the program list or the ghosts view); a finished
    /// removal's results are dropped on the way, and after one the ghosts are scanned again.
    pub fn leave_leftovers(&mut self) {
        self.view = self.leftovers_origin;
        self.removal_plan = None;
        self.leftover_popup = None;
        if self.removal_done {
            self.leftovers = None;
            self.removal_done = false;
            self.last_manifests.clear();
            if self.leftovers_origin == View::Ghosts {
                // The ghosts just quarantined must not be offered again.
                self.ghosts = None;
                self.ghost_popup = None;
                self.refresh_ghosts();
            } else if self.leftovers_origin == View::Space {
                // The space list was dropped when the removal started (`space_rescan`); it is
                // scanned again once the space view shows, not when the user goes elsewhere.
            } else if !self.queue_running() {
                // Its reports name what was just quarantined: "Visa rester" must not offer them again.
                self.close_flow();
            }
        }
    }

    /// True while checked leftovers are being moved to quarantine.
    pub fn removal_running(&self) -> bool {
        self.quarantine_rx.is_some()
    }

    /// "Ta bort markerade": plans where the checked items will be stored (`volume_ok` and
    /// `free_bytes` are live in production) and starts the removal, unless something falls back
    /// to C: or a drive is short of space: then the confirmation opens instead.
    pub fn request_removal(&mut self, volume_ok: &dyn Fn(char) -> bool, free_bytes: &dyn Fn(char) -> Option<u64>) {
        if self.removal_running() {
            return;
        }
        let Some(sel) = &self.leftovers else { return };
        let items: Vec<_> = sel.checked_items().into_iter().flat_map(|(_, items)| items).collect();
        if items.is_empty() {
            return;
        }
        let plan = plan_destinations(&items, volume_ok, free_bytes);
        if plan.fallbacks.is_empty() && plan.insufficient.is_empty() {
            self.remove_checked_leftovers();
        } else {
            self.removal_plan = Some(plan);
        }
    }

    /// "Avbryt" in the removal confirmation: closes it, and a removal that came from the ghosts
    /// or space view goes back there, so the choices made there are not left behind.
    pub fn cancel_removal_plan(&mut self) {
        self.removal_plan = None;
        if matches!(self.leftovers_origin, View::Ghosts | View::Space) && !self.removal_running() {
            self.leave_leftovers();
        }
    }

    /// "Fortsätt" in the removal confirmation; refused while a drive is short of space.
    pub fn confirm_removal(&mut self) {
        if self.removal_plan.as_ref().is_some_and(|p| p.insufficient.is_empty()) {
            self.removal_plan = None;
            self.remove_checked_leftovers();
        }
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
        if self.leftovers_origin == View::Space {
            // The space list is out of date once its entries move: drop it and stop measuring
            // them; it is scanned again when the space view shows next.
            self.cancel_space_scan();
            self.close_browse();
            self.space = None;
            self.space_complete = false;
            self.space_rescan = true;
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
                    log::error!("{msg:?}");
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
                    self.warnings.push(Warning::Fixed(Msg::QuarantineAborted));
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
                log::error!("{msg:?}");
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

    /// True while the leftovers view is in use, or a removal is being confirmed or runs: a
    /// finished search must not replace what is about to be removed.
    fn leftovers_busy(&self) -> bool {
        self.view == View::Leftovers
            || self.removal_running()
            || self.removal_plan.is_some()
            || self.space_confirm.is_some()
            || self.ghost_confirm
    }

    /// Shows a finished search's leftovers once nothing else uses the leftovers view.
    fn show_waiting_search(&mut self) {
        if self.leftovers_busy() {
            return;
        }
        let Some((name, report)) = self.waiting_search.take() else { return };
        // Started from the space view's link: its back button returns there.
        let origin = if self.view == View::Space { View::Space } else { View::Programs };
        self.show_leftovers(LeftoverSelection::from_reports(&[(name, report)]));
        self.leftovers_origin = origin;
    }

    /// Opens the leftovers view once the standalone search is done (see
    /// [`Self::show_waiting_search`]).
    fn drain_leftover_scan(&mut self) {
        let Some(rx) = self.leftover_scan_rx.take() else { return };
        match rx.try_recv() {
            Ok(LeftoverScanEvent::Done(name, report)) => {
                self.waiting_search = Some((name, report));
                self.show_waiting_search();
            }
            Ok(LeftoverScanEvent::Failed(msg)) => {
                log::error!("{msg:?}");
                self.warnings.push(msg);
            }
            Err(TryRecvError::Empty) => self.leftover_scan_rx = Some(rx),
            Err(TryRecvError::Disconnected) => {
                log::error!("leftover scan worker disconnected before Done");
                self.warnings.push(Warning::Fixed(Msg::LeftoverScanAborted));
            }
        }
    }

    /// "Behåll": keeps ghost `key`'s name, folders and keys, saves the keep list and drops the row.
    pub fn keep_ghost(&mut self, key: &str) {
        self.keep_ghost_at(key, &stale_core::paths::keep_list_path());
    }

    fn keep_ghost_at(&mut self, key: &str, path: &Path) {
        let Some(sel) = self.ghosts.as_mut() else { return };
        let Some(index) = sel.ghosts.iter().position(|g| g.key == key) else { return };
        let Some((ghost, checked, group)) = sel.remove_by_key(key) else { return };
        self.ghost_popup = None;
        let keep_before = self.keep.clone();
        self.keep.keep_name(ghost.name.clone());
        for e in &ghost.evidence {
            match e {
                Evidence::Folder { path, .. } => self.keep.keep_path(path.clone()),
                Evidence::RegKeyMissingPath { key, .. } | Evidence::RegKeyOrphan { key, .. } => self.keep.keep_key(key.clone()),
                Evidence::DanglingRef { .. } | Evidence::UsageTrace { .. } => {}
            }
        }
        self.last_kept = Some(KeptGhost { ghost, index, checked, group, keep_before });
        if let Err(e) = self.keep.save_to(path) {
            log::warn!("could not save the keep list to {}: {e}", path.display());
            self.warnings.push(Warning::Detail(Msg::KeepSaveFailed, e.to_string()));
        }
    }

    /// "Ångra" after "Behåll": puts the keep list back as it was before (and saves it) and the
    /// ghost back in its row, checked as it was.
    pub fn undo_keep(&mut self) {
        self.undo_keep_at(&stale_core::paths::keep_list_path());
    }

    fn undo_keep_at(&mut self, path: &Path) {
        let Some(kept) = self.last_kept.take() else { return };
        self.keep = kept.keep_before;
        self.ghost_popup = None;
        self.save_keep(path);
        if let Some(sel) = self.ghosts.as_mut() {
            sel.insert(kept.index, (kept.ghost, kept.checked, kept.group));
        }
    }

    /// "Ta bort" in the "Behållna" window: forgets one keep entry and saves the list. Nothing is
    /// rescanned; the next "Uppdatera" shows what the entry hid.
    pub fn remove_keep_entry(&mut self, entry: &KeepEntry) {
        self.remove_keep_entry_at(entry, &stale_core::paths::keep_list_path());
    }

    fn remove_keep_entry_at(&mut self, entry: &KeepEntry, path: &Path) {
        match entry {
            KeepEntry::Name(n) => self.keep.remove_name(n),
            KeepEntry::Path(p) => self.keep.remove_path(p),
            KeepEntry::Key(k) => self.keep.remove_key(k),
        }
        // The undo notice would bring the removed entry back.
        self.last_kept = None;
        self.save_keep(path);
    }

    fn save_keep(&mut self, path: &Path) {
        if let Err(e) = self.keep.save_to(path) {
            log::warn!("could not save the keep list to {}: {e}", path.display());
            self.warnings.push(Warning::Detail(Msg::KeepSaveFailed, e.to_string()));
        }
    }

    /// "Ta bort" in the ghosts view's confirmation: the checked ghosts' items go through the
    /// leftovers view's removal (drive window, progress, summary); "Tillbaka" returns here.
    pub fn remove_checked_ghosts(&mut self) {
        self.remove_checked_ghosts_with(&stale_core::quarantine::live_volume_ok, &stale_core::quarantine::live_free_bytes);
    }

    fn remove_checked_ghosts_with(&mut self, volume_ok: &dyn Fn(char) -> bool, free_bytes: &dyn Fn(char) -> Option<u64>) {
        self.ghost_confirm = false;
        if self.removal_running() {
            return;
        }
        let Some(sel) = &self.ghosts else { return };
        let selection = sel.removal_selection();
        if selection.groups.is_empty() {
            return;
        }
        self.ghost_popup = None;
        self.show_leftovers(selection);
        self.leftovers_origin = View::Ghosts;
        self.request_removal(volume_ok, free_bytes);
    }

    /// "Sök rester": switches to the space view; a finished removal is closed first. The scan
    /// starts the first time, and again when the last one was stopped before it finished.
    pub fn show_space(&mut self) {
        if self.view == View::Leftovers {
            self.leave_leftovers();
        }
        self.view = View::Space;
        if !self.space_scan_running() && !self.space_complete {
            self.refresh_space();
        }
    }

    /// Stops any running space scan and starts a new one.
    pub fn refresh_space(&mut self) {
        let programs = self.programs.clone();
        self.begin_space_scan_with(|generation| worker::spawn_space_scan(generation, programs));
    }

    fn begin_space_scan_with(&mut self, spawn: impl FnOnce(u64) -> (Receiver<(u64, SpaceEvent)>, Arc<AtomicBool>)) {
        self.cancel_space_scan();
        self.space_generation += 1;
        self.space = None;
        self.space_complete = false;
        self.space_rescan = false;
        self.cancel_space_removal();
        self.close_browse();
        let (rx, cancel) = spawn(self.space_generation);
        self.space_rx = Some(rx);
        self.space_cancel = Some(cancel);
    }

    /// Stops the running space scan; what it has measured stays, what it still sends is dropped.
    pub fn cancel_space_scan(&mut self) {
        if let Some(cancel) = self.space_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.space_rx = None;
    }

    /// True while a space scan lists or measures.
    pub fn space_scan_running(&self) -> bool {
        self.space_rx.is_some()
    }

    /// The space view, or the leftovers view on its behalf, is shown.
    fn space_view_active(&self) -> bool {
        self.view == View::Space || (self.view == View::Leftovers && self.leftovers_origin == View::Space)
    }

    /// Folds one space scan event into the state; events of an older scan are dropped.
    fn apply_space_event(&mut self, generation: u64, ev: SpaceEvent) {
        if generation != self.space_generation {
            return;
        }
        match ev {
            SpaceEvent::Entries(entries) => self.space = Some(SpaceSelection::new(entries)),
            SpaceEvent::Size(i, size, partial) => {
                if let Some(sel) = self.space.as_mut() {
                    sel.set_size(i, size, partial);
                }
            }
            SpaceEvent::Done => {
                self.space_complete = true;
                self.space_rx = None;
                self.space_cancel = None;
            }
            SpaceEvent::Failed(msg) => {
                log::error!("{msg:?}");
                self.warnings.push(msg);
                self.space_rx = None;
                self.space_cancel = None;
            }
        }
    }

    /// Applies every space scan event waiting on the channel.
    fn drain_space(&mut self) {
        let Some(rx) = self.space_rx.take() else { return };
        let mut events = Vec::new();
        let gone = loop {
            match rx.try_recv() {
                Ok(ev) => events.push(ev),
                Err(TryRecvError::Empty) => break false,
                Err(TryRecvError::Disconnected) => break true,
            }
        };
        self.space_rx = Some(rx);
        for (generation, ev) in events {
            self.apply_space_event(generation, ev);
        }
        if gone && self.space_rx.is_some() {
            // Gone without `Done`: the worker panicked (logged by the panic hook).
            log::error!("space scan worker disconnected before Done");
            self.warnings.push(Warning::Fixed(Msg::SpaceScanAborted));
            self.space_rx = None;
            self.space_cancel = None;
        }
    }

    /// Opens the drill-down window on `root` and starts reading it.
    pub fn open_browse(&mut self, root: PathBuf) {
        self.space_browse = Some(BrowseState::new(root.clone()));
        self.browse_rx = Some(worker::spawn_browse(root, self.programs.clone()));
    }

    /// Shows `dir` in the drill-down window; a listing still running for another folder is
    /// dropped.
    pub fn browse_to(&mut self, dir: PathBuf) {
        let Some(b) = self.space_browse.as_mut() else { return };
        b.go_to(dir.clone());
        self.browse_rx = Some(worker::spawn_browse(dir, self.programs.clone()));
    }

    pub fn close_browse(&mut self) {
        self.space_browse = None;
        self.browse_rx = None;
    }

    /// True while the drill-down window's folder is being read.
    pub fn browse_running(&self) -> bool {
        self.browse_rx.is_some()
    }

    /// Shows the drill-down listing once it is done.
    fn drain_browse(&mut self) {
        let Some(rx) = self.browse_rx.take() else { return };
        match rx.try_recv() {
            Ok(Ok(r)) => {
                if let Some(b) = self.space_browse.as_mut() {
                    b.set_children(&r.dir, r.children, r.more);
                }
            }
            Ok(Err(msg)) => {
                log::error!("{msg:?}");
                self.warnings.push(msg);
            }
            Err(TryRecvError::Empty) => self.browse_rx = Some(rx),
            Err(TryRecvError::Disconnected) => {
                log::error!("browse worker disconnected before its result");
                self.warnings.push(Warning::Fixed(Msg::BrowseAborted));
            }
        }
    }

    /// "Ta bort markerade…" in the space view: opens the confirmation and lists what the checked
    /// entries would move on a worker (a temp folder's old files are read from disk).
    pub fn ask_remove_space(&mut self) {
        self.ask_remove_space_with(worker::spawn_space_removal_list);
    }

    fn ask_remove_space_with(&mut self, spawn: impl FnOnce(Vec<SpaceEntry>) -> Receiver<Result<LeftoverSelection, Warning>>) {
        let Some(sel) = &self.space else { return };
        let entries = sel.checked_snapshot();
        if entries.is_empty() {
            return;
        }
        self.space_confirm_rx = Some(spawn(entries));
        self.space_confirm = Some(SpaceConfirm::List(None));
    }

    /// Fills the confirmation in once the listing worker is done.
    fn drain_space_confirm(&mut self) {
        let Some(rx) = self.space_confirm_rx.take() else { return };
        let failed = match rx.try_recv() {
            Ok(Ok(listed)) => {
                if let Some(SpaceConfirm::List(slot)) = self.space_confirm.as_mut() {
                    *slot = Some(listed);
                }
                None
            }
            Ok(Err(msg)) => Some(msg),
            Err(TryRecvError::Empty) => {
                self.space_confirm_rx = Some(rx);
                None
            }
            Err(TryRecvError::Disconnected) => Some(Warning::Fixed(Msg::RemovalListAborted)),
        };
        if let Some(msg) = failed {
            log::error!("{msg:?}");
            self.warnings.push(msg);
            self.cancel_space_removal();
        }
    }

    /// "Ta bort markerade…" in the drill-down window.
    pub fn ask_remove_browse(&mut self) {
        let Some(b) = &self.space_browse else { return };
        if b.summary().0 > 0 {
            self.space_confirm = Some(SpaceConfirm::Browse);
        }
    }

    /// What "Ta bort" in the confirmation would remove now, built from the current check marks;
    /// `None` while the listing runs.
    pub fn space_confirm_selection(&self) -> Option<LeftoverSelection> {
        match self.space_confirm.as_ref()? {
            SpaceConfirm::List(listed) => Some(self.space.as_ref()?.still_checked(listed.as_ref()?)),
            SpaceConfirm::Browse => Some(self.space_browse.as_ref()?.removal_selection()),
        }
    }

    /// Closes the confirmation; a listing still running is dropped.
    pub fn cancel_space_removal(&mut self) {
        self.space_confirm = None;
        self.space_confirm_rx = None;
    }

    /// "Ta bort" in the space view's confirmation: what is checked now goes through the
    /// leftovers view's removal (drive window, progress, summary); "Tillbaka" returns to the
    /// space view.
    pub fn confirm_space_removal(&mut self) {
        self.confirm_space_removal_with(&stale_core::quarantine::live_volume_ok, &stale_core::quarantine::live_free_bytes);
    }

    fn confirm_space_removal_with(&mut self, volume_ok: &dyn Fn(char) -> bool, free_bytes: &dyn Fn(char) -> Option<u64>) {
        let selection = self.space_confirm_selection();
        self.cancel_space_removal();
        let Some(selection) = selection else { return };
        if self.removal_running() || selection.groups.is_empty() {
            return;
        }
        self.close_browse();
        self.show_leftovers(selection);
        self.leftovers_origin = View::Space;
        self.request_removal(volume_ok, free_bytes);
    }

    /// The space view's per-frame work: its workers' results, stopping it when it is left,
    /// the rescan after a removal from it, and a search result that waited.
    fn logic_space(&mut self) {
        self.drain_space();
        self.drain_browse();
        self.drain_space_confirm();
        if !self.space_view_active() {
            // Leaving the view stops the measuring and forgets the confirmation; coming back
            // starts the scan again.
            self.cancel_space_scan();
            self.close_browse();
            self.cancel_space_removal();
        } else if self.view == View::Space && self.space_rescan {
            self.refresh_space();
        }
        self.show_waiting_search();
    }

    /// True while a ghost scan runs.
    pub fn ghost_scan_running(&self) -> bool {
        self.ghost_rx.is_some()
    }

    /// Switches to the ghosts view; a finished removal is closed first. The last ghost scan's
    /// result is shown again as long as the program list has not changed since it started;
    /// otherwise (or when there is none) a ghost scan starts.
    pub fn show_ghosts(&mut self) {
        if self.view == View::Leftovers {
            self.leave_leftovers();
        }
        self.view = View::Ghosts;
        if self.ghosts_need_scan() {
            self.refresh_ghosts();
        }
    }

    /// True when there is no ghost scan result (nor one running or waiting) for the current
    /// program list.
    fn ghosts_need_scan(&self) -> bool {
        let have = self.ghosts.is_some() || self.ghost_scan_busy();
        !have || self.ghosts_generation != Some(self.programs_generation)
    }

    /// Starts a ghost scan over the installed programs, unless one runs or the program scan has
    /// not finished (without the full list every installed program would look like a ghost).
    /// While the program scan runs, the ghost scan starts once it is done; with no programs at
    /// all (the scan was aborted) it is refused with a warning.
    pub fn refresh_ghosts(&mut self) {
        if self.ghost_scan_running() {
            return;
        }
        if self.scanning {
            self.ghost_scan_waiting = true;
            return;
        }
        self.ghost_scan_waiting = false;
        if self.programs.is_empty() {
            log::warn!("ghost scan refused: no installed programs known");
            self.warnings.push(Warning::Fixed(Msg::GhostsNeedPrograms));
            return;
        }
        self.ghost_rx = Some(worker::spawn_ghost_scan(self.programs.clone(), self.unmatched_hits.clone()));
        self.ghosts_generation = Some(self.programs_generation);
    }

    /// Starts (or refuses, see [`Self::refresh_ghosts`]) a ghost scan that waited for the
    /// program scan, once that is done.
    fn start_waiting_ghost_scan(&mut self) {
        if self.ghost_scan_waiting && !self.scanning {
            self.refresh_ghosts();
        }
    }

    /// True while a ghost scan runs or waits for the program scan.
    pub fn ghost_scan_busy(&self) -> bool {
        self.ghost_scan_running() || self.ghost_scan_waiting
    }

    /// Shows the ghost scan's result once it is done.
    fn drain_ghost_scan(&mut self) {
        let Some(rx) = self.ghost_rx.take() else { return };
        match rx.try_recv() {
            Ok(GhostEvent::Done(ghosts)) => {
                log::info!("ghost scan found {} ghosts", ghosts.len());
                self.ghost_popup = None;
                self.ghost_confirm = false;
                self.ghosts = Some(GhostSelection::new(ghosts, &stale_core::paths::tasks_root()));
                self.last_kept = None;
            }
            Ok(GhostEvent::Failed(msg)) => {
                log::error!("{msg:?}");
                self.warnings.push(msg);
            }
            Err(TryRecvError::Empty) => self.ghost_rx = Some(rx),
            Err(TryRecvError::Disconnected) => {
                log::error!("ghost scan worker disconnected before Done");
                self.warnings.push(Warning::Fixed(Msg::GhostScanAborted));
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
                        self.warnings.push(Warning::Fixed(Msg::ScanAborted));
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
        self.drain_ghost_scan();
        self.start_waiting_ghost_scan();
        self.logic_space();
        self.drain_provenance();
        self.request_provenance();
        if self.scanning
            || !self.provenance_rx.is_empty()
            || self.ghost_scan_running()
            || self.space_scan_running()
            || self.browse_rx.is_some()
            || self.space_confirm_rx.is_some()
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
        let zoom = ui.ctx().zoom_factor();
        let shown = self.sync_zoom_at(zoom, &Settings::path());
        if (shown - zoom).abs() > 0.001 {
            ui.ctx().set_zoom_factor(shown);
        }
        egui::Panel::top("top").show(ui, |ui| {
            toolbar::show(ui, self);
            if !self.warnings.is_empty() {
                egui::CollapsingHeader::new(strings::t().warnings).id_salt("warnings").show(ui, |ui| {
                    for w in &self.warnings {
                        ui.label(w.text());
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
                Some(row) => details::details_panel(ui, row, self.provenance.get(&row.program.id)),
                None => {
                    ui.label(strings::t().no_selection);
                }
            }
        });
        uninstall_dialog::show(ui, self);
        leftovers_view::search_window(ui, self);
        ghosts_view::keep_window(ui, self);
        egui::CentralPanel::default().show(ui, |ui| {
            match self.view {
                View::Programs => {}
                View::Leftovers => return leftovers_view::show(ui, self),
                View::Quarantine => return quarantine_view::show(ui, self),
                View::Ghosts => return ghosts_view::show(ui, self, now),
                View::Space => return space_view::show(ui, self),
            }
            if self.rows.is_empty() {
                if !self.scanning {
                    ui.label(strings::t().no_programs);
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
    use stale_core::scan::ScanWarning;
    use stale_core::quarantine::{Failure, QuarantineId, RestoreOutcome};
    use stale_core::usage::matching::LastUsed;
    use stale_core::{Hive, RegKeyPath, Scope};

    use crate::worker::QuarantineAdminEvent;

    use super::*;

    /// Test files in a temp folder belong to whoever runs the tests: trust them.
    fn trust_all(_: &std::fs::File) -> Option<bool> {
        Some(true)
    }

    fn program(id: &str, hidden: bool) -> Program {
        Program {
            account: None,
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
        ProgramUsage { last_used: LastUsed { when: None, source: None, confident: true, via: None, autostart: false }, exes: Vec::new(), main_exe: None }
    }

    fn unknown_provenance() -> Provenance {
        Provenance {
            signature: Signature::Unknown,
            description: None,
            company: None,
            install_date: None,
            source: InstallSource::Unknown,
        }
    }

    #[test]
    fn provenance_is_probed_once_per_program_and_cleared_on_refresh() {
        use std::sync::mpsc::channel;
        let mut app = StaleApp::empty(Settings::default());
        app.apply_event(ScanEvent::Programs(vec![program("a", false)]));
        app.selected = Some(ProgramId("a".into()));
        let mut calls: Vec<(ProgramId, Option<PathBuf>)> = Vec::new();
        let (tx, rx) = channel();
        let mut rx = Some(rx);
        let mut spawn = |p: Program, exe: Option<PathBuf>| {
            calls.push((p.id, exe));
            rx.take().unwrap_or_else(|| channel().1)
        };
        // The main exe is not known before usage arrives: nothing is probed yet.
        app.request_provenance_with(&mut spawn);
        let exe = PathBuf::from(r"C:\Apps\A\a.exe");
        let mut u = usage();
        u.main_exe = Some(exe.clone());
        app.apply_event(ScanEvent::Usage(HashMap::from([(ProgramId("a".into()), u)])));
        app.request_provenance_with(&mut spawn);
        app.request_provenance_with(&mut spawn);
        assert!(app.provenance_for(&ProgramId("a".into())).is_none(), "pending");
        tx.send(unknown_provenance()).unwrap();
        app.drain_provenance();
        assert_eq!(app.provenance_for(&ProgramId("a".into())), Some(&unknown_provenance()));
        app.request_provenance_with(&mut spawn);
        assert_eq!(calls, vec![(ProgramId("a".into()), Some(exe))], "one probe per program");
        app.clear_scan_data();
        assert!(app.provenance_for(&ProgramId("a".into())).is_none());
        assert!(app.provenance_rx.is_empty());
    }

    #[test]
    fn at_most_four_provenance_probes_run_at_once() {
        use std::sync::mpsc::{channel, Sender};
        let mut app = StaleApp::empty(Settings::default());
        let ids: Vec<String> = (0..6).map(|i| format!("p{i}")).collect();
        app.apply_event(ScanEvent::Programs(ids.iter().map(|id| program(id, false)).collect()));
        app.apply_event(ScanEvent::Usage(ids.iter().map(|id| (ProgramId(id.clone()), usage())).collect()));
        let mut senders: Vec<Sender<Provenance>> = Vec::new();
        let mut spawned: Vec<ProgramId> = Vec::new();
        // The user clicks through six programs while no probe finishes.
        for id in &ids {
            app.selected = Some(ProgramId(id.clone()));
            app.request_provenance_with(|p, _| {
                spawned.push(p.id);
                let (tx, rx) = channel();
                senders.push(tx);
                rx
            });
        }
        assert_eq!(spawned.len(), MAX_PROVENANCE_PROBES);
        assert_eq!(app.provenance_rx.len(), MAX_PROVENANCE_PROBES);
        // One finishes: the next frame probes the current selection.
        senders[0].send(unknown_provenance()).unwrap();
        app.drain_provenance();
        app.request_provenance_with(|p, _| {
            spawned.push(p.id);
            channel().1
        });
        assert_eq!(spawned.last(), Some(&ProgramId("p5".into())));
        assert_eq!(spawned.len(), MAX_PROVENANCE_PROBES + 1);
    }

    #[test]
    fn a_lost_provenance_worker_is_not_respawned() {
        let mut app = StaleApp::empty(Settings::default());
        let mut p = program("a", false);
        p.install_date = chrono::NaiveDate::from_ymd_opt(2024, 1, 2);
        app.apply_event(ScanEvent::Programs(vec![p]));
        app.apply_event(ScanEvent::Usage(HashMap::from([(ProgramId("a".into()), usage())])));
        app.selected = Some(ProgramId("a".into()));
        let mut calls = 0;
        let mut spawn = |_: Program, _: Option<PathBuf>| {
            calls += 1;
            std::sync::mpsc::channel().1
        };
        app.request_provenance_with(&mut spawn);
        app.drain_provenance();
        app.request_provenance_with(&mut spawn);
        assert_eq!(calls, 1);
        let shown = app.provenance_for(&ProgramId("a".into())).expect("a fallback is cached");
        assert_eq!(shown.signature, Signature::Unknown);
        assert_eq!(shown.install_date, chrono::NaiveDate::from_ymd_opt(2024, 1, 2));
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
        app.apply_event(ScanEvent::Warning(ScanWarning::ThreadFailed { error: "w".into() }));
        let a = app.rows.iter().find(|r| r.program.id.0 == "a").unwrap();
        let b = app.rows.iter().find(|r| r.program.id.0 == "b").unwrap();
        assert_eq!(a.usage, Some(usage()));
        assert_eq!(a.size, None);
        assert_eq!(b.usage, None);
        assert_eq!(b.size, Some(1234));
        assert_eq!(app.warnings, vec![Warning::Scan(ScanWarning::ThreadFailed { error: "w".into() })]);
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
        assert_eq!(app.warnings, vec![Warning::Fixed(Msg::ScanAborted)]);
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
        tx.send(ScanEvent::Warning(ScanWarning::ThreadFailed { error: "w".into() })).unwrap();
        app.drain_events();
        assert!(app.scanning && app.rx.is_some());
        assert_eq!(app.warnings, vec![Warning::Scan(ScanWarning::ThreadFailed { error: "w".into() })]);
    }

    #[test]
    fn clear_scan_data_forgets_previous_scan() {
        let mut app = StaleApp::empty(Settings::default());
        app.apply_event(ScanEvent::Programs(vec![program("a", false)]));
        app.apply_event(ScanEvent::Sizes(HashMap::from([(ProgramId("a".into()), 7)])));
        app.apply_event(ScanEvent::Warning(ScanWarning::ThreadFailed { error: "w".into() }));
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
    fn dependency_needs_acknowledgement_also_when_forced_and_resets_on_reopen() {
        let mut app = StaleApp::empty(Settings::default());
        let mut git = program("g", false);
        git.name = "Git".into();
        app.apply_event(ScanEvent::Programs(vec![program("m", false), git]));
        app.open_queue_dialog_for(vec![ProgramId("m".into()), ProgramId("g".into())], UninstallMode::Forced);
        let mut pending = app.pending.take().expect("dialog opened");
        assert_eq!(pending.entries[0].dependency, None);
        assert_eq!(pending.entries[1].dependency, Some("toolchain"));
        pending.entries[1].acknowledged = true;
        app.pending = Some(pending);
        app.open_queue_dialog_for(vec![ProgramId("g".into())], UninstallMode::Forced);
        assert!(!app.pending.as_ref().unwrap().entries[0].acknowledged);
        // Starting without the acknowledgement does nothing, and the dialog stays open.
        app.start_queue();
        assert!(app.flow.is_none());
        assert!(app.pending.is_some(), "a rejected start keeps the dialog");
    }

    #[test]
    fn pending_queue_carries_scope_and_account() {
        let mut app = StaleApp::empty(Settings::default());
        let mut other = program("o", false);
        other.scope = Scope::OtherUser;
        other.account = Some(r"PC\anna".into());
        other.uninstall_string = Some(r#""C:\Users\anna\AppData\Local\X\unins000.exe" /S"#.into());
        app.apply_event(ScanEvent::Programs(vec![program("m", false), other]));
        app.open_queue_dialog_for(vec![ProgramId("m".into()), ProgramId("o".into())], UninstallMode::Normal);
        let entries = app.pending.expect("dialog opened").entries;
        assert_eq!((entries[0].scope, entries[0].account.as_deref(), entries[0].uninstall_path.as_deref()), (Scope::Machine, None, None));
        assert_eq!(entries[1].name, "App o");
        assert_eq!(entries[1].scope, Scope::OtherUser);
        assert_eq!(entries[1].account.as_deref(), Some(r"PC\anna"));
        assert_eq!(entries[1].uninstall_path.as_deref(), Some(r"C:\Users\anna\AppData\Local\X\unins000.exe"));
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
        assert!(Settings::load_from_with(&path, &trust_all).show_hidden, "setting is saved");
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

    #[test]
    fn removal_with_fallback_or_short_space_asks_first() {
        use stale_core::leftovers::{Confidence, Leftover, LeftoverKind, LeftoverReport};
        let on = |path: &str, size| Leftover {
            kind: LeftoverKind::Dir { path: path.into(), size: Some(size) },
            confidence: Confidence::High,
            note: None,
        };
        let mut app = StaleApp::empty(Settings::default());
        let report = LeftoverReport { items: vec![on(r"E:\Foo", 30), on(r"C:\Apps\Foo", 7)] };
        app.leftovers = Some(LeftoverSelection::from_reports(&[("A".into(), report)]));

        app.request_removal(&|d| d != 'E', &|_| Some(10));
        let plan = app.removal_plan.clone().expect("confirmation open");
        assert_eq!((plan.fallbacks, plan.insufficient), (vec![('E', 30)], vec!['C']));
        app.confirm_removal();
        assert!(app.removal_plan.is_some() && app.quarantine_rx.is_none(), "blocked while space is short");
        app.leave_leftovers();
        assert!(app.removal_plan.is_none());

        app.request_removal(&|d| d != 'E', &|_| Some(1_000));
        assert_eq!(app.removal_plan.as_ref().map(|p| p.fallbacks.clone()), Some(vec![('E', 30)]));
        assert!(app.quarantine_rx.is_none(), "nothing starts before the confirmation");
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
        app.apply_quarantine_admin(QuarantineAdminEvent::Failed(Warning::Fixed(Msg::QuarantineAdminAborted)));
        assert_eq!(app.quarantine, vec![manifest("c")], "a failure keeps the rows");
        assert_eq!(app.warnings, vec![Warning::Fixed(Msg::QuarantineAdminAborted)]);
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
        assert_eq!(Settings::load_from_with(&path, &trust_all).quarantine_days, 7);
    }

    #[test]
    fn ui_scale_defaults_to_150_percent() {
        let app = StaleApp::empty(Settings::default());
        assert_eq!(app.ui_scale(), 1.5);
        assert!(UI_SCALES.contains(&DEFAULT_UI_SCALE));
    }

    #[test]
    fn set_ui_scale_saves() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut app = StaleApp::empty(Settings::default());
        app.set_ui_scale_at(1.25, &path);
        assert_eq!(app.ui_scale(), 1.25);
        assert_eq!(Settings::load_from_with(&path, &trust_all).ui_scale, Some(1.25));
    }

    #[test]
    fn keyboard_zoom_is_saved_once_per_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut app = StaleApp::empty(Settings::default());
        app.sync_zoom_at(1.5, &path);
        assert!(!path.exists(), "unchanged: nothing saved");
        app.sync_zoom_at(1.6000001, &path);
        assert_eq!(Settings::load_from_with(&path, &trust_all).ui_scale, Some(1.6));
        std::fs::remove_file(&path).unwrap();
        app.sync_zoom_at(1.6000001, &path);
        assert!(!path.exists(), "same value again is not saved again");
    }

    #[test]
    fn keyboard_zoom_is_clamped_to_the_valid_range() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut app = StaleApp::empty(Settings::default());
        assert_eq!(app.sync_zoom_at(5.0, &path), 3.0);
        assert_eq!(Settings::load_from_with(&path, &trust_all).ui_scale, Some(3.0));
        assert_eq!(app.sync_zoom_at(0.2, &path), 0.75);
        assert_eq!(Settings::load_from_with(&path, &trust_all).ui_scale, Some(0.75));
        assert_eq!(app.sync_zoom_at(1.0, &path), 1.0);
        assert_eq!(Settings::load_from_with(&path, &trust_all).ui_scale, Some(1.0));
    }

    /// Swedish only: the language is process-wide and other tests running in parallel expect
    /// Swedish text.
    #[test]
    fn chosen_language_is_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut app = StaleApp::empty(Settings::default());
        app.set_language_at(strings::Lang::Sv, &path);
        assert_eq!(app.settings.language.as_deref(), Some("sv"));
        assert_eq!(Settings::load_from_with(&path, &trust_all).language.as_deref(), Some("sv"));
        assert_eq!(strings::lang(), strings::Lang::Sv);
    }

    #[test]
    fn unmatched_hits_are_stored() {
        use stale_core::usage::UsageSource;
        let hit = UsageHit {
            exe_path: r"C:\Gone\gone.exe".into(),
            last_run: DateTime::from_timestamp(1_792_000_000, 0).unwrap(),
            source: UsageSource::Prefetch,
            run_count: Some(3),
        };
        let mut app = StaleApp::empty(Settings::default());
        app.apply_event(ScanEvent::UnmatchedHits(vec![hit.clone()]));
        assert_eq!(app.unmatched_hits, vec![hit]);
        app.clear_scan_data();
        assert!(app.unmatched_hits.is_empty(), "a new scan starts without the old hits");
    }

    #[test]
    fn show_ghosts_reuses_the_cached_scan_until_programs_change() {
        use crate::ghost_sel::tests::ghost;
        use stale_core::ghosts::Assessment;
        let mut app = StaleApp::empty(Settings::default());
        app.apply_event(ScanEvent::Programs(vec![program("a", false)]));
        app.ghosts = Some(crate::ghost_sel::tests::selection_of(vec![ghost("Alpha", Assessment::LeftoversOnly, 1)]));
        app.ghosts_generation = Some(app.programs_generation);
        app.show_ghosts();
        assert_eq!(app.view, View::Ghosts);
        assert!(app.ghost_rx.is_none(), "the cached result is shown again");
        assert_eq!(app.ghosts.as_ref().unwrap().ghosts.len(), 1);

        assert!(!app.ghosts_need_scan());
        // A new program list (Uppdatera) means a new ghost scan; not started here, it would read
        // the real machine.
        app.apply_event(ScanEvent::Programs(vec![program("a", false), program("b", false)]));
        assert!(app.ghosts_need_scan());
        app.ghosts = None;
        app.ghosts_generation = Some(app.programs_generation);
        assert!(app.ghosts_need_scan(), "no result yet");
    }

    #[test]
    fn undo_keep_restores_ghost() {
        use crate::ghost_sel::tests::ghost;
        use stale_core::ghosts::Assessment;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("important.json");
        let mut app = StaleApp::empty(Settings::default());
        app.keep.keep_name("Earlier".into());
        app.ghosts = Some(crate::ghost_sel::tests::selection_of(vec![
            ghost("Alpha", Assessment::LeftoversOnly, 1),
            ghost("Beta", Assessment::ProbablyUninstalled, 2),
            ghost("Gamma", Assessment::LeftoversOnly, 3),
        ]));
        app.keep_ghost_at("beta", &path);
        assert!(app.keep.is_name_kept("Beta"));
        assert_eq!(app.last_kept.as_ref().map(|k| k.ghost.name.as_str()), Some("Beta"));

        app.undo_keep_at(&path);
        let sel = app.ghosts.as_ref().unwrap();
        assert_eq!(sel.ghosts.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), vec!["Alpha", "Beta", "Gamma"]);
        assert_eq!(sel.checked, vec![false, true, false], "back in its row, checked as before");
        assert_eq!(app.keep.names, vec!["Earlier".to_string()], "the keep entries are reverted");
        assert!(app.keep.paths.is_empty() && app.keep.keys.is_empty());
        assert_eq!(KeepList::load_from_with(&path, &trust_all), app.keep, "and saved");
        assert!(app.last_kept.is_none());
        app.undo_keep_at(&path);
        assert_eq!(app.ghosts.as_ref().unwrap().ghosts.len(), 3, "a second undo does nothing");
    }

    #[test]
    fn remove_keep_entry_saves_and_rescans_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("important.json");
        let mut app = StaleApp::empty(Settings::default());
        let key = RegKeyPath { hive: Hive::LocalMachine, path: r"SOFTWARE\Alpha".into() };
        app.keep.keep_name("Alpha".into());
        app.keep.keep_path(PathBuf::from(r"C:\Program Files\Alpha"));
        app.keep.keep_key(key.clone());
        app.programs = vec![program("installed", false)];
        app.remove_keep_entry_at(&KeepEntry::Path(PathBuf::from(r"c:\program files\alpha")), &path);
        app.remove_keep_entry_at(&KeepEntry::Key(key), &path);
        assert_eq!(app.keep.names, vec!["Alpha".to_string()]);
        assert!(app.keep.paths.is_empty() && app.keep.keys.is_empty());
        assert_eq!(KeepList::load_from_with(&path, &trust_all), app.keep);
        app.remove_keep_entry_at(&KeepEntry::Name("alpha".into()), &path);
        assert_eq!(KeepList::load_from_with(&path, &trust_all), KeepList::default());
        assert!(app.ghost_rx.is_none() && !app.ghost_scan_busy(), "nothing is rescanned");
    }

    #[test]
    fn keep_ghost_persists_name_and_paths_and_removes_row() {
        use crate::ghost_sel::tests::ghost;
        use stale_core::ghosts::{Assessment, RefKind};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("important.json");
        let mut alpha = ghost("Alpha", Assessment::ProbablyUninstalled, 100);
        let missing = RegKeyPath { hive: Hive::CurrentUser, path: r"Software\Alpha".into() };
        alpha.evidence.push(Evidence::RegKeyMissingPath { key: missing.clone(), value_name: "Path".into(), path: r"D:\Alpha".into(), shared: false });
        alpha.evidence.push(Evidence::DanglingRef {
            kind: RefKind::Run,
            location: r"LocalMachine\SOFTWARE\Microsoft\Windows\CurrentVersion\Run".into(),
            target: r"C:\Program Files\Alpha.exe".into(),
            value_name: Some("Alpha".into()),
            partly_live: false,
        });
        let mut app = StaleApp::empty(Settings::default());
        app.ghosts = Some(crate::ghost_sel::tests::selection_of(vec![alpha, ghost("Beta", Assessment::LeftoversOnly, 1)]));

        app.keep_ghost_at("alpha", &path);
        let saved = KeepList::load_from_with(&path, &trust_all);
        assert_eq!(saved, app.keep);
        assert_eq!(saved.names, vec!["Alpha".to_string()]);
        assert_eq!(saved.paths, vec![PathBuf::from(r"C:\Program Files\Alpha")]);
        assert_eq!(saved.keys, vec![RegKeyPath { hive: Hive::LocalMachine, path: r"SOFTWARE\Alpha".into() }, missing]);
        let names: Vec<&str> = app.ghosts.as_ref().unwrap().ghosts.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, vec!["Beta"]);

        app.keep_ghost_at("missing", &path);
        assert_eq!(KeepList::load_from_with(&path, &trust_all), saved, "an unknown key changes nothing");
    }

    #[test]
    fn ghost_removal_cancel_returns_to_ghosts() {
        use crate::ghost_sel::tests::ghost;
        use stale_core::ghosts::Assessment;
        let mut app = StaleApp::empty(Settings::default());
        app.view = View::Ghosts;
        let mut alpha = ghost("Alpha", Assessment::ProbablyUninstalled, 100);
        if let Evidence::Folder { path, .. } = &mut alpha.evidence[0] {
            *path = PathBuf::from(r"E:\Alpha");
        }
        app.ghosts = Some(crate::ghost_sel::tests::selection_of(vec![alpha]));
        app.remove_checked_ghosts_with(&|d| d != 'E', &|_| Some(1_000_000));
        assert!(app.removal_plan.is_some());
        app.cancel_removal_plan();
        assert_eq!(app.view, View::Ghosts);
        assert!(app.removal_plan.is_none() && !app.removal_running());
    }

    #[test]
    fn keep_and_undo_close_the_ghost_popup() {
        use crate::ghost_sel::tests::ghost;
        use stale_core::ghosts::Assessment;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("important.json");
        let mut app = StaleApp::empty(Settings::default());
        app.ghosts = Some(crate::ghost_sel::tests::selection_of(vec![
            ghost("Alpha", Assessment::LeftoversOnly, 1),
            ghost("Beta", Assessment::LeftoversOnly, 2),
        ]));
        app.ghost_popup = Some(1);
        app.keep_ghost_at("alpha", &path);
        assert_eq!(app.ghost_popup, None);
        app.ghost_popup = Some(0);
        app.undo_keep_at(&path);
        assert_eq!(app.ghost_popup, None);
    }

    #[test]
    fn remove_checked_ghosts_opens_leftovers_with_origin_ghosts() {
        use crate::ghost_sel::tests::ghost;
        use stale_core::ghosts::Assessment;
        let mut app = StaleApp::empty(Settings::default());
        app.view = View::Ghosts;
        app.ghost_confirm = true;
        // Alpha's folder is on E:, which cannot hold a payload: the drive window opens, so the
        // test never starts a real removal.
        let mut alpha = ghost("Alpha", Assessment::ProbablyUninstalled, 100);
        if let Evidence::Folder { path, .. } = &mut alpha.evidence[0] {
            *path = PathBuf::from(r"E:\Alpha");
        }
        app.ghosts = Some(crate::ghost_sel::tests::selection_of(vec![
            alpha,
            ghost("Beta", Assessment::Uncertain, 50),
            ghost("Gamma", Assessment::LeftoversOnly, 7),
        ]));
        // Leftovers-only ghosts start unchecked; the user ticks Gamma.
        app.ghosts.as_mut().unwrap().checked[2] = true;
        app.remove_checked_ghosts_with(&|d| d != 'E', &|_| Some(1_000_000));
        assert!(!app.ghost_confirm);
        assert_eq!((app.view, app.leftovers_origin), (View::Leftovers, View::Ghosts));
        let sel = app.leftovers.as_ref().expect("selection built");
        let groups: Vec<(&str, usize)> = sel.groups.iter().map(|g| (g.program_name.as_str(), g.items.len())).collect();
        assert_eq!(groups, vec![("Alpha", 2), ("Gamma", 2)]);
        assert!(app.removal_plan.is_some() && app.quarantine_rx.is_none(), "the drive window comes before the removal");

        // Nothing to remove: the view stays where it is.
        let mut none = StaleApp::empty(Settings::default());
        none.view = View::Ghosts;
        none.ghosts = Some(crate::ghost_sel::tests::selection_of(vec![ghost("Beta", Assessment::Uncertain, 50)]));
        none.remove_checked_ghosts_with(&|_| true, &|_| Some(1_000));
        assert_eq!(none.view, View::Ghosts);

        app.leave_leftovers();
        assert_eq!(app.view, View::Ghosts, "back returns to the ghosts view");
        assert!(app.ghost_rx.is_none(), "no rescan without a removal");

        app.flow = Some(FlowState::new(&[]));
        app.open_leftovers_from_flow();
        assert_eq!(app.leftovers_origin, View::Programs, "the uninstall flow's leftovers return to the list");
    }

    #[test]
    fn refresh_ghosts_refuses_without_programs() {
        let mut app = StaleApp::empty(Settings::default());
        app.refresh_ghosts();
        assert!(app.ghost_rx.is_none() && !app.ghost_scan_busy());
        assert_eq!(app.warnings, vec![Warning::Fixed(Msg::GhostsNeedPrograms)]);
    }

    #[test]
    fn ghost_scan_waits_while_scanning() {
        let mut app = StaleApp::empty(Settings::default());
        app.scanning = true;
        app.show_ghosts();
        assert_eq!(app.view, View::Ghosts);
        assert!(app.ghost_scan_busy() && app.ghost_rx.is_none(), "waits for the program scan");
        // The program scan ends without programs: the waiting scan is refused, not started.
        app.start_waiting_ghost_scan();
        assert!(app.ghost_scan_busy(), "still scanning: keeps waiting");
        app.apply_event(ScanEvent::Done);
        app.start_waiting_ghost_scan();
        assert!(!app.ghost_scan_busy() && app.ghost_rx.is_none());
        assert_eq!(app.warnings, vec![Warning::Fixed(Msg::GhostsNeedPrograms)]);
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

    /// Starts a space scan whose events the test sends itself; returns the sender and the
    /// scan's cancel flag.
    fn fake_space_scan(app: &mut StaleApp) -> (std::sync::mpsc::Sender<(u64, SpaceEvent)>, Arc<AtomicBool>, u64) {
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut generation = 0;
        app.begin_space_scan_with(|g| {
            generation = g;
            (rx, cancel.clone())
        });
        (tx, cancel, generation)
    }

    #[test]
    fn cancelled_scan_drops_late_sizes() {
        use crate::space_sel::tests::entry;
        use stale_core::space::SpaceKind;
        use std::sync::atomic::Ordering;
        let mut app = StaleApp::empty(Settings::default());
        app.view = View::Space;
        let (tx1, cancel1, g1) = fake_space_scan(&mut app);
        let entries = vec![entry(r"D:\A", SpaceKind::DriveRoot, None), entry(r"D:\B", SpaceKind::DriveRoot, None)];
        tx1.send((g1, SpaceEvent::Entries(entries.clone()))).unwrap();
        tx1.send((g1, SpaceEvent::Size(0, Some(5), false))).unwrap();
        app.drain_space();
        assert_eq!(app.space.as_ref().unwrap().entries[0].size, Some(5));
        assert!(app.space_scan_running());

        // Uppdatera: the old scan is told to stop and its late sizes are dropped.
        let (tx2, cancel2, g2) = fake_space_scan(&mut app);
        assert!(cancel1.load(Ordering::SeqCst) && !cancel2.load(Ordering::SeqCst));
        assert!(g2 > g1 && app.space.is_none());
        assert!(tx1.send((g1, SpaceEvent::Size(1, Some(99), false))).is_err(), "the old receiver is gone");
        tx2.send((g2, SpaceEvent::Entries(entries))).unwrap();
        // A late event of the old scan on the current channel is dropped too.
        tx2.send((g1, SpaceEvent::Size(1, Some(99), false))).unwrap();
        tx2.send((g1, SpaceEvent::Done)).unwrap();
        tx2.send((g2, SpaceEvent::Size(0, Some(7), true))).unwrap();
        app.drain_space();
        let sel = app.space.as_ref().unwrap();
        assert_eq!(sel.entries.iter().map(|e| e.size).collect::<Vec<_>>(), vec![Some(7), None]);
        assert!(sel.entries[0].partial);
        assert!(app.space_scan_running() && !app.space_complete, "the old Done ends nothing");

        // Leaving the view stops the measuring; what was measured stays.
        app.view = View::Programs;
        assert!(!app.space_view_active());
        app.cancel_space_scan();
        assert!(cancel2.load(Ordering::SeqCst) && !app.space_scan_running());
        assert_eq!(app.space.as_ref().unwrap().entries[0].size, Some(7));
        assert!(app.warnings.is_empty());
    }

    #[test]
    fn space_scan_done_and_lost_worker() {
        let mut app = StaleApp::empty(Settings::default());
        let (tx, _, g) = fake_space_scan(&mut app);
        tx.send((g, SpaceEvent::Entries(Vec::new()))).unwrap();
        tx.send((g, SpaceEvent::Done)).unwrap();
        drop(tx);
        app.drain_space();
        assert!(app.space_complete && !app.space_scan_running());
        assert!(app.warnings.is_empty(), "a disconnect after Done is no failure");
        let (tx, _, _) = fake_space_scan(&mut app);
        drop(tx);
        app.drain_space();
        assert!(!app.space_scan_running());
        assert_eq!(app.warnings, vec![Warning::Fixed(Msg::SpaceScanAborted)]);
    }

    #[test]
    fn removal_from_space_uses_leftovers_flow_with_origin_space() {
        use crate::space_sel::tests::entry;
        use stale_core::fsys::FakeFs;
        use stale_core::space::{SpaceKind, WinItem};
        let mut app = StaleApp::empty(Settings::default());
        app.view = View::Space;
        let (_tx, _, _) = fake_space_scan(&mut app);
        let mut sel = SpaceSelection::new(vec![
            entry(r"E:\Foton", SpaceKind::DriveRoot, Some(30)),
            entry(r"C:\Windows.old", SpaceKind::Windows(WinItem::WindowsOld), Some(70)),
        ]);
        sel.toggle(0);
        sel.toggle(1);
        app.space = Some(sel);
        ask_with_listing(&mut app, &FakeFs::new().dir(r"E:\Foton", 30));
        assert_eq!(app.space_confirm_selection().map(|s| s.summary()), Some((1, 1, 30)));
        app.space_browse = Some(BrowseState::new(PathBuf::from(r"E:\Foton")));
        // E: cannot hold a payload: the drive window opens, so no real removal starts.
        app.confirm_space_removal_with(&|d| d != 'E', &|_| Some(1_000_000));
        assert_eq!((app.view, app.leftovers_origin), (View::Leftovers, View::Space));
        assert!(app.space_confirm.is_none() && app.space_browse.is_none());
        let groups: Vec<(&str, usize)> =
            app.leftovers.as_ref().unwrap().groups.iter().map(|g| (g.program_name.as_str(), g.items.len())).collect();
        assert_eq!(groups, vec![(r"E:\Foton", 1)], "Windows.old never reaches the quarantine");
        assert!(app.removal_plan.is_some() && app.quarantine_rx.is_none(), "the drive window comes before the removal");
        assert!(app.space_view_active(), "the scan keeps measuring meanwhile");
        assert!(app.space_scan_running());

        app.cancel_removal_plan();
        assert_eq!(app.view, View::Space, "Avbryt returns to the space view");
        assert!(app.space.as_ref().unwrap().checked[0], "with the check marks kept");

        // No confirmation open: nothing happens.
        app.confirm_space_removal_with(&|_| true, &|_| Some(1_000));
        assert_eq!(app.view, View::Space);
    }

    /// "Ta bort markerade…" in the space list, with the listing worker replaced by `fs` and
    /// its result already received.
    fn ask_with_listing(app: &mut StaleApp, fs: &stale_core::fsys::FakeFs) {
        app.ask_remove_space_with(|entries| {
            let (tx, rx) = std::sync::mpsc::channel();
            tx.send(Ok(crate::space_sel::removal_selection_of(fs, &entries, Utc::now()))).unwrap();
            rx
        });
        assert_eq!(app.space_confirm, Some(SpaceConfirm::List(None)), "a spinner until listed");
        app.drain_space_confirm();
    }

    #[test]
    fn unchecked_while_confirming_is_not_removed() {
        use crate::space_sel::tests::entry;
        use stale_core::fsys::FakeFs;
        use stale_core::space::SpaceKind;
        let mut app = StaleApp::empty(Settings::default());
        app.view = View::Space;
        let mut sel = SpaceSelection::new(vec![entry(r"E:\Foton", SpaceKind::DriveRoot, Some(200)), entry(r"E:\ISO", SpaceKind::DriveRoot, Some(5))]);
        sel.toggle(0);
        app.space = Some(sel);
        ask_with_listing(&mut app, &FakeFs::new().dir(r"E:\Foton", 200).dir(r"E:\ISO", 5));
        // The user unchecks Foton and checks ISO behind the open confirmation.
        let sel = app.space.as_mut().unwrap();
        sel.toggle(0);
        sel.toggle(1);
        assert_eq!(app.space_confirm_selection().map(|s| s.groups.len()), Some(0));
        app.confirm_space_removal_with(&|_| true, &|_| Some(1_000_000));
        assert_eq!(app.view, View::Space, "nothing is removed");
        assert!(app.leftovers.is_none() && app.quarantine_rx.is_none() && app.space_confirm.is_none());

        // The drill-down: its confirmation also reads the check marks at "Ta bort".
        let mut b = BrowseState::new(PathBuf::from(r"E:\Data"));
        b.set_children(
            std::path::Path::new(r"E:\Data"),
            vec![stale_core::space::Child { path: PathBuf::from(r"E:\Data\a.bin"), is_dir: false, size: Some(1), partial: false, blocked: None }],
            0,
        );
        b.toggle(0);
        app.space_browse = Some(b);
        app.ask_remove_browse();
        assert_eq!(app.space_confirm, Some(SpaceConfirm::Browse));
        app.space_browse.as_mut().unwrap().toggle(0);
        app.confirm_space_removal_with(&|_| true, &|_| Some(1_000_000));
        assert_eq!(app.view, View::Space);
        assert!(app.leftovers.is_none());
    }

    #[test]
    fn leaving_the_space_view_forgets_the_confirmation() {
        use crate::space_sel::tests::entry;
        use stale_core::space::SpaceKind;
        let mut app = StaleApp::empty(Settings::default());
        app.view = View::Space;
        let mut sel = SpaceSelection::new(vec![entry(r"E:\Foton", SpaceKind::DriveRoot, Some(200))]);
        sel.toggle(0);
        app.space = Some(sel);
        // The listing is still running.
        let (_tx, rx) = std::sync::mpsc::channel();
        app.ask_remove_space_with(|_| rx);
        assert_eq!(app.space_confirm_selection(), None);
        app.view = View::Programs;
        app.logic_space();
        assert!(app.space_confirm.is_none() && app.space_confirm_rx.is_none());
    }

    #[test]
    fn a_search_finishing_during_a_removal_waits() {
        use stale_core::leftovers::{Confidence, Leftover, LeftoverKind};
        let dir = |p: &str| Leftover { kind: LeftoverKind::Dir { path: p.into(), size: Some(1) }, confidence: Confidence::High, note: None };
        let mut app = StaleApp::empty(Settings::default());
        app.view = View::Leftovers;
        app.leftovers_origin = View::Space;
        app.leftovers = Some(LeftoverSelection::from_reports(&[(r"E:\Foton".into(), LeftoverReport { items: vec![dir(r"E:\Foton")] })]));
        app.removal_plan = Some(plan_destinations(&[], &|_| true, &|_| None));
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(LeftoverScanEvent::Done("Foo".into(), LeftoverReport { items: vec![dir(r"C:\Foo")] })).unwrap();
        app.leftover_scan_rx = Some(rx);
        app.drain_leftover_scan();
        assert!(!app.leftover_search_running());
        assert_eq!(app.leftovers.as_ref().unwrap().groups[0].program_name, r"E:\Foton", "the removal's items stay");
        assert_eq!(app.leftovers_origin, View::Space);
        // Avbryt in the drive window goes back to the space view; then the search shows.
        app.cancel_removal_plan();
        assert_eq!(app.view, View::Space);
        app.show_waiting_search();
        assert_eq!(app.view, View::Leftovers);
        assert_eq!(app.leftovers.as_ref().unwrap().groups[0].program_name, "Foo");
        assert_eq!(app.leftovers_origin, View::Space);
        assert!(app.waiting_search.is_none());
    }
}

