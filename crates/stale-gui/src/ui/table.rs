//! Central program table with sortable headers.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use stale_core::ProgramId;

use crate::app::Row;
use crate::filter::{is_stale, Sort, SortKey};
use crate::{format, strings};

/// Last-used text colour of rows unused for more than six months.
const STALE_COLOR: egui::Color32 = egui::Color32::from_rgb(178, 58, 0);
const ROW_HEIGHT: f32 = 22.0;

/// The table's mutable view state.
pub struct TableState<'a> {
    pub sort: &'a mut Sort,
    pub selected: &'a mut Option<ProgramId>,
    pub checked: &'a mut HashSet<ProgramId>,
}

/// Draws `rows[i]` for each `i` in `visible`, in that order.
pub fn show(ui: &mut egui::Ui, rows: &[Row], visible: &[usize], state: TableState<'_>, now: DateTime<Utc>) {
    let TableState { sort, selected, checked } = state;
    TableBuilder::new(ui)
        .striped(true)
        .sense(egui::Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::auto())
        .column(Column::remainder().at_least(160.0).clip(true))
        .column(Column::initial(180.0).clip(true))
        .column(Column::auto())
        .column(Column::initial(80.0))
        .column(Column::initial(90.0))
        .column(Column::initial(120.0))
        .header(ROW_HEIGHT, |mut header| {
            header.col(|_| {});
            for (key, title) in [
                (SortKey::Name, strings::COL_NAME),
                (SortKey::Publisher, strings::COL_PUBLISHER),
                (SortKey::Drive, strings::COL_DRIVE),
                (SortKey::Size, strings::COL_SIZE),
                (SortKey::Installed, strings::COL_INSTALLED),
                (SortKey::LastUsed, strings::COL_LAST_USED),
            ] {
                header.col(|ui| {
                    let text = match (sort.key == key, sort.ascending) {
                        (true, true) => format!("{title} ▲"),
                        (true, false) => format!("{title} ▼"),
                        (false, _) => title.to_string(),
                    };
                    let label = egui::Label::new(egui::RichText::new(text).strong()).sense(egui::Sense::click());
                    if ui.add(label).clicked() {
                        *sort = sort.toggled(key);
                    }
                });
            }
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, visible.len(), |mut table_row| {
                let row = &rows[visible[table_row.index()]];
                let id = &row.program.id;
                table_row.set_selected(selected.as_ref() == Some(id));

                table_row.col(|ui| {
                    let mut on = checked.contains(id);
                    if ui.checkbox(&mut on, "").changed() {
                        if on {
                            checked.insert(id.clone());
                        } else {
                            checked.remove(id);
                        }
                    }
                });
                table_row.col(|ui| {
                    ui.label(&row.program.name);
                });
                table_row.col(|ui| {
                    ui.label(row.program.publisher.as_deref().unwrap_or(strings::UNKNOWN_VALUE));
                });
                table_row.col(|ui| drive_badge(ui, row.program.drive));
                table_row.col(|ui| {
                    ui.label(format::human_size(row.size_bytes()));
                });
                table_row.col(|ui| {
                    ui.label(format::ymd(row.program.install_date));
                });
                table_row.col(|ui| {
                    let last_used = row.last_used();
                    let mut text = egui::RichText::new(format::relative_date(last_used, now));
                    if is_stale(last_used, now) {
                        text = text.color(STALE_COLOR);
                    }
                    ui.label(text);
                });

                if table_row.response().clicked() {
                    *selected = Some(id.clone());
                }
            });
        });
}

/// The drive letter in a small rounded frame.
fn drive_badge(ui: &mut egui::Ui, drive: Option<char>) {
    let text = drive.map_or_else(|| strings::UNKNOWN_DRIVE.to_string(), |d| format!("{d}:"));
    egui::Frame::new()
        .fill(ui.visuals().widgets.inactive.weak_bg_fill)
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(5, 0))
        .show(ui, |ui| {
            ui.label(text);
        });
}
