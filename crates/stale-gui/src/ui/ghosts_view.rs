//! The ghosts view ("Spökprogram"): programs gone from the uninstall list that left traces, each
//! with its evidence and Behåll; the checked ones go on to the leftovers view.

use chrono::{DateTime, Utc};
use eframe::egui;
use stale_core::ghosts::{parse_location, Assessment, Evidence, Ghost, RefKind};

use super::leftovers_view::{key_text, note_text};
use super::STALE_COLOR;
use crate::app::{KeepEntry, StaleApp, View};
use stale_core::keep::KeepList;
use crate::{format, strings};

/// Evidence lists longer than this start collapsed.
const OPEN_EVIDENCE_MAX: usize = 5;

/// What a click in the view asks for, applied after drawing.
enum Action {
    Back,
    Refresh,
    Keep(String),
    UndoKeep,
    ShowLeftovers,
}

pub fn show(ui: &mut egui::Ui, app: &mut StaleApp, now: DateTime<Utc>) {
    let busy = app.ghost_scan_busy();
    let removing = app.removal_running();
    let mut action = None;
    ui.horizontal(|ui| {
        ui.heading(strings::GHOSTS_TITLE);
        if ui.button(strings::BACK_TO_PROGRAMS).clicked() {
            action = Some(Action::Back);
        }
        if ui.add_enabled(!busy, egui::Button::new(strings::REFRESH)).clicked() {
            action = Some(Action::Refresh);
        }
    });
    ui.separator();
    if let Some(kept) = &app.last_kept {
        ui.horizontal(|ui| {
            ui.label(strings::kept_notice(&kept.ghost.name));
            if ui.button(strings::UNDO).clicked() {
                action = Some(Action::UndoKeep);
            }
        });
    }
    if busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(strings::GHOSTS_SCANNING);
        });
    } else if app.ghosts.is_none() {
        ui.label(strings::GHOSTS_NOT_SCANNED);
    } else if let Some(sel) = app.ghosts.as_mut() {
        if sel.ghosts.is_empty() {
            ui.label(strings::GHOSTS_EMPTY);
        } else {
            let (checked, total, bytes) = sel.summary();
            let can_show = checked > 0 && !removing;
            egui::Panel::bottom("ghosts_footer").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(strings::ghost_summary(checked, total, &format::human_size(Some(bytes))));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_enabled(can_show, egui::Button::new(strings::SHOW_GHOST_LEFTOVERS)).clicked() {
                            action = Some(Action::ShowLeftovers);
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
                        .id_salt(("ghost_section", assessment_label(assessment)))
                        .default_open(open)
                        .show(ui, |ui| {
                            for i in rows {
                                ghost_row(ui, &sel.ghosts[i], &mut sel.checked[i], now, &mut action);
                            }
                        });
                }
            });
        }
    }
    match action {
        Some(Action::Back) => app.view = View::Programs,
        Some(Action::Refresh) => app.refresh_ghosts(),
        Some(Action::Keep(key)) => app.keep_ghost(&key),
        Some(Action::UndoKeep) => app.undo_keep(),
        Some(Action::ShowLeftovers) => app.open_leftovers_from_ghosts(),
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
    egui::Window::new(strings::KEPT_TITLE).open(&mut open).collapsible(false).show(&ctx, |ui| {
        if keep_entries(&app.keep).is_empty() {
            ui.label(strings::KEPT_EMPTY);
            return;
        }
        egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
            for (entry, text) in keep_entries(&app.keep) {
                ui.horizontal(|ui| {
                    if ui.button(strings::KEPT_REMOVE).clicked() {
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

/// Check box, name, assessment, size, last use and Behåll; then why it is uncertain and its evidence.
fn ghost_row(ui: &mut egui::Ui, ghost: &Ghost, checked: &mut bool, now: DateTime<Utc>, action: &mut Option<Action>) {
    ui.horizontal(|ui| {
        ui.add(egui::Checkbox::without_text(checked));
        ui.strong(&ghost.name);
        if let Some(publisher) = &ghost.publisher {
            ui.weak(publisher);
        }
        let badge = assessment_label(ghost.assessment);
        if ghost.assessment == Assessment::Uncertain {
            ui.colored_label(STALE_COLOR, badge);
        } else {
            ui.label(badge);
        }
        ui.label(format::human_size(Some(ghost.total_size)));
        ui.label(format::relative_date(ghost.last_used, now));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(strings::KEEP).on_hover_text(strings::KEEP_TIP).clicked() {
                *action = Some(Action::Keep(ghost.key.clone()));
            }
        });
    });
    if let Some(why) = uncertainty_text(ghost) {
        ui.colored_label(STALE_COLOR, format!("{} {why}", strings::WARNING_ICON));
    }
    let lines: Vec<String> = ghost.evidence.iter().map(evidence_line).collect();
    egui::CollapsingHeader::new(strings::evidence_header(lines.len()))
        .id_salt(("ghost_evidence", &ghost.key))
        .default_open(lines.len() <= OPEN_EVIDENCE_MAX)
        .show(ui, |ui| {
            for line in &lines {
                ui.label(line);
            }
        });
    ui.add_space(4.0);
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
        Assessment::ProbablyUninstalled => strings::ASSESS_PROBABLY,
        Assessment::LeftoversOnly => strings::ASSESS_LEFTOVERS,
        Assessment::Uncertain => strings::ASSESS_UNCERTAIN,
    }
}

/// The reasons and the note in Swedish, joined; `None` when there are none.
fn uncertainty_text(ghost: &Ghost) -> Option<String> {
    let mut parts: Vec<&str> = ghost.reasons.iter().map(|r| note_text(r)).collect();
    if let Some(note) = ghost.note.as_deref().filter(|n| !ghost.reasons.iter().any(|r| r == n)) {
        parts.push(note_text(note));
    }
    (!parts.is_empty()).then(|| parts.join(strings::LIST_SEPARATOR))
}

fn ref_label(kind: RefKind) -> &'static str {
    match kind {
        RefKind::Run => strings::REF_RUN,
        RefKind::AppPaths => strings::REF_APP_PATHS,
        RefKind::ClassesServer => strings::REF_CLASSES_SERVER,
        RefKind::ShellCommand => strings::REF_SHELL_COMMAND,
        RefKind::Service => strings::REF_SERVICE,
        RefKind::ScheduledTask => strings::REF_TASK,
        RefKind::Shortcut => strings::REF_SHORTCUT,
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
        Evidence::RegKeyOrphan { key } => strings::evidence_key(&key_text(key)),
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
        let missing = Evidence::RegKeyMissingPath { key: key.clone(), value_name: "Path".into(), path: r"D:\Foo".into() };
        assert_eq!(evidence_line(&missing), strings::evidence_key_missing_path(r"HKCU\Software\Foo", r"D:\Foo"));
        assert_eq!(evidence_line(&Evidence::RegKeyOrphan { key }), strings::evidence_key(r"HKCU\Software\Foo"));
        let run = Evidence::DanglingRef {
            kind: RefKind::Run,
            location: r"LocalMachine\SOFTWARE\Microsoft\Windows\CurrentVersion\Run".into(),
            target: r"C:\Foo\foo.exe".into(),
            value_name: Some("Foo".into()),
        };
        assert_eq!(
            evidence_line(&run),
            strings::evidence_ref(strings::REF_RUN, r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Run", r"C:\Foo\foo.exe")
        );
        let task = Evidence::DanglingRef {
            kind: RefKind::ScheduledTask,
            location: r"Vendor\Updater".into(),
            target: r"C:\Foo\up.exe".into(),
            value_name: None,
        };
        assert_eq!(evidence_line(&task), strings::evidence_ref(strings::REF_TASK, r"Vendor\Updater", r"C:\Foo\up.exe"));
        let when = DateTime::from_timestamp(1_792_000_000, 0).unwrap();
        let usage = Evidence::UsageTrace { exe: r"C:\Foo\foo.exe".into(), last_run: when, source: UsageSource::Prefetch };
        assert_eq!(
            evidence_line(&usage),
            strings::evidence_usage(&format::ymd(Some(format::local_date(when))), strings::SOURCE_PREFETCH, r"C:\Foo\foo.exe")
        );
    }

    #[test]
    fn uncertainty_lists_reasons_and_a_separate_note_once() {
        let mut g = ghost("Foo", Assessment::Uncertain, 1);
        assert_eq!(uncertainty_text(&g), None);
        g.reasons = vec![REASON_SIBLING_OWNED.into(), REASON_OTHER_PROFILE.into()];
        g.note = Some(REASON_OTHER_PROFILE.into());
        let both = format!("{}{}{}", strings::NOTE_SIBLING_OWNED, strings::LIST_SEPARATOR, strings::NOTE_OTHER_PROFILE);
        assert_eq!(uncertainty_text(&g), Some(both));
        g.reasons.clear();
        assert_eq!(uncertainty_text(&g).as_deref(), Some(strings::NOTE_OTHER_PROFILE));
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
        assert_eq!(assessment_label(Assessment::ProbablyUninstalled), strings::ASSESS_PROBABLY);
        assert_eq!(assessment_label(Assessment::LeftoversOnly), strings::ASSESS_LEFTOVERS);
        assert_eq!(assessment_label(Assessment::Uncertain), strings::ASSESS_UNCERTAIN);
        assert_eq!(section_title(Assessment::ProbablyUninstalled, 2), "Troligen avinstallerade (2)");
        assert_eq!(section_title(Assessment::LeftoversOnly, 1), "Rester utan installation (bocka i själv) (1)");
        assert_eq!(section_title(Assessment::Uncertain, 3), "Osäkra (3)");
        assert_eq!(ref_label(RefKind::ClassesServer), strings::REF_CLASSES_SERVER);
        assert_eq!(ref_label(RefKind::Shortcut), strings::REF_SHORTCUT);
    }
}
