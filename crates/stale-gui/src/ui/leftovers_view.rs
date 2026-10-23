//! The leftovers view (checkable leftovers per program, removal to quarantine, its summary) and
//! the "Sök rester" window.

use std::path::PathBuf;

use eframe::egui;
use stale_core::quarantine::{DestinationPlan, QuarantineManifest};
use stale_core::RegKeyPath;

use super::{leftover_popup, stale_color};
use crate::app::{StaleApp, View};
use crate::leftover_sel::LeftoverGroup;
use crate::{format, strings};

/// What a click in the view asks for, applied after drawing.
enum Action {
    Back,
    MarkSafe,
    Remove,
    ConfirmRemove,
    CancelRemove,
}

pub fn show(ui: &mut egui::Ui, app: &mut StaleApp) {
    let removing = app.removal_running();
    let mut action = None;
    let mut open_popup = None;
    let back = back_label(app.leftovers_origin);
    ui.horizontal(|ui| {
        ui.heading(strings::t().leftovers_title);
        if ui.add_enabled(!removing, egui::Button::new(back)).clicked() {
            action = Some(Action::Back);
        }
    });
    ui.separator();
    if let Some(plan) = &app.removal_plan {
        removal_confirm_window(ui, plan, &mut action);
    }
    if app.removal_done {
        removal_summary(ui, &app.last_manifests, back, &mut action);
    } else if let Some(sel) = app.leftovers.as_mut() {
        let (checked, total, bytes) = sel.summary();
        egui::Panel::bottom("leftovers_footer").show(ui, |ui| {
            ui.add_space(4.0);
            if let Some((done, total, what)) = &app.removal_progress {
                let fraction = if *total == 0 { 0.0 } else { *done as f32 / *total as f32 };
                ui.add(egui::ProgressBar::new(fraction).text(format!("{done}/{total}")));
                ui.label(what);
                return;
            }
            ui.horizontal(|ui| {
                ui.label(strings::checked_summary(checked, total, &format::human_size(Some(bytes))));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let asking = app.removal_plan.is_some();
                    if ui.add_enabled(checked > 0 && !removing && !asking, egui::Button::new(strings::t().remove_checked)).clicked() {
                        action = Some(Action::Remove);
                    }
                    if ui.add_enabled(!removing, egui::Button::new(strings::t().cancel)).clicked() {
                        action = Some(Action::Back);
                    }
                    if ui.add_enabled(!removing, egui::Button::new(strings::t().mark_safe)).clicked() {
                        action = Some(Action::MarkSafe);
                    }
                });
            });
        });
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if total == 0 {
                ui.label(strings::t().no_leftovers);
            }
            for (gi, group) in sel.groups.iter_mut().enumerate() {
                if group_row(ui, gi, group, !removing) {
                    open_popup = Some(gi);
                }
            }
        });
        if let Some(gi) = app.leftover_popup {
            let title = sel.groups[gi].program_name.clone();
            let id = ui.scope_id().with(("leftover_popup", gi));
            if !leftover_popup::show(ui.ctx(), id, &title, &[], &mut sel.groups[gi], !removing) {
                app.leftover_popup = None;
            }
        }
    }
    if open_popup.is_some() {
        app.leftover_popup = open_popup;
        // Show the popup now, not on the next input event.
        ui.ctx().request_repaint();
    }
    match action {
        Some(Action::Back) => app.leave_leftovers(),
        Some(Action::MarkSafe) => {
            if let Some(sel) = app.leftovers.as_mut() {
                sel.mark_safe();
            }
        }
        Some(Action::Remove) => {
            app.request_removal(&stale_core::quarantine::live_volume_ok, &stale_core::quarantine::live_free_bytes)
        }
        Some(Action::ConfirmRemove) => app.confirm_removal(),
        Some(Action::CancelRemove) => app.cancel_removal_plan(),
        None => {}
    }
}

const ROW_HEIGHT: f32 = 24.0;

/// What the user did to a compact row.
pub(super) struct RowClicks {
    pub clicked: bool,
    pub double_clicked: bool,
}

/// A compact, clickable row of fixed height with a hover highlight. `contents` draws it
/// (centered vertically). The row senses clicks under
/// the contents, so the check box keeps its own click.
pub(super) fn compact_row(ui: &mut egui::Ui, index: usize, contents: impl FnOnce(&mut egui::Ui)) -> RowClicks {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), egui::Sense::hover());
    let row = ui.interact(rect, ui.scope_id().with(("leftover_row", index)), egui::Sense::click());
    if row.hovered() {
        ui.painter().rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.horizontal_centered(contents);
    });
    RowClicks { clicked: row.clicked(), double_clicked: row.double_clicked() }
}

