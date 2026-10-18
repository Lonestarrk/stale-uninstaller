//! egui drawing code for the main window.

use eframe::egui;

pub mod details;
pub mod filter_panel;
pub mod ghosts_view;
pub mod leftovers_view;
pub mod quarantine_view;
pub mod table;
pub mod toolbar;
pub mod uninstall_dialog;

/// Warning colour: last-used text of rows unused for more than six months, risky leftovers, failures.
pub const STALE_COLOR: egui::Color32 = egui::Color32::from_rgb(178, 58, 0);
