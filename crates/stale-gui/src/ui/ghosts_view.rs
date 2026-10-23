//! The ghosts view ("Spökprogram"): programs gone from the uninstall list that left traces, one
//! compact row each with Behåll; a double-click shows the details and "Ta bort markerade…" moves
//! the checked ones' leftovers to the quarantine through the leftovers view.

use chrono::{DateTime, Utc};
use eframe::egui;
use stale_core::ghosts::{parse_location, Assessment, Evidence, Ghost, RefKind};

use super::leftovers_view::{compact_row, key_text, note_text, row_toggles};
use super::{leftover_popup, stale_color};
use crate::app::{KeepEntry, StaleApp, View};
use stale_core::keep::KeepList;
use crate::{format, strings};

/// What a click in the view asks for, applied after drawing.
enum Action {
    Back,
    Refresh,
    Keep(String),
    UndoKeep,
    AskRemove,
    CancelRemove,
    ConfirmRemove,
}

pub fn show(ui: &mut egui::Ui, app: &mut StaleApp, now: DateTime<Utc>) {
    let busy = app.ghost_scan_busy();
    let removing = app.removal_running();
    let mut action = None;
    let mut open_popup = None;
    ui.horizontal(|ui| {
        ui.heading(strings::t().ghosts_title);
        if ui.button(strings::t().back_to_programs).clicked() {
            action = Some(Action::Back);
        }
        if ui.add_enabled(!busy, egui::Button::new(strings::t().refresh)).clicked() {
            action = Some(Action::Refresh);
        }
    });
    ui.separator();
    if let Some(kept) = &app.last_kept {
        ui.horizontal(|ui| {
            ui.label(strings::kept_notice(&kept.ghost.name));
            if ui.button(strings::t().undo).clicked() {
                action = Some(Action::UndoKeep);
            }
        });
    }
    if busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(strings::t().ghosts_scanning);
        });
    } else if app.ghosts.is_none() {
        ui.label(strings::t().ghosts_not_scanned);
    } else if let Some(sel) = app.ghosts.as_mut() {
        if sel.ghosts.is_empty() {
            ui.label(strings::t().ghosts_empty);
        } else {
            let (checked, items, bytes) = sel.summary();
            let can_remove = items > 0 && !removing;
            if app.ghost_confirm {
                remove_confirm_window(ui.ctx(), items, checked, bytes, &mut action);
            }
            egui::Panel::bottom("ghosts_footer").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(strings::ghost_summary(checked, sel.ghosts.len(), &format::human_size(Some(bytes))));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_enabled(can_remove, egui::Button::new(strings::t().remove_ghosts)).clicked() {
                            action = Some(Action::AskRemove);
                        }
                    });
                });
            });
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                // Probably uninstalled and leftovers-only start open; uncertain starts collapsed.
                for (assessment, open) in
                    [(Assessment::ProbablyUninstalled, true), (Assessment::LeftoversOnly, true), (Assessment::Uncertain, false)]
                {
                    let rows: Vec<usize> = (0..sel.ghosts.len()).filter(|&i| sel.ghosts[i].assessment == assessment).collect();
                    if rows.is_empty() {
                        continue;
                    }
                    egui::CollapsingHeader::new(section_title(assessment, rows.len()))
                        .id_salt(("ghost_section", format!("{assessment:?}")))
                        .default_open(open)
                        .show(ui, |ui| {
                            for i in rows {
                                if ghost_row(ui, i, &sel.ghosts[i], &mut sel.checked[i], now, &mut action) {
                                    open_popup = Some(i);
                                }
                            }
                        });
                }
            });
            if let Some(gi) = app.ghost_popup {
                match (sel.ghosts.get(gi), sel.leftovers.get_mut(gi)) {
                    (Some(ghost), Some(group)) => {
                        let id = ui.scope_id().with(("ghost_popup", gi));
                        if !leftover_popup::show(ui.ctx(), id, &ghost.name, &popup_info(ghost), group, !removing) {
                            app.ghost_popup = None;
                        }
                    }
                    _ => app.ghost_popup = None,
                }
            }
        }
    }
    if open_popup.is_some() {
        app.ghost_popup = open_popup;
        // Show the popup now, not on the next input event.
        ui.ctx().request_repaint();
    }
    match action {
        Some(Action::Back) => app.view = View::Programs,
        Some(Action::Refresh) => app.refresh_ghosts(),
        Some(Action::Keep(key)) => app.keep_ghost(&key),
        Some(Action::UndoKeep) => app.undo_keep(),
        Some(Action::AskRemove) => app.ghost_confirm = true,
        Some(Action::CancelRemove) => app.ghost_confirm = false,
        Some(Action::ConfirmRemove) => app.remove_checked_ghosts(),
        None => {}
    }
}