/// One program's row: checkbox, name, "N av M" and the size of what is checked. A click ticks or
/// unticks the program's items, a double-click asks for the popup (returns true).
fn group_row(ui: &mut egui::Ui, index: usize, group: &mut LeftoverGroup, editable: bool) -> bool {
    let mut box_clicked = false;
    let clicks = compact_row(ui, index, |ui| {
        let mut on = group.checked_count() > 0;
        box_clicked = ui.add_enabled(editable, egui::Checkbox::new(&mut on, "")).clicked();
        if box_clicked {
            group.toggle_all();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(format::human_size(Some(group.checked_bytes())));
            ui.label(strings::items_checked(group.checked_count(), group.items.len()));
            // A long name is cut off instead of pushing the numbers away.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(&group.program_name).truncate());
            });
        });
    });
    if !editable {
        // A removal is running: the choices are frozen, the popup still opens to look.
        return clicks.double_clicked;
    }
    row_click(group, box_clicked, clicks.clicked, clicks.double_clicked)
}

/// Whether a row's own click toggles its check mark: not when the click was on the check box.
pub(super) fn row_toggles(box_clicked: bool, clicked: bool) -> bool {
    clicked && !box_clicked
}

/// Applies a row's click to its group and says whether the popup is wanted. Both clicks of a
/// double-click report `clicked()` (egui adds `double_clicked()` to the second), so they toggle
/// twice and leave the check boxes as they were; the box's own click was already handled.
fn row_click(group: &mut LeftoverGroup, box_clicked: bool, clicked: bool, double_clicked: bool) -> bool {
    if row_toggles(box_clicked, clicked) {
        group.toggle_all();
    }
    double_clicked
}

/// Before a removal that copies files to C: or lacks space: one line per fallback drive and per
/// drive short of space; Fortsätt only when there is room.
fn removal_confirm_window(ui: &egui::Ui, plan: &DestinationPlan, action: &mut Option<Action>) {
    egui::Window::new(strings::t().remove_checked)
        .id(egui::Id::unique("remove_checked_window"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            for (from, bytes) in &plan.fallbacks {
                ui.label(strings::fallback_notice(*from, &format::human_size(Some(*bytes))));
            }
            for drive in &plan.insufficient {
                ui.colored_label(stale_color(ui.visuals()), strings::insufficient_space(*drive));
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(strings::t().cancel).clicked() {
                    *action = Some(Action::CancelRemove);
                }
                if ui.add_enabled(plan.insufficient.is_empty(), egui::Button::new(strings::t().continue_button)).clicked() {
                    *action = Some(Action::ConfirmRemove);
                }
            });
        });
}

/// A removal's failures, one line each, with the core's error codes worded.
fn failure_lines(failures: &[stale_core::quarantine::Failure]) -> Vec<String> {
    failures.iter().map(super::quarantine_view::failure_text).collect()
}

/// After the removal: per program what was moved and what failed, then the way back.
fn removal_summary(ui: &mut egui::Ui, manifests: &[QuarantineManifest], back: &str, action: &mut Option<Action>) {
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        for m in manifests {
            ui.strong(&m.program_name);
            let count = m.moved.len() + m.keys.len() + m.values.len() + m.services.len() + m.tasks.len();
            ui.label(strings::moved_to_quarantine(count));
            for line in failure_lines(&m.failures) {
                ui.colored_label(stale_color(ui.visuals()), line);
            }
            if !m.services.is_empty() {
                ui.label(strings::t().service_reboot);
            }
            ui.add_space(6.0);
        }
        ui.separator();
        if ui.button(back).clicked() {
            *action = Some(Action::Back);
        }
    });
}

