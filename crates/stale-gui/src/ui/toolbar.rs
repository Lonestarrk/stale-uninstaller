//! Action buttons, scan progress and the search box.

use eframe::egui;

use crate::app::StaleApp;
use crate::strings;

pub fn show(ui: &mut egui::Ui, app: &mut StaleApp) {
    ui.horizontal(|ui| {
        for label in [strings::UNINSTALL, strings::FORCE_REMOVE, strings::FIND_LEFTOVERS] {
            ui.add_enabled(false, egui::Button::new(label)).on_disabled_hover_text(strings::NOT_YET);
        }
        // One scan at a time: refreshing waits for the running scan to finish.
        if ui.add_enabled(!app.scanning, egui::Button::new(strings::REFRESH)).clicked() {
            app.refresh();
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
