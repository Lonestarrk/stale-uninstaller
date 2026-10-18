//! The leftovers view (checkable leftovers per program, removal to quarantine, its summary) and
//! the "Sök rester" window.

use std::path::PathBuf;

use eframe::egui;
use stale_core::leftovers::{Confidence, Leftover, LeftoverKind};
use stale_core::quarantine::{DestinationPlan, QuarantineManifest};
use stale_core::{Hive, RegKeyPath};

use super::STALE_COLOR;
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
                egui::CollapsingHeader::new(&group.program_name)
                    .id_salt(("leftover_group", gi))
                    .default_open(true)
                    .show(ui, |ui| group_body(ui, group, !removing));
            }
        });
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
        Some(Action::CancelRemove) => app.removal_plan = None,
        None => {}
    }
}

/// Before a removal that copies files to C: or lacks space: one line per fallback drive and per
/// drive short of space; Fortsätt only when there is room.
fn removal_confirm_window(ui: &egui::Ui, plan: &DestinationPlan, action: &mut Option<Action>) {
    egui::Window::new(strings::t().remove_checked)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            for (from, bytes) in &plan.fallbacks {
                ui.label(strings::fallback_notice(*from, &format::human_size(Some(*bytes))));
            }
            for drive in &plan.insufficient {
                ui.colored_label(STALE_COLOR, strings::insufficient_space(*drive));
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

/// One program's leftovers in three sections; check boxes are disabled while a removal runs.
fn group_body(ui: &mut egui::Ui, group: &mut LeftoverGroup, editable: bool) {
    for (section, title) in
        [strings::t().section_files, strings::t().section_registry, strings::t().section_other].into_iter().enumerate()
    {
        let indices: Vec<usize> = (0..group.items.len()).filter(|&i| section_of(&group.items[i].kind) == section).collect();
        if indices.is_empty() {
            continue;
        }
        let bytes = indices.iter().filter_map(|&i| group.items[i].size()).sum();
        ui.strong(format!("{title} {}", strings::section_counts(indices.len(), &format::human_size(Some(bytes)))));
        for i in indices {
            item_row(ui, &group.items[i], &mut group.checked[i], editable);
        }
    }
}

fn item_row(ui: &mut egui::Ui, item: &Leftover, checked: &mut bool, editable: bool) {
    ui.horizontal(|ui| {
        ui.add_enabled(editable, egui::Checkbox::without_text(checked));
        ui.weak(kind_label(&item.kind));
        ui.label(item_text(&item.kind));
        if item.confidence == Confidence::Low {
            let warning = match &item.note {
                Some(note) => format!("{} {}", strings::t().warning_icon, note_text(note)),
                None => strings::t().warning_icon.to_string(),
            };
            ui.colored_label(STALE_COLOR, warning);
        }
        if let LeftoverKind::Dir { size, .. } | LeftoverKind::File { size, .. } = &item.kind {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format::human_size(*size));
            });
        }
    });
}

/// After the removal: per program what was moved and what failed, then the way back.
fn removal_summary(ui: &mut egui::Ui, manifests: &[QuarantineManifest], back: &str, action: &mut Option<Action>) {
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        for m in manifests {
            ui.strong(&m.program_name);
            let count = m.moved.len() + m.keys.len() + m.values.len() + m.services.len() + m.tasks.len();
            ui.label(strings::moved_to_quarantine(count));
            for f in &m.failures {
                ui.colored_label(STALE_COLOR, format!("{}: {}", f.what, f.error));
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

/// The back button's text: "Tillbaka" to the ghosts view, else "Till programlistan".
fn back_label(origin: View) -> &'static str {
    if origin == View::Ghosts { strings::t().back } else { strings::t().back_to_programs }
}

/// 0: folders and files, 1: registry, 2: everything else.
fn section_of(kind: &LeftoverKind) -> usize {
    match kind {
        LeftoverKind::Dir { .. } | LeftoverKind::File { .. } => 0,
        LeftoverKind::RegKey { .. } | LeftoverKind::RegValue { .. } => 1,
        LeftoverKind::Shortcut { .. } | LeftoverKind::Service { .. } | LeftoverKind::ScheduledTask { .. } => 2,
    }
}

fn kind_label(kind: &LeftoverKind) -> &'static str {
    match kind {
        LeftoverKind::Dir { .. } => strings::t().kind_dir,
        LeftoverKind::File { .. } => strings::t().kind_file,
        LeftoverKind::RegKey { .. } => strings::t().kind_regkey,
        LeftoverKind::RegValue { .. } => strings::t().kind_regvalue,
        LeftoverKind::Shortcut { .. } => strings::t().kind_shortcut,
        LeftoverKind::Service { .. } => strings::t().kind_service,
        LeftoverKind::ScheduledTask { .. } => strings::t().kind_task,
    }
}

/// `HKLM\<path>`, `HKCU\<path>` or `HKU\<path>`.
pub(crate) fn key_text(key: &RegKeyPath) -> String {
    let hive = match key.hive {
        Hive::LocalMachine => "HKLM",
        Hive::CurrentUser => "HKCU",
        Hive::Users => "HKU",
    };
    format!(r"{hive}\{}", key.path)
}

/// The path, key or name that identifies a leftover.
fn item_text(kind: &LeftoverKind) -> String {
    match kind {
        LeftoverKind::Dir { path, .. } | LeftoverKind::File { path, .. } => path.display().to_string(),
        LeftoverKind::RegKey { key } => key_text(key),
        LeftoverKind::RegValue { key, name } => format!("{} → {name}", key_text(key)),
        LeftoverKind::Shortcut { lnk, .. } => lnk.display().to_string(),
        LeftoverKind::Service { name, .. } | LeftoverKind::ScheduledTask { name, .. } => name.clone(),
    }
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
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        ] {
            assert_eq!(note_text(key), text, "{key}");
        }
        assert_eq!(note_text("something-new"), "something-new");
    }

    #[test]
    fn back_button_names_its_destination() {
        assert_eq!(back_label(View::Ghosts), strings::t().back);
        assert_eq!(back_label(View::Programs), strings::t().back_to_programs);
    }

    #[test]
    fn registry_items_show_hive_abbreviation() {
        let key = |hive| RegKeyPath { hive, path: r"Software\Foo".into() };
        assert_eq!(item_text(&LeftoverKind::RegKey { key: key(Hive::LocalMachine) }), r"HKLM\Software\Foo");
        assert_eq!(item_text(&LeftoverKind::RegKey { key: key(Hive::CurrentUser) }), r"HKCU\Software\Foo");
        assert_eq!(item_text(&LeftoverKind::RegKey { key: key(Hive::Users) }), r"HKU\Software\Foo");
        assert_eq!(
            item_text(&LeftoverKind::RegValue { key: key(Hive::CurrentUser), name: "Foo".into() }),
            r"HKCU\Software\Foo → Foo"
        );
        assert_eq!(kind_label(&LeftoverKind::RegValue { key: key(Hive::Users), name: String::new() }), strings::t().kind_regvalue);
    }

    #[test]
    fn sections_split_files_registry_other() {
        let path = PathBuf::from(r"C:\x");
        assert_eq!(section_of(&LeftoverKind::File { path: path.clone(), size: None }), 0);
        assert_eq!(section_of(&LeftoverKind::RegKey { key: RegKeyPath { hive: Hive::LocalMachine, path: "x".into() } }), 1);
        assert_eq!(section_of(&LeftoverKind::Service { name: "s".into(), image: path }), 2);
    }
}