/// The "Behållna" window: every kept name, folder and registry key, each with "Ta bort".
pub fn keep_window(ui: &mut egui::Ui, app: &mut StaleApp) {
    if !app.keep_window_open {
        return;
    }
    let ctx = ui.ctx().clone();
    let mut open = true;
    let mut remove = None;
    egui::Window::new(strings::t().kept_title).id(egui::Id::unique("keep_list_window")).open(&mut open).collapsible(false).show(&ctx, |ui| {
        if keep_entries(&app.keep).is_empty() {
            ui.label(strings::t().kept_empty);
            return;
        }
        egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
            for (entry, text) in keep_entries(&app.keep) {
                ui.horizontal(|ui| {
                    if ui.button(strings::t().kept_remove).clicked() {
                        remove = Some(entry.clone());
                    }
                    ui.label(text);
                });
            }
        });
    });
    app.keep_window_open = open;
    if let Some(entry) = remove {
        app.remove_keep_entry(&entry);
    }
}

/// Each keep entry with its line: names, then folders, then registry keys.
fn keep_entries(keep: &KeepList) -> Vec<(KeepEntry, String)> {
    let names = keep.names.iter().map(|n| (KeepEntry::Name(n.clone()), strings::kept_name(n)));
    let paths = keep.paths.iter().map(|p| (KeepEntry::Path(p.clone()), strings::kept_path(&p.display().to_string())));
    let keys = keep.keys.iter().map(|k| (KeepEntry::Key(k.clone()), strings::kept_key(&key_text(k))));
    names.chain(paths).chain(keys).collect()
}

/// "Ta bort markerade…": what goes to the quarantine, then Avbryt / Ta bort.
fn remove_confirm_window(ctx: &egui::Context, items: usize, programs: usize, bytes: u64, action: &mut Option<Action>) {
    egui::Window::new(strings::t().remove_ghosts)
        .id(egui::Id::unique("remove_ghosts_window"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_max_width(ctx.content_rect().width() * 0.6);
            ui.label(strings::ghost_remove_confirm(items, programs, &format::human_size(Some(bytes))));
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(strings::t().cancel).clicked() {
                    *action = Some(Action::CancelRemove);
                }
                if ui.button(strings::t().remove).clicked() {
                    *action = Some(Action::ConfirmRemove);
                }
            });
        });
}

/// One compact row: check box, name, publisher, badge (with a warning for an uncertain one), size,
/// last use and Behåll. A click toggles the check box, a double-click asks for the popup (true).
fn ghost_row(ui: &mut egui::Ui, index: usize, ghost: &Ghost, checked: &mut bool, now: DateTime<Utc>, action: &mut Option<Action>) -> bool {
    let mut box_clicked = false;
    let clicks = compact_row(ui, index, |ui| {
        box_clicked = ui.add(egui::Checkbox::without_text(checked)).clicked();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(strings::t().keep).on_hover_text(strings::t().keep_tip).clicked() {
                *action = Some(Action::Keep(ghost.key.clone()));
            }
            ui.label(format::relative_date(ghost.last_used, now));
            ui.label(format::human_size(Some(ghost.total_size)));
            let badge = assessment_label(ghost.assessment);
            if ghost.assessment == Assessment::Uncertain {
                ui.colored_label(stale_color(ui.visuals()), badge);
            } else {
                ui.label(badge);
            }
            if let Some(why) = uncertainty_text(ghost) {
                ui.colored_label(stale_color(ui.visuals()), strings::t().warning_icon).on_hover_text(why);
            }
            // Long names are cut off instead of pushing the columns away.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(egui::RichText::new(&ghost.name).strong()).truncate());
                if let Some(publisher) = &ghost.publisher {
                    ui.add(egui::Label::new(egui::RichText::new(publisher).weak()).truncate());
                }
            });
        });
    });
    // Both clicks of a double-click toggle, which leaves the check box as it was.
    if row_toggles(box_clicked, clicks.clicked) {
        *checked = !*checked;
    }
    clicks.double_clicked
}

