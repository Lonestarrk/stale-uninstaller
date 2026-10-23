//! The "Sök rester" view: folders and files no installed program owns and Windows' own space
//! hogs, one compact row each, largest first, with sizes filling in as they are measured. A click
//! checks a row, a double-click opens it in the drill-down window, and "Ta bort markerade…" moves
//! the checked ones to the quarantine through the leftovers view. Windows' space hogs have no
//! check box: they offer Windows' own cleanup instead.

use std::path::Path;

use eframe::egui;

use super::leftovers_view::{compact_row, row_toggles};
use super::space_browse;
use crate::app::{SearchDialog, StaleApp, View};
use crate::space_sel::{space_row_in, SpaceRow};
use crate::{format, strings};

/// What a click in the view asks for, applied after drawing.
enum Action {
    Back,
    Refresh,
    ManualSearch,
    Open(usize),
    WindowsCleanup,
    AskRemove,
    CancelRemove,
    ConfirmRemove,
}

/// The height `compact_row` gives a row.
const ROW_HEIGHT: f32 = 24.0;

pub fn show(ui: &mut egui::Ui, app: &mut StaleApp) {
    let removing = app.removal_running();
    let scanning = app.space_scan_running();
    let mut action = None;
    ui.horizontal(|ui| {
        ui.heading(strings::t().search_title);
        if ui.button(strings::t().back_to_programs).clicked() {
            action = Some(Action::Back);
        }
        if ui.add_enabled(!removing, egui::Button::new(strings::t().refresh)).clicked() {
            action = Some(Action::Refresh);
        }
        let can_search = app.search.is_none() && !app.leftover_search_running() && !removing;
        if ui.add_enabled(can_search, egui::Link::new(strings::t().space_manual_link)).clicked() {
            action = Some(Action::ManualSearch);
        }
    });
    ui.separator();
    match app.space.as_mut() {
        None if scanning => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(strings::t().space_listing);
            });
        }
        None => {
            ui.label(strings::t().space_empty);
        }
        Some(sel) => {
            let (checked, bytes) = sel.summary();
            egui::Panel::bottom("space_footer").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if scanning {
                        ui.spinner();
                        ui.label(strings::space_measuring(sel.measured_count(), sel.entries.len()));
                    }
                    ui.label(strings::space_summary(checked, &format::human_size(Some(bytes))));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let asking = app.space_pending_removal.is_some();
                        if ui.add_enabled(checked > 0 && !removing && !asking, egui::Button::new(strings::t().remove_ghosts)).clicked() {
                            action = Some(Action::AskRemove);
                        }
                    });
                });
            });
            if sel.entries.is_empty() {
                ui.label(strings::t().space_empty);
            } else {
                let order = sel.order();
                let lang = strings::lang();
                egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, ROW_HEIGHT, order.len(), |ui, range| {
                    for k in range {
                        let i = order[k];
                        let row = space_row_in(lang, &sel.entries[i], sel.measured[i]);
                        let (toggle, open) = space_row(ui, i, &row, &mut sel.checked[i], &mut action);
                        if toggle {
                            sel.toggle(i);
                        }
                        if open {
                            action = Some(Action::Open(i));
                        }
                    }
                });
            }
        }
    }
    space_browse::show(ui, app);
    if let Some(pending) = &app.space_pending_removal {
        let (items, _, bytes) = pending.summary();
        remove_confirm_window(ui.ctx(), items, bytes, &mut action);
    }
    match action {
        Some(Action::Back) => app.view = View::Programs,
        Some(Action::Refresh) => app.refresh_space(),
        Some(Action::ManualSearch) => app.search = Some(SearchDialog::default()),
        Some(Action::Open(i)) => {
            let entry = app.space.as_ref().and_then(|s| s.entries.get(i)).map(|e| (e.path.clone(), e.is_dir));
            match entry {
                Some((path, true)) => {
                    app.open_browse(path);
                    // Show the window now, not on the next input event.
                    ui.ctx().request_repaint();
                }
                Some((path, false)) => show_in_explorer(&path),
                None => {}
            }
        }
        Some(Action::WindowsCleanup) => open_storage_settings(),
        Some(Action::AskRemove) => app.ask_remove_space(),
        Some(Action::CancelRemove) => app.cancel_space_removal(),
        Some(Action::ConfirmRemove) => app.confirm_space_removal(),
        None => {}
    }
}

