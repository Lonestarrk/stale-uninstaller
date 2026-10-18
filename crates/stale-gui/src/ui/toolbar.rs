//! Action buttons, scan progress and the search box.

use eframe::egui;
use stale_core::uninstall::UninstallMode;

use crate::app::StaleApp;
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
        ui.add_enabled(false, egui::Button::new(strings::FIND_LEFTOVERS)).on_disabled_hover_text(strings::NOT_YET);
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
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut app.filters.search)
                    .hint_text(strings::SEARCH_HINT)
                    .desired_width(220.0),
            );
        });
    });
}
