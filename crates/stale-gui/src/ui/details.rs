//! Bottom panel with the selected program's details and per-exe usage.

use eframe::egui;
use egui_extras::{Column, TableBuilder};
use stale_core::usage::matching::{ExcludeReason, ExeUsage, ProgramUsage};
use stale_core::{Hive, Scope};

use crate::app::Row;
use crate::strings::Lang;
use crate::{format, strings};

const ROW_HEIGHT: f32 = 20.0;

/// The reason an exe does not count towards "last used".
pub fn exclude_label(reason: ExcludeReason) -> &'static str {
    match reason {
        ExcludeReason::NamePattern => strings::t().excluded_name_pattern,
        ExcludeReason::Service => strings::t().excluded_service,
        ExcludeReason::ScheduledTask => strings::t().excluded_scheduled_task,
        ExcludeReason::NotMain => strings::t().excluded_not_main,
    }
}

/// "✓" for a counted main exe, "räknas" for a counted subcomponent, else the reason it is left
/// out; a counted autostart exe gets " (autostart)".
fn counted_label(exe: &ExeUsage) -> String {
    let label = match exe.excluded {
        Some(reason) => return exclude_label(reason).to_string(),
        None if exe.is_main => strings::t().counted_yes,
        None => strings::t().counted_subcomponent,
    };
    if exe.autostart { format!("{label} ({})", strings::t().autostart) } else { label.to_string() }
}

fn hive_label(hive: Hive) -> &'static str {
    match hive {
        Hive::LocalMachine => "HKLM",
        Hive::CurrentUser => "HKCU",
        Hive::Users => "HKU",
    }
}

/// "2026-10-18 (BAM)", plus the table's notes: "2026-10-18 (BAM) (autostart, via x.exe)".
fn last_used_text_in(lang: Lang, usage: Option<&ProgramUsage>) -> String {
    let s = strings::texts(lang);
    let Some(usage) = usage else { return s.pending.to_string() };
    let (Some(when), Some(source)) = (usage.last_used.when, usage.last_used.source) else {
        return s.never_seen.to_string();
    };
    let mut text = format!("{} ({})", format::ymd(Some(format::local_date(when))), format::source_label_in(lang, source));
    if let Some(notes) = format::usage_notes_in(lang, usage.last_used.via.as_deref(), usage.last_used.autostart) {
        text.push_str(&format!(" ({notes})"));
    }
    if !usage.last_used.confident {
        text.push_str(s.uncertain_filetime);
    }
    text
}

