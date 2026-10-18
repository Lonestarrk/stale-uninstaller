//! Action buttons, scan progress and the search box.

use eframe::egui;
use stale_core::uninstall::UninstallMode;

use crate::app::{SearchDialog, StaleApp};
use crate::strings;

pub fn show(ui: &mut egui::Ui, app: &mut StaleApp) {
    ui.horizontal(|ui| {
        // One queue at a time: the buttons wait for the dialog and any running queue.
        let can_queue = !app.checked.is_empty() && app.pending.is_none() && !app.queue_running();
        for (label, mode) in [(strings::UNINSTALL, UninstallMode::Normal), (strings::FORCE_REMOVE, UninstallMode::Forced)] {
            if ui.add_enabled(can_queue, egui::Button::new(label)).clicked() {
                app.open_queue_dialog(mode);
            }
        }
        let can_search = app.search.is_none() && !app.leftover_search_running() && !app.removal_running();
        if ui.add_enabled(can_search, egui::Button::new(strings::FIND_LEFTOVERS)).clicked() {
            app.search = Some(SearchDialog::default());
        }
        // One scan at a time: refreshing waits for the running scan to finish.
        if ui.add_enabled(!app.scanning, egui::Button::new(strings::REFRESH)).clicked() {
            app.refresh();
        }
        let mut show_hidden = app.settings.show_hidden;
        let mut toggled = false;
        ui.menu_button(strings::SETTINGS_MENU, |ui| {
            toggled = ui.checkbox(&mut show_hidden, strings::SHOW_HIDDEN).changed();
        });
        if toggled {
            app.toggle_show_hidden();
        }
        if app.scanning {
            ui.spinner();
            ui.label(strings::LOADING);
        }
        if app.leftover_search_running() {
            ui.spinner();
            ui.label(strings::SEARCHING);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut app.filters.search)
                    .hint_text(strings::SEARCH_HINT)
                    .desired_width(220.0),
            );
        });
    });
}