/// The popup's lines above the items: why the ghost is uncertain, then its usage traces.
fn popup_info(ghost: &Ghost) -> Vec<String> {
    let traces = ghost.evidence.iter().filter(|e| matches!(e, Evidence::UsageTrace { .. })).map(evidence_line);
    uncertainty_text(ghost).into_iter().chain(traces).collect()
}

fn section_title(a: Assessment, count: usize) -> String {
    match a {
        Assessment::ProbablyUninstalled => strings::probably_section(count),
        Assessment::LeftoversOnly => strings::leftovers_section(count),
        Assessment::Uncertain => strings::uncertain_section(count),
    }
}

fn assessment_label(a: Assessment) -> &'static str {
    match a {
        Assessment::ProbablyUninstalled => strings::t().assess_probably,
        Assessment::LeftoversOnly => strings::t().assess_leftovers,
        Assessment::Uncertain => strings::t().assess_uncertain,
    }
}

/// The reasons and the note in the user's language, joined; `None` when there are none.
fn uncertainty_text(ghost: &Ghost) -> Option<String> {
    let mut parts: Vec<&str> = ghost.reasons.iter().map(|r| note_text(r)).collect();
    if let Some(note) = ghost.note.as_deref().filter(|n| !ghost.reasons.iter().any(|r| r == n)) {
        parts.push(note_text(note));
    }
    (!parts.is_empty()).then(|| parts.join(strings::t().list_separator))
}

fn ref_label(kind: RefKind) -> &'static str {
    match kind {
        RefKind::Run => strings::t().ref_run,
        RefKind::AppPaths => strings::t().ref_app_paths,
        RefKind::ClassesServer => strings::t().ref_classes_server,
        RefKind::ShellCommand => strings::t().ref_shell_command,
        RefKind::Service => strings::t().ref_service,
        RefKind::ScheduledTask => strings::t().ref_task,
        RefKind::Shortcut => strings::t().ref_shortcut,
    }
}