fn exe_file_name(exe: &ExeUsage) -> String {
    exe.exe_path.file_name().map_or_else(|| exe.exe_path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// The date, plus the run count when it is known and non-zero (UserAssist often stores 0).
fn last_run_text(exe: &ExeUsage) -> String {
    let date = format::ymd(Some(format::local_date(exe.last_run)));
    match exe.run_count {
        Some(n) if n > 0 => format!("{date} ({})", strings::run_count(n)),
        _ => date,
    }
}

/// Details of the selected program: where it lives, how it uninstalls and per-exe usage.
pub fn details_panel(ui: &mut egui::Ui, row: &Row) {
    let p = &row.program;
    let or_dash = |s: Option<String>| s.unwrap_or_else(|| strings::t().unknown_value.to_string());
    egui::Grid::new("details_grid").num_columns(2).show(ui, |ui| {
        let location = p.install_location.as_ref().map(|l| l.display().to_string());
        let mut key = format!("{}\\{}", hive_label(p.key.hive), p.key.path);
        if p.scope == Scope::OtherUser {
            key.push_str(strings::t().other_user_suffix);
        }
        for (label, value) in [
            (strings::t().detail_location, or_dash(location)),
            (strings::t().detail_uninstaller, or_dash(p.uninstall_string.clone())),
            (strings::t().detail_registry_key, key),
            (strings::t().detail_last_used, last_used_text_in(strings::lang(), row.usage.as_ref())),
        ] {
            ui.label(egui::RichText::new(label).strong());
            ui.add(egui::Label::new(value).truncate());
            ui.end_row();
        }
    });
    let Some(usage) = row.usage.as_ref().filter(|u| !u.exes.is_empty()) else { return };
    ui.separator();
    TableBuilder::new(ui)
        .id_salt("details_exes")
        .striped(true)
        .column(Column::initial(220.0).clip(true))
        .column(Column::initial(150.0))
        .column(Column::initial(90.0))
        .column(Column::remainder())
        .header(ROW_HEIGHT, |mut header| {
            for title in [strings::t().col_file, strings::t().col_last_run, strings::t().col_source, strings::t().col_counted] {
                header.col(|ui| {
                    ui.strong(title);
                });
            }
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, usage.exes.len(), |mut r| {
                let exe = &usage.exes[r.index()];
                r.col(|ui| {
                    ui.label(exe_file_name(exe)).on_hover_text(exe.exe_path.display().to_string());
                });
                r.col(|ui| {
                    ui.label(last_run_text(exe));
                });
                r.col(|ui| {
                    ui.label(format::source_label(exe.source));
                });
                r.col(|ui| {
                    ui.label(counted_label(exe));
                });
            });
        });
}

#[cfg(test)]
mod tests {
    use stale_core::usage::matching::ExcludeReason;

    use super::*;
    use crate::strings::{self, Lang};

    #[test]
    fn counted_label_marks_subcomponents() {
        let exe = |is_main, excluded| ExeUsage {
            exe_path: r"C:\a\b.exe".into(),
            last_run: chrono::Utc::now(),
            source: stale_core::usage::UsageSource::Bam,
            run_count: None,
            is_main,
            excluded,
            autostart: false,
        };
        assert_eq!(counted_label(&exe(true, None)), strings::t().counted_yes);
        assert_eq!(counted_label(&exe(false, None)), "räknas");
        assert_eq!(counted_label(&exe(false, Some(ExcludeReason::Service))), strings::t().excluded_service);
        let auto = ExeUsage { autostart: true, ..exe(true, None) };
        assert_eq!(counted_label(&auto), format!("{} (autostart)", strings::t().counted_yes));
    }

    #[test]
    fn last_used_text_uses_the_tables_form_in_both_languages() {
        use chrono::{Local, TimeZone};
        use stale_core::usage::matching::{LastUsed, ProgramUsage};
        use stale_core::usage::UsageSource;
        let when = Local.with_ymd_and_hms(2026, 10, 18, 12, 0, 0).single().unwrap().with_timezone(&chrono::Utc);
        let last_used =
            LastUsed { when: Some(when), source: Some(UsageSource::Bam), confident: true, via: Some("x.exe".into()), autostart: true };
        let usage = ProgramUsage { last_used, exes: Vec::new() };
        assert_eq!(last_used_text_in(Lang::Sv, Some(&usage)), "2026-10-18 (BAM) (autostart, via x.exe)");
        assert_eq!(last_used_text_in(Lang::En, Some(&usage)), "2026-10-18 (BAM) (autostart, via x.exe)");
        let mut plain = usage.clone();
        plain.last_used.via = None;
        plain.last_used.autostart = false;
        assert_eq!(last_used_text_in(Lang::Sv, Some(&plain)), "2026-10-18 (BAM)");
        plain.last_used.source = Some(UsageSource::FileTime);
        plain.last_used.confident = false;
        assert_eq!(last_used_text_in(Lang::Sv, Some(&plain)), "2026-10-18 (filtid) – osäker (filtid)");
        assert_eq!(last_used_text_in(Lang::En, Some(&plain)), "2026-10-18 (file time) – uncertain (file time)");
        assert_eq!(last_used_text_in(Lang::En, None), "…");
        plain.last_used.when = None;
        assert_eq!(last_used_text_in(Lang::En, Some(&plain)), "Never seen");
    }

    #[test]
    fn exclude_reason_labels_are_swedish() {
        assert_eq!(exclude_label(ExcludeReason::NamePattern), strings::t().excluded_name_pattern);
        assert_eq!(exclude_label(ExcludeReason::Service), strings::t().excluded_service);
        assert_eq!(exclude_label(ExcludeReason::NotMain), strings::t().excluded_not_main);
        for r in [ExcludeReason::NamePattern, ExcludeReason::Service, ExcludeReason::ScheduledTask, ExcludeReason::NotMain] {
            assert!(!exclude_label(r).is_empty());
        }
    }
}
