//! Application state and the eframe shell.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use eframe::egui;
use stale_core::scan::ScanEvent;
use stale_core::settings::Settings;
use stale_core::usage::matching::ProgramUsage;
use stale_core::{Program, ProgramId};

use crate::{strings, worker};

pub struct Row {
    pub program: Program,
    pub usage: Option<ProgramUsage>,
    pub size: Option<u64>,
}

pub struct StaleApp {
    pub rows: Vec<Row>,
    pub settings: Settings,
    pub scanning: bool,
    pub warnings: Vec<String>,
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
            rx: None,
            programs: Vec::new(),
            usage: HashMap::new(),
            sizes: HashMap::new(),
        }
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
                    // The worker is gone; stop waiting even if it died before `Done`.
                    self.scanning = false;
                    return;
                }
            }
        }
    }

    fn show_programs(&self, ui: &mut egui::Ui) {
        if self.rows.is_empty() {
            if !self.scanning {
                ui.label(strings::NO_PROGRAMS);
            }
            return;
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("programs").striped(true).show(ui, |ui| {
                for row in &self.rows {
                    match row.program.drive {
                        Some(d) => ui.label(d.to_string()),
                        None => ui.label(strings::UNKNOWN_DRIVE),
                    };
                    ui.label(&row.program.name);
                    ui.end_row();
                }
            });
        });
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
        egui::Panel::top("top").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(strings::APP_TITLE);
                if self.scanning {
                    ui.spinner();
                    ui.label(strings::LOADING);
                }
            });
            if !self.warnings.is_empty() {
                ui.collapsing(strings::WARNINGS, |ui| {
                    for w in &self.warnings {
                        ui.label(w);
                    }
                });
            }
        });
        egui::CentralPanel::default().show(ui, |ui| self.show_programs(ui));
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
}