/// One row: check box (or room for one), name, location, explanation, kind and size; a Windows
/// space hog has "Öppna Windows rensning" instead of a check box. Returns whether a click on the
/// row (not on its box) toggles it, and whether it was double-clicked.
fn space_row(ui: &mut egui::Ui, index: usize, row: &SpaceRow, checked: &mut bool, action: &mut Option<Action>) -> (bool, bool) {
    let mut box_clicked = false;
    let clicks = compact_row(ui, index, |ui| {
        if row.checkbox {
            // The box flips `checked` itself; the row's own click goes through `toggle`.
            box_clicked = ui.add(egui::Checkbox::without_text(checked)).clicked();
        } else {
            ui.add_space(ui.spacing().icon_width);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_sized([80.0, ROW_HEIGHT], egui::Label::new(&row.size));
            ui.weak(row.kind);
            if !row.checkbox
                && ui.button(strings::t().open_windows_cleanup).on_hover_text(strings::t().open_windows_cleanup_tip).clicked()
            {
                *action = Some(Action::WindowsCleanup);
            }
            // Long texts are cut off instead of pushing the columns away; the explanation shows
            // in full on hover.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(egui::RichText::new(&row.name).strong()).truncate());
                ui.add(egui::Label::new(egui::RichText::new(&row.location).weak()).truncate());
                if let Some(text) = &row.explanation {
                    ui.add(egui::Label::new(egui::RichText::new(text).italics()).truncate()).on_hover_text(text);
                }
            });
        });
    });
    // Both clicks of a double-click toggle, which leaves the check box as it was.
    (row.checkbox && row_toggles(box_clicked, clicks.clicked), clicks.double_clicked)
}

/// "Ta bort markerade…": how much goes to the quarantine, then Avbryt / Ta bort.
fn remove_confirm_window(ctx: &egui::Context, items: usize, bytes: u64, action: &mut Option<Action>) {
    egui::Window::new(strings::t().remove_ghosts)
        .id(egui::Id::unique("remove_space_window"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_max_width(ctx.content_rect().width() * 0.6);
            ui.label(strings::space_remove_confirm(items, &format::human_size(Some(bytes))));
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(strings::t().cancel).clicked() {
                    *action = Some(Action::CancelRemove);
                }
                if ui.button(strings::t().remove).clicked() {
                    *action = Some(Action::ConfirmRemove);
                }
            });
        });
}

/// The Explorer of the pinned Windows folder (it is not in System32).
fn explorer() -> std::path::PathBuf {
    stale_core::paths::windows_dir().join("explorer.exe")
}

/// "Öppna Windows rensning": Windows' Storage settings (Storage Sense), fire and forget. Stale
/// never cleans these folders itself.
fn open_storage_settings() {
    if let Err(e) = std::process::Command::new(explorer()).arg("ms-settings:storagesense").spawn() {
        log::warn!("could not open the storage settings: {e}");
    }
}

/// Opens Explorer with `path` selected, fire and forget.
pub(super) fn show_in_explorer(path: &Path) {
    use std::os::windows::process::CommandExt;
    // Explorer parses its own command line: the path is quoted after the comma, as it expects.
    let arg = format!("/select,\"{}\"", path.display());
    if let Err(e) = std::process::Command::new(explorer()).raw_arg(arg).spawn() {
        log::warn!("could not open Explorer at {}: {e}", path.display());
    }
}
