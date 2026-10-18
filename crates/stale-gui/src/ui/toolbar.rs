//! Action buttons, the settings menu, scan progress and the search box.

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
        // Leaving the leftovers view waits for a running removal, like its own back button.
        let quarantine = strings::quarantine_button(app.quarantine.len());
        if ui.add_enabled(!app.removal_running(), egui::Button::new(quarantine)).clicked() {
            app.show_quarantine();
        }
        // One scan at a time: refreshing waits for the running scan to finish.
        if ui.add_enabled(!app.scanning, egui::Button::new(strings::REFRESH)).clicked() {
            app.refresh();
        }
        let mut show_hidden = app.settings.show_hidden;
        let mut toggled = false;
        let mut days = app.settings.quarantine_days;
        let mut save_days = false;
        ui.menu_button(strings::SETTINGS_MENU, |ui| {
            toggled = ui.checkbox(&mut show_hidden, strings::SHOW_HIDDEN).changed();
            ui.horizontal(|ui| {
                ui.label(strings::AUTO_PURGE_DAYS);
                let r = ui.add(egui::DragValue::new(&mut days).range(0..=365));
                // Saved once a drag ends, not for every value passed on the way.
                save_days = r.drag_stopped() || (r.changed() && !r.dragged());
                if days == 0 {
                    ui.label(strings::AUTO_PURGE_OFF);
                }
            });
        });
        if toggled {
            app.toggle_show_hidden();
        }
        if days != app.settings.quarantine_days {
            app.settings.quarantine_days = days;
        }
        if save_days {
            app.set_quarantine_days(days);
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
