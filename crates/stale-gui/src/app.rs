//! Application state and the eframe shell.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use chrono::{DateTime, Utc};
use eframe::egui;
use stale_core::scan::ScanEvent;
use stale_core::settings::Settings;
use stale_core::usage::matching::ProgramUsage;
use stale_core::{Program, ProgramId};

use crate::filter::{bucket_counts, visible_rows, Filters, Sort};
use crate::ui::{details, filter_panel, table, toolbar};
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

    /// Measured size, falling back to the registry estimate.
    pub fn size_bytes(&self) -> Option<u64> {
        self.size.or(self.program.estimated_size_bytes)
    }
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
    rx: Option<Receiver<ScanEvent>>,
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
            rx: None,
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
        if self.scanning {
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
        egui::CentralPanel::default().show(ui, |ui| {
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
    use stale_core::usage::matching::LastUsed;
    use stale_core::{Hive, RegKeyPath, Scope};

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
}
