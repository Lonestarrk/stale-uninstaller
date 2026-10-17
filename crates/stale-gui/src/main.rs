// Keep a console in debug builds so panics are visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use stale_gui::app::StaleApp;
use stale_gui::logging;
use stale_gui::strings::APP_TITLE;

fn main() -> eframe::Result {
    logging::init();
    log::info!("stale {} starting", stale_core::version());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 700.0]).with_title(APP_TITLE),
        ..Default::default()
    };
    eframe::run_native(APP_TITLE, options, Box::new(|cc| Ok(Box::new(StaleApp::new(cc)))))
}
