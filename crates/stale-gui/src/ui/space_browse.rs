//! The drill-down window of the "Sök rester" view: an opened entry's contents, largest first,
//! folder by folder. Children that may not be removed (Windows, installed programs, profile
//! folders, Stale's own) have no check box and say why.

use std::path::PathBuf;

use eframe::egui;
use stale_core::space::Child;

use super::leftovers_view::{compact_row, row_toggles};
use super::space_view::show_in_explorer;
use crate::app::StaleApp;
use crate::space_sel::{block_text_in, size_text_in, BrowseState};
use crate::strings::{self, Lang};
use crate::format;

const ROW_HEIGHT: f32 = 24.0;

/// What a click in the window asks for, applied after drawing.
#[derive(Default)]
struct Clicks {
    go_to: Option<PathBuf>,
    select: Option<PathBuf>,
    ask_remove: bool,
}

pub fn show(ui: &mut egui::Ui, app: &mut StaleApp) {
    let removing = app.removal_running();
    let loading = app.browse_running();
    // While the confirmation is open the window is frozen: no checking, no navigating.
    let asking = app.space_confirm.is_some();
    let Some(b) = app.space_browse.as_mut() else { return };
    let ctx = ui.ctx().clone();
    let screen = ctx.content_rect();
    let mut open = true;
    let mut clicks = Clicks::default();
    let title = b.root.file_name().map_or_else(|| b.root.display().to_string(), |n| n.to_string_lossy().into_owned());
    egui::Window::new(title)
        .id(egui::Id::unique("space_browse_window"))
        .open(&mut open)
        .collapsible(false)
        .default_size([screen.width() * 0.6, screen.height() * 0.6])
        .show(&ctx, |ui| {
            breadcrumb(ui, b, !asking, &mut clicks);
            ui.separator();
            match b.children.as_ref() {
                None => {
                    ui.horizontal(|ui| {
                        if loading {
                            ui.spinner();
                            ui.label(strings::t().browse_loading);
                        } else {
                            ui.label(strings::t().browse_aborted);
                        }
                    });
                }
                Some(children) if children.is_empty() => {
                    ui.label(strings::t().browse_empty);
                }
                Some(_) => listing(ui, b, screen.height() * 0.5, !asking, &mut clicks),
            }
            ui.separator();
            footer(ui, b, removing || asking, &mut clicks);
        });
    if !open {
        app.close_browse();
        return;
    }
    if let Some(path) = clicks.select {
        show_in_explorer(&path);
    }
    if let Some(dir) = clicks.go_to {
        app.browse_to(dir);
        ctx.request_repaint();
    } else if clicks.ask_remove {
        app.ask_remove_browse();
    }
}

/// From the opened entry down to the folder shown; a click on a step goes back up to it.
fn breadcrumb(ui: &mut egui::Ui, b: &BrowseState, editable: bool, clicks: &mut Clicks) {
    ui.horizontal_wrapped(|ui| {
        let crumbs = b.crumbs();
        let last = crumbs.len() - 1;
        for (k, (label, path)) in crumbs.into_iter().enumerate() {
            if k > 0 {
                ui.weak("›");
            }
            if k == last {
                ui.strong(label);
            } else if ui.add_enabled(editable, egui::Link::new(label)).clicked() {
                clicks.go_to = Some(path);
            }
        }
    });
}

/// The children, one compact row each, then "… N till". A click checks a row, a double-click
/// on a folder goes into it.
fn listing(ui: &mut egui::Ui, b: &mut BrowseState, max_height: f32, editable: bool, clicks: &mut Clicks) {
    let lang = strings::lang();
    let Some(children) = b.children.as_ref() else { return };
    let mut toggled = None;
    egui::ScrollArea::vertical().max_height(max_height).auto_shrink([false, true]).show_rows(
        ui,
        ROW_HEIGHT,
        children.len() + 1,
        |ui, range| {
            for i in range {
                let Some(child) = children.get(i) else {
                    if b.more > 0 {
                        ui.weak(strings::more_children_in(lang, b.more));
                    }
                    continue;
                };
                let (toggle, double) = child_row(ui, i, child, &mut b.checked[i], editable, lang);
                if toggle {
                    toggled = Some(i);
                }
                if editable && double && child.is_dir {
                    clicks.go_to = Some(child.path.clone());
                }
            }
        },
    );
    if let Some(i) = toggled {
        b.toggle(i);
    }
}

/// One child: check box (none when blocked), name, the reason it is blocked, kind and size.
/// Returns whether the row's own click toggles it, and whether it was double-clicked.
fn child_row(ui: &mut egui::Ui, index: usize, child: &Child, checked: &mut bool, editable: bool, lang: Lang) -> (bool, bool) {
    let mut box_clicked = false;
    let clicks = compact_row(ui, index, |ui| {
        if child.blocked.is_none() {
            box_clicked = ui.add_enabled(editable, egui::Checkbox::without_text(checked)).clicked();
        } else {
            ui.add_space(ui.spacing().icon_width);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_sized([80.0, ROW_HEIGHT], egui::Label::new(size_text_in(lang, child.size, child.partial)));
            ui.weak(if child.is_dir { strings::t().kind_dir } else { strings::t().kind_file });
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let name = child.path.file_name().map_or_else(|| child.path.display().to_string(), |n| n.to_string_lossy().into_owned());
                ui.add(egui::Label::new(name).truncate());
                if let Some(reason) = child.blocked {
                    let text = block_text_in(lang, reason);
                    ui.add(egui::Label::new(egui::RichText::new(text).weak().italics()).truncate()).on_hover_text(text);
                }
            });
        });
    });
    (editable && child.blocked.is_none() && row_toggles(box_clicked, clicks.clicked), clicks.double_clicked)
}

/// "N markerade · X", "Öppna i Utforskaren" and "Ta bort markerade…" (off while `busy`).
fn footer(ui: &mut egui::Ui, b: &BrowseState, busy: bool, clicks: &mut Clicks) {
    let (checked, bytes) = b.summary();
    ui.horizontal(|ui| {
        ui.label(strings::space_summary(checked, &format::human_size(Some(bytes))));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add_enabled(checked > 0 && !busy, egui::Button::new(strings::t().remove_ghosts)).clicked() {
                clicks.ask_remove = true;
            }
            if ui.button(strings::t().open_in_explorer).clicked() {
                // The one checked child, else the folder shown.
                let picked: Vec<&PathBuf> =
                    b.children.iter().flatten().zip(&b.checked).filter(|(_, on)| **on).map(|(c, _)| &c.path).collect();
                clicks.select = Some(match picked.as_slice() {
                    [one] => (*one).clone(),
                    _ => b.dir.clone(),
                });
            }
        });
    });
}
