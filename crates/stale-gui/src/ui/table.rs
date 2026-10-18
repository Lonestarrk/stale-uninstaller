//! Central program table with sortable headers.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use stale_core::ProgramId;

use crate::app::Row;
use crate::filter::{is_stale, Sort, SortKey};
use crate::{format, strings};

use super::stale_color;

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
                (SortKey::Name, strings::t().col_name),
                (SortKey::Publisher, strings::t().col_publisher),
                (SortKey::Drive, strings::t().col_drive),
                (SortKey::Size, strings::t().col_size),
                (SortKey::Installed, strings::t().col_installed),
                (SortKey::LastUsed, strings::t().col_last_used),
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

                let mut box_clicked = false;
                table_row.col(|ui| {
                    let mut on = checked.contains(id);
                    let response = ui.checkbox(&mut on, "");
                    box_clicked = response.clicked();
                    if response.changed() {
                        if on {
                            checked.insert(id.clone());
                        } else {
                            checked.remove(id);
                        }
                    }
                });
                table_row.col(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(&row.program.name);
                        if let Some(reason) = stale_core::dependencies::is_shared_dependency(&row.program) {
                            ui.small(egui::RichText::new(strings::t().dependency_badge).color(stale_color(ui.visuals())))
                                .on_hover_text(strings::dependency_reason_in(strings::lang(), reason));
                        }
                    });
                });
                table_row.col(|ui| {
                    ui.label(row.program.publisher.as_deref().unwrap_or(strings::t().unknown_value));
                });
                table_row.col(|ui| drive_badge(ui, row.program.drive));
                table_row.col(|ui| {
                    ui.label(format::human_size(row.size_bytes()));
                });
                table_row.col(|ui| {
                    ui.label(format::ymd(row.program.install_date));
                });
                table_row.col(|ui| {
                    if row.usage.is_none() {
                        ui.label(strings::t().pending);
                        return;
                    }
                    let last_used = row.last_used();
                    let mut text = egui::RichText::new(format::last_used_label(last_used, row.last_used_via(), row.last_used_autostart(), now));
                    if is_stale(last_used, now) {
                        text = text.color(stale_color(ui.visuals()));
                    }
                    ui.label(text);
                });

                if table_row.response().clicked() {
                    *selected = Some(id.clone());
                    // A click anywhere on the row ticks or unticks it; the box handles its own click.
                    if !box_clicked {
                        toggle_checked(checked, id);
                    }
                }
            });
        });
}

/// Ticks `id` when it is unticked and unticks it when it is ticked.
pub fn toggle_checked(checked: &mut HashSet<ProgramId>, id: &ProgramId) {
    if !checked.remove(id) {
        checked.insert(id.clone());
    }
}

/// The drive letter in a small rounded frame.
fn drive_badge(ui: &mut egui::Ui, drive: Option<char>) {
    let text = drive.map_or_else(|| strings::t().unknown_drive.to_string(), |d| format!("{d}:"));
    egui::Frame::new()
        .fill(ui.visuals().widgets.inactive.weak_bg_fill)
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(5, 0))
        .show(ui, |ui| {
            ui.label(text);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_click_ticks_then_unticks() {
        let mut checked = HashSet::new();
        let a = ProgramId("a".into());
        let b = ProgramId("b".into());
        toggle_checked(&mut checked, &a);
        assert!(checked.contains(&a));
        toggle_checked(&mut checked, &b);
        assert_eq!(checked.len(), 2);
        toggle_checked(&mut checked, &a);
        assert!(!checked.contains(&a) && checked.contains(&b));
    }
}
