//! Bottom panel with the selected program's details and per-exe usage.

use eframe::egui;
use egui_extras::{Column, TableBuilder};
use stale_core::usage::matching::{ExcludeReason, ExeUsage};
use stale_core::{Hive, Scope};

use crate::app::Row;
use crate::{format, strings};

const ROW_HEIGHT: f32 = 20.0;

/// Swedish reason an exe does not count towards "last used".
pub fn exclude_label(reason: ExcludeReason) -> &'static str {
    match reason {
        ExcludeReason::NamePattern => strings::EXCLUDED_NAME_PATTERN,
        ExcludeReason::Service => strings::EXCLUDED_SERVICE,
        ExcludeReason::ScheduledTask => strings::EXCLUDED_SCHEDULED_TASK,
        ExcludeReason::NotMain => strings::EXCLUDED_NOT_MAIN,
    }
}

/// "✓" for a counted main exe, "räknas" for a counted subcomponent, else the reason it is left out.
fn counted_label(exe: &ExeUsage) -> &'static str {
    match exe.excluded {
        Some(reason) => exclude_label(reason),
        None if exe.is_main => strings::COUNTED_YES,
        None => strings::COUNTED_SUBCOMPONENT,
    }
}

fn hive_label(hive: Hive) -> &'static str {
    match hive {
        Hive::LocalMachine => "HKLM",
        Hive::CurrentUser => "HKCU",
        Hive::Users => "HKU",
    }
}

fn last_used_text(row: &Row) -> String {
    let Some(usage) = &row.usage else { return strings::PENDING.to_string() };
    let (Some(when), Some(source)) = (usage.last_used.when, usage.last_used.source) else {
        return strings::NEVER_SEEN.to_string();
    };
    let mut text = format!("{} ({})", format::ymd(Some(format::local_date(when))), format::source_label(source));
    if let Some(via) = &usage.last_used.via {
        text.push_str(&format!(" {} {via}", strings::VIA));
    }
    if !usage.last_used.confident {
        text.push_str(strings::UNCERTAIN_FILETIME);
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
        Some(n) if n > 0 => format!("{date} ({n} {})", strings::RUNS),
        _ => date,
    }
}

/// Details of the selected program: where it lives, how it uninstalls and per-exe usage.
pub fn details_panel(ui: &mut egui::Ui, row: &Row) {
    let p = &row.program;
    let or_dash = |s: Option<String>| s.unwrap_or_else(|| strings::UNKNOWN_VALUE.to_string());
    egui::Grid::new("details_grid").num_columns(2).show(ui, |ui| {
        let location = p.install_location.as_ref().map(|l| l.display().to_string());
        let mut key = format!("{}\\{}", hive_label(p.key.hive), p.key.path);
        if p.scope == Scope::OtherUser {
            key.push_str(strings::OTHER_USER_SUFFIX);
        }
        for (label, value) in [
            (strings::DETAIL_LOCATION, or_dash(location)),
            (strings::DETAIL_UNINSTALLER, or_dash(p.uninstall_string.clone())),
            (strings::DETAIL_REGISTRY_KEY, key),
            (strings::DETAIL_LAST_USED, last_used_text(row)),
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
            for title in [strings::COL_FILE, strings::COL_LAST_RUN, strings::COL_SOURCE, strings::COL_COUNTED] {
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
    use crate::strings;

    #[test]
    fn counted_label_marks_subcomponents() {
        let exe = |is_main, excluded| ExeUsage {
            exe_path: r"C:\a\b.exe".into(),
            last_run: chrono::Utc::now(),
            source: stale_core::usage::UsageSource::Bam,
            run_count: None,
            is_main,
            excluded,
        };
        assert_eq!(counted_label(&exe(true, None)), strings::COUNTED_YES);
        assert_eq!(counted_label(&exe(false, None)), "räknas");
        assert_eq!(counted_label(&exe(false, Some(ExcludeReason::Service))), strings::EXCLUDED_SERVICE);
    }

    #[test]
    fn exclude_reason_labels_are_swedish() {
        assert_eq!(exclude_label(ExcludeReason::NamePattern), strings::EXCLUDED_NAME_PATTERN);
        assert_eq!(exclude_label(ExcludeReason::Service), strings::EXCLUDED_SERVICE);
        assert_eq!(exclude_label(ExcludeReason::NotMain), strings::EXCLUDED_NOT_MAIN);
        for r in [ExcludeReason::NamePattern, ExcludeReason::Service, ExcludeReason::ScheduledTask, ExcludeReason::NotMain] {
            assert!(!exclude_label(r).is_empty());
        }
    }
}
