//! The quarantine view: every quarantine with its size per drive, Ångra / Ta bort permanent per
//! row, and Töm karantän behind a confirmation.

use eframe::egui;
use egui_extras::{Column, TableBuilder};
use stale_core::quarantine::{Failure, QuarantineId, QuarantineManifest};

use super::STALE_COLOR;
use crate::app::{StaleApp, View};
use crate::{format, strings};

const ROW_HEIGHT: f32 = 22.0;

/// What a click in the view asks for, applied after drawing.
enum Action {
    Back,
    AskPurgeAll,
    CancelPurgeAll,
    PurgeAll,
    Restore(QuarantineId),
    Purge(QuarantineId),
}

pub fn show(ui: &mut egui::Ui, app: &mut StaleApp) {
    let busy = app.quarantine_busy;
    let mut action = None;
    ui.horizontal(|ui| {
        ui.heading(strings::QUARANTINE_TITLE);
        if ui.button(strings::BACK_TO_PROGRAMS).clicked() {
            action = Some(Action::Back);
        }
        let can_purge = !app.quarantine.is_empty() && !busy && !app.confirm_purge_all;
        if ui.add_enabled(can_purge, egui::Button::new(strings::PURGE_ALL)).clicked() {
            action = Some(Action::AskPurgeAll);
        }
        if busy {
            ui.spinner();
        }
    });
    ui.separator();
    if app.confirm_purge_all {
        confirm_window(ui, &app.quarantine, busy, &mut action);
    }
    if app.quarantine.is_empty() {
        if !busy {
            ui.label(strings::QUARANTINE_EMPTY);
        }
    } else {
        let failures = app
            .last_restore_failures
            .as_ref()
            .filter(|(id, _)| app.quarantine.iter().any(|m| &m.id == id))
            .map(|(_, f)| f);
        if let Some(failures) = failures {
            egui::Panel::bottom("restore_failures").show(ui, |ui| restore_failures(ui, failures));
        }
        table(ui, &app.quarantine, busy, &mut action);
    }
    match action {
        Some(Action::Back) => {
            app.confirm_purge_all = false;
            app.view = View::Programs;
        }
        Some(Action::AskPurgeAll) => app.confirm_purge_all = true,
        Some(Action::CancelPurgeAll) => app.confirm_purge_all = false,
        Some(Action::PurgeAll) => {
            app.confirm_purge_all = false;
            let ids = app.quarantine.iter().map(|m| m.id.clone()).collect();
            app.purge_quarantine(ids);
        }
        Some(Action::Restore(id)) => app.restore_quarantine(id),
        Some(Action::Purge(id)) => app.purge_quarantine(vec![id]),
        None => {}
    }
}

/// Program · Datum · Storlek · Status · Ångra / Ta bort permanent; the buttons wait while busy.
fn table(ui: &mut egui::Ui, rows: &[QuarantineManifest], busy: bool, action: &mut Option<Action>) {
    TableBuilder::new(ui)
        .id_salt("quarantine_table")
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::remainder().at_least(160.0).clip(true))
        .column(Column::auto())
        .column(Column::initial(180.0).clip(true))
        .column(Column::auto())
        .column(Column::auto())
        .header(ROW_HEIGHT, |mut header| {
            for title in [strings::COL_PROGRAM, strings::COL_DATE, strings::COL_SIZE, strings::COL_STATUS, ""] {
                header.col(|ui| {
                    ui.strong(title);
                });
            }
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, rows.len(), |mut row| {
                let m = &rows[row.index()];
                row.col(|ui| {
                    let name = ui.label(&m.program_name);
                    if !m.services.is_empty() {
                        name.on_hover_text(strings::SERVICE_REBOOT);
                    }
                });
                row.col(|ui| {
                    ui.label(format::ymd(Some(format::local_date(m.created))));
                });
                row.col(|ui| {
                    ui.label(format::drive_sizes(&m.size_per_drive()));
                });
                row.col(|ui| status(ui, &m.failures));
                row.col(|ui| {
                    if ui.add_enabled(!busy, egui::Button::new(strings::RESTORE)).clicked() {
                        *action = Some(Action::Restore(m.id.clone()));
                    }
                    if ui.add_enabled(!busy, egui::Button::new(strings::PURGE_ONE)).clicked() {
                        *action = Some(Action::Purge(m.id.clone()));
                    }
                });
            });
        });
}

/// "ok", or "{n} fel" in the warning colour with the failures on hover.
fn status(ui: &mut egui::Ui, failures: &[Failure]) {
    if failures.is_empty() {
        ui.label(strings::STATUS_OK);
        return;
    }
    let text = failures.iter().map(failure_text).collect::<Vec<_>>().join("\n");
    ui.colored_label(STALE_COLOR, strings::error_count(failures.len())).on_hover_text(text);
}

fn failure_text(f: &Failure) -> String {
    format!("{}: {}", f.what, f.error)
}

/// What the last partial restore could not put back.
fn restore_failures(ui: &mut egui::Ui, failures: &[Failure]) {
    ui.add_space(4.0);
    ui.label(strings::RESTORE_PARTIAL);
    egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
        for f in failures {
            ui.colored_label(STALE_COLOR, failure_text(f));
        }
    });
}

/// "Detta frigör {size} och kan inte ångras." with Avbryt / Töm.
fn confirm_window(ui: &egui::Ui, rows: &[QuarantineManifest], busy: bool, action: &mut Option<Action>) {
    let size = format::human_size(Some(rows.iter().map(|m| m.total_size()).sum()));
    egui::Window::new(strings::PURGE_ALL)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            ui.label(strings::purge_all_confirm(&size));
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(strings::CANCEL).clicked() {
                    *action = Some(Action::CancelPurgeAll);
                }
                if ui.add_enabled(!busy, egui::Button::new(strings::PURGE)).clicked() {
                    *action = Some(Action::PurgeAll);
                }
            });
        });
}