/// One evidence line; registry locations use the `HKLM\…` form.
fn evidence_line(e: &Evidence) -> String {
    match e {
        Evidence::Folder { path, size, .. } => {
            strings::evidence_folder(&path.display().to_string(), &format::human_size(*size))
        }
        Evidence::RegKeyMissingPath { key, path, .. } => {
            strings::evidence_key_missing_path(&key_text(key), &path.display().to_string())
        }
        Evidence::RegKeyOrphan { key, .. } => strings::evidence_key(&key_text(key)),
        Evidence::DanglingRef { kind, location, target, .. } => {
            let location = parse_location(location).map_or_else(|| location.clone(), |k| key_text(&k));
            strings::evidence_ref(ref_label(*kind), &location, &target.display().to_string())
        }
        Evidence::UsageTrace { exe, last_run, source } => strings::evidence_usage(
            &format::ymd(Some(format::local_date(*last_run))),
            format::source_label(*source),
            &exe.display().to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use stale_core::ghosts::group::{REASON_OTHER_PROFILE, REASON_SIBLING_OWNED};
    use stale_core::ghosts::RootKind;
    use stale_core::usage::UsageSource;
    use stale_core::{Hive, RegKeyPath};

    use super::*;
    use crate::ghost_sel::tests::ghost;

    #[test]
    fn evidence_lines_name_kind_and_target() {
        let key = RegKeyPath { hive: Hive::CurrentUser, path: r"Software\Foo".into() };
        let folder = Evidence::Folder {
            path: PathBuf::from(r"C:\Program Files\Foo"),
            size: Some(3 * 1024 * 1024),
            modified: None,
            root_kind: RootKind::ProgramFiles,
            holds_user_data: false,
        };
        assert_eq!(evidence_line(&folder), strings::evidence_folder(r"C:\Program Files\Foo", "3 MB"));
        let missing = Evidence::RegKeyMissingPath { key: key.clone(), value_name: "Path".into(), path: r"D:\Foo".into(), shared: false };
        assert_eq!(evidence_line(&missing), strings::evidence_key_missing_path(r"HKCU\Software\Foo", r"D:\Foo"));
        assert_eq!(evidence_line(&Evidence::RegKeyOrphan { key, shared: false }), strings::evidence_key(r"HKCU\Software\Foo"));
        let run = Evidence::DanglingRef {
            kind: RefKind::Run,
            location: r"LocalMachine\SOFTWARE\Microsoft\Windows\CurrentVersion\Run".into(),
            target: r"C:\Foo\foo.exe".into(),
            value_name: Some("Foo".into()),
            partly_live: false,
        };
        assert_eq!(
            evidence_line(&run),
            strings::evidence_ref(strings::t().ref_run, r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Run", r"C:\Foo\foo.exe")
        );
        let task = Evidence::DanglingRef {
            kind: RefKind::ScheduledTask,
            location: r"Vendor\Updater".into(),
            target: r"C:\Foo\up.exe".into(),
            value_name: None,
            partly_live: false,
        };
        assert_eq!(evidence_line(&task), strings::evidence_ref(strings::t().ref_task, r"Vendor\Updater", r"C:\Foo\up.exe"));
        let when = DateTime::from_timestamp(1_792_000_000, 0).unwrap();
        let usage = Evidence::UsageTrace { exe: r"C:\Foo\foo.exe".into(), last_run: when, source: UsageSource::Prefetch };
        assert_eq!(
            evidence_line(&usage),
            strings::evidence_usage(&format::ymd(Some(format::local_date(when))), strings::t().source_prefetch, r"C:\Foo\foo.exe")
        );
    }

    #[test]
    fn uncertainty_lists_reasons_and_a_separate_note_once() {
        let mut g = ghost("Foo", Assessment::Uncertain, 1);
        assert_eq!(uncertainty_text(&g), None);
        g.reasons = vec![REASON_SIBLING_OWNED.into(), REASON_OTHER_PROFILE.into()];
        g.note = Some(REASON_OTHER_PROFILE.into());
        let both = format!("{}{}{}", strings::t().note_sibling_owned, strings::t().list_separator, strings::t().note_other_profile);
        assert_eq!(uncertainty_text(&g), Some(both));
        g.reasons.clear();
        assert_eq!(uncertainty_text(&g).as_deref(), Some(strings::t().note_other_profile));
    }

    #[test]
    fn popup_info_is_the_uncertainty_then_usage_traces_only() {
        let mut g = ghost("Foo", Assessment::Uncertain, 1);
        assert!(popup_info(&g).is_empty(), "folders and keys are the popup's items, not info");
        g.reasons = vec![REASON_SIBLING_OWNED.into()];
        let when = DateTime::from_timestamp(1_792_000_000, 0).unwrap();
        let trace = Evidence::UsageTrace { exe: r"C:\Foo\foo.exe".into(), last_run: when, source: UsageSource::Bam };
        g.evidence.push(trace.clone());
        assert_eq!(popup_info(&g), vec![strings::t().note_sibling_owned.to_string(), evidence_line(&trace)]);
    }

    #[test]
    fn keep_entries_list_names_paths_and_keys() {
        let keep = KeepList {
            names: vec!["Alpha".into()],
            paths: vec![PathBuf::from(r"C:\Program Files\Alpha")],
            keys: vec![RegKeyPath { hive: Hive::CurrentUser, path: r"Software\Alpha".into() }],
        };
        let entries = keep_entries(&keep);
        assert_eq!(
            entries,
            vec![
                (KeepEntry::Name("Alpha".into()), strings::kept_name("Alpha")),
                (KeepEntry::Path(PathBuf::from(r"C:\Program Files\Alpha")), strings::kept_path(r"C:\Program Files\Alpha")),
                (
                    KeepEntry::Key(RegKeyPath { hive: Hive::CurrentUser, path: r"Software\Alpha".into() }),
                    strings::kept_key(r"HKCU\Software\Alpha")
                ),
            ]
        );
        assert!(keep_entries(&KeepList::default()).is_empty());
    }

    #[test]
    fn badges_and_ref_labels_are_swedish() {
        assert_eq!(assessment_label(Assessment::ProbablyUninstalled), strings::t().assess_probably);
        assert_eq!(assessment_label(Assessment::LeftoversOnly), strings::t().assess_leftovers);
        assert_eq!(assessment_label(Assessment::Uncertain), strings::t().assess_uncertain);
        assert_eq!(section_title(Assessment::ProbablyUninstalled, 2), "Troligen avinstallerade (2)");
        assert_eq!(section_title(Assessment::LeftoversOnly, 1), "Rester utan installation (bocka i själv) (1)");
        assert_eq!(section_title(Assessment::Uncertain, 3), "Osäkra (3)");
        assert_eq!(ref_label(RefKind::ClassesServer), strings::t().ref_classes_server);
        assert_eq!(ref_label(RefKind::Shortcut), strings::t().ref_shortcut);
    }
}
