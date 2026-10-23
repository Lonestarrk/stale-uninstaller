//! Action buttons, the settings menu, scan progress and the search box.

use eframe::egui;
use stale_core::uninstall::UninstallMode;

use crate::app::StaleApp;
use crate::strings;

pub fn show(ui: &mut egui::Ui, app: &mut StaleApp) {
    ui.horizontal(|ui| {
        // One queue at a time: the buttons wait for the dialog and any running queue.
        let can_queue = !app.checked.is_empty() && app.pending.is_none() && !app.queue_running();
        for (label, mode) in [(strings::t().uninstall, UninstallMode::Normal), (strings::t().force_remove, UninstallMode::Forced)] {
            if ui.add_enabled(can_queue, egui::Button::new(label)).clicked() {
                app.open_queue_dialog(mode);
            }
        }
        // The space scan starts the first time the view opens; the search by name is a link there.
        if ui.add_enabled(!app.removal_running(), egui::Button::new(strings::t().find_leftovers)).clicked() {
            app.show_space();
        }
        // Leaving the leftovers view waits for a running removal, like its own back button.
        let quarantine = strings::quarantine_button(app.quarantine.len());
        if ui.add_enabled(!app.removal_running(), egui::Button::new(quarantine)).clicked() {
            app.show_quarantine();
        }
        // Like the quarantine button; the view itself waits for the program scan before scanning.
        if ui.add_enabled(!app.removal_running(), egui::Button::new(strings::t().ghosts)).clicked() {
            app.show_ghosts();
        }
        // One scan at a time: refreshing waits for the running scan to finish.
        if ui.add_enabled(!app.scanning, egui::Button::new(strings::t().refresh)).clicked() {
            app.refresh();
        }
        let mut show_hidden = app.settings.show_hidden;
        let mut toggled = false;
        let mut days = app.settings.quarantine_days;
        let mut save_days = false;
        let mut open_kept = false;
        let mut chosen_lang = None;
        let mut chosen_scale = None;
        ui.menu_button(strings::t().settings_menu, |ui| {
            toggled = ui.checkbox(&mut show_hidden, strings::t().show_hidden).changed();
            ui.horizontal(|ui| {
                ui.label(strings::t().auto_purge_days);
                let r = ui.add(egui::DragValue::new(&mut days).range(0..=365));
                // Saved once a drag ends, not for every value passed on the way.
                save_days = r.drag_stopped() || (r.changed() && !r.dragged());
                if days == 0 {
                    ui.label(strings::t().auto_purge_off);
                }
            });
            if ui.button(strings::t().kept_menu).clicked() {
                open_kept = true;
                ui.close();
            }
            ui.menu_button(strings::t().size_menu, |ui| {
                let current = app.ui_scale();
                for scale in crate::app::UI_SCALES {
                    let label = format!("{} %", (scale * 100.0).round() as u32);
                    if ui.radio((current - scale).abs() < 0.005, label).clicked() {
                        chosen_scale = Some(scale);
                        ui.close();
                    }
                }
            });
            ui.menu_button(strings::t().language_menu, |ui| {
                let current = strings::lang();
                for (lang, label) in [(strings::Lang::Sv, strings::t().lang_sv), (strings::Lang::En, strings::t().lang_en)] {
                    if ui.radio(current == lang, label).clicked() {
                        chosen_lang = Some(lang);
                        ui.close();
                    }
                }
            });
        });
        if let Some(scale) = chosen_scale {
            ui.ctx().set_zoom_factor(scale);
            app.set_ui_scale_at(scale, &stale_core::settings::Settings::path());
        }
        if let Some(lang) = chosen_lang {
            app.set_language(lang);
        }
        if toggled {
            app.toggle_show_hidden();
        }
        if open_kept {
            app.keep_window_open = true;
        }
        if days != app.settings.quarantine_days {
            app.settings.quarantine_days = days;
        }
        if save_days {
            app.set_quarantine_days(days);
        }
        if app.scanning {
            ui.spinner();
            ui.label(strings::t().loading);
        }
        if app.leftover_search_running() {
            ui.spinner();
            ui.label(strings::t().searching);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut app.filters.search)
                    .hint_text(strings::t().search_hint)
                    .desired_width(220.0),
            );
        });
    });
}