/// The "Sök rester" window: program name, optional folder, Avbryt / Sök.
pub fn search_window(ui: &mut egui::Ui, app: &mut StaleApp) {
    let Some(search) = app.search.as_mut() else { return };
    let ctx = ui.ctx().clone();
    let mut close = false;
    let mut start = false;
    egui::Window::new(strings::t().search_title)
        .id(egui::Id::unique("search_window"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(&ctx, |ui| {
            egui::Grid::new("leftover_search").num_columns(2).show(ui, |ui| {
                ui.label(strings::t().search_name);
                ui.add(egui::TextEdit::singleline(&mut search.name).desired_width(260.0));
                ui.end_row();
                ui.label(strings::t().search_dir);
                ui.add(egui::TextEdit::singleline(&mut search.dir).desired_width(260.0));
                ui.end_row();
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(strings::t().cancel).clicked() {
                    close = true;
                }
                if ui.add_enabled(!search.name.trim().is_empty(), egui::Button::new(strings::t().search)).clicked() {
                    start = true;
                }
            });
        });
    if start {
        let name = search.name.trim().to_string();
        let dir = Some(search.dir.trim()).filter(|d| !d.is_empty()).map(PathBuf::from);
        app.search = None;
        app.search_leftovers_manual(name, dir);
    } else if close {
        app.search = None;
    }
}

/// The back button's text: "Till programlistan" to the program list, else "Tillbaka" (to the
/// ghosts or space view).
fn back_label(origin: View) -> &'static str {
    if origin == View::Programs { strings::t().back_to_programs } else { strings::t().back }
}

/// `HKLM\<path>`, `HKCU\<path>` or `HKU\<path>`.
pub(crate) fn key_text(key: &RegKeyPath) -> String {
    format!(r"{}\{}", key.hive.short_name(), key.path)
}

/// The user's text for a core note or ghost reason key; unknown keys are shown as they are.
pub(crate) fn note_text(note: &str) -> &str {
    use stale_core::ghosts::group as g;
    use stale_core::leftovers as l;
    match note {
        "user-folder" => strings::t().note_user_folder,
        "shared-publisher-key" => strings::t().note_shared_publisher,
        l::NOTE_OTHER_PROGRAM => strings::t().note_other_program,
        l::NOTE_CONTAINS_USER_DATA => strings::t().note_contains_user_data,
        l::NOTE_SUFFIX_MATCH => strings::t().note_suffix_match,
        l::NOTE_PROGID_OF_PROGRAM => strings::t().note_progid_of_program,
        g::REASON_APPDATA_ONLY => strings::t().note_appdata_only,
        g::REASON_DATA_ONLY => strings::t().note_data_only,
        g::REASON_OTHER_PROFILE => strings::t().note_other_profile,
        g::REASON_FIRST_WORD => strings::t().note_first_word,
        g::REASON_WORD_OF_NAME => strings::t().note_word_of_installed,
        g::REASON_SIBLING_OWNED => strings::t().note_sibling_owned,
        g::REASON_PUBLISHER_OF_INSTALLED => strings::t().note_publisher_of_installed,
        g::REASON_IN_USE => strings::t().note_in_use,
        g::REASON_LAUNCHER_LIBRARY => strings::t().note_launcher_library,
        g::NOTE_PARTLY_LIVE => strings::t().note_partly_live,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stale_core::leftovers::LeftoverKind;

    #[test]
    fn double_click_leaves_row_check_unchanged() {
        use stale_core::leftovers::{Confidence, Leftover};
        let item = |p: &str, confidence| Leftover {
            kind: LeftoverKind::Dir { path: PathBuf::from(p), size: Some(1) },
            confidence,
            note: None,
        };
        let report = stale_core::leftovers::LeftoverReport { items: vec![item(r"C:\A", Confidence::High), item(r"C:\B", Confidence::Medium)] };
        let mut group = crate::leftover_sel::LeftoverSelection::from_reports(&[("A".into(), report)]).groups.remove(0);
        let before = group.checked.clone();
        assert!(!row_click(&mut group, false, true, false), "first click: no popup");
        assert_eq!(group.checked, vec![false, false]);
        assert!(row_click(&mut group, false, true, true), "second click: popup");
        assert_eq!(group.checked, before);
        // A click on the box itself is not toggled a second time by the row.
        assert!(!row_click(&mut group, true, true, false));
        assert_eq!(group.checked, before);
    }

    #[test]
    fn notes_map_to_swedish_and_unknown_pass_through() {
        assert_eq!(note_text("user-folder"), strings::t().note_user_folder);
        assert_eq!(note_text("shared-publisher-key"), strings::t().note_shared_publisher);
        assert_eq!(note_text("used-by-other-program"), strings::t().note_other_program);
        for (key, text) in [
            ("contains-user-data", strings::t().note_contains_user_data),
            ("suffix-match", strings::t().note_suffix_match),
            ("progid-of-program", strings::t().note_progid_of_program),
            ("appdata-only", strings::t().note_appdata_only),
            ("data-only", strings::t().note_data_only),
            ("other-profile", strings::t().note_other_profile),
            ("first-word-match", strings::t().note_first_word),
            ("word-of-installed-name", strings::t().note_word_of_installed),
            ("sibling-owned", strings::t().note_sibling_owned),
            ("publisher-of-installed", strings::t().note_publisher_of_installed),
            ("in-use", strings::t().note_in_use),
            ("launcher-library", strings::t().note_launcher_library),
            ("partly-live", strings::t().note_partly_live),
        ] {
            assert_eq!(note_text(key), text, "{key}");
        }
        assert_eq!(note_text("something-new"), "something-new");
    }

    #[test]
    fn partly_live_note_is_worded_in_both_languages() {
        use crate::strings::{texts, Lang};
        assert_eq!(texts(Lang::Sv).note_partly_live, "kör också något som finns kvar");
        assert_eq!(texts(Lang::En).note_partly_live, "also runs something that is still there");
    }

    #[test]
    fn removal_failures_are_worded() {
        let failures = vec![
            stale_core::quarantine::Failure { what: r"C:\Apps\Foo".into(), error: r"locked: C:\Apps\Foo\a.dll".into() },
            stale_core::quarantine::Failure { what: r"C:\Apps\Bar".into(), error: "destination-exists".into() },
        ];
        assert_eq!(
            failure_lines(&failures),
            vec![
                r"C:\Apps\Foo: används av ett annat program: C:\Apps\Foo\a.dll".to_string(),
                r"C:\Apps\Bar: målet finns redan".to_string()
            ]
        );
    }

    #[test]
    fn back_button_names_its_destination() {
        assert_eq!(back_label(View::Ghosts), strings::t().back);
        assert_eq!(back_label(View::Space), strings::t().back);
        assert_eq!(back_label(View::Programs), strings::t().back_to_programs);
    }
}
