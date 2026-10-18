//! Display formatting of dates and sizes in the user's language (no egui).

use std::collections::BTreeMap;

use chrono::{DateTime, Local, NaiveDate, Utc};
use stale_core::usage::UsageSource;

use crate::strings::{self, texts, Lang};

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// The calendar date of `t` in the local time zone.
pub fn local_date(t: DateTime<Utc>) -> NaiveDate {
    t.with_timezone(&Local).date_naive()
}

/// [`relative_date_in`] in the current language.
pub fn relative_date(when: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    relative_date_in(strings::lang(), when, now)
}

/// "Idag", "Igår", "5 dagar sedan", "14 mån sedan", "4 år sedan" or "Aldrig sedd" ("Today",
/// "Yesterday", "5 days ago", "14 mo ago", "4 yr ago", "Never seen"), counted in local calendar
/// days (yesterday 23:59 is "Igår" at 00:01). Future times count as today.
pub fn relative_date_in(lang: Lang, when: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    let s = texts(lang);
    let Some(when) = when else {
        return s.never_seen.to_string();
    };
    let days = (local_date(now) - local_date(when)).num_days().max(0);
    let months = days / 30;
    match days {
        0 => s.today.to_string(),
        1 => s.yesterday.to_string(),
        2..=29 => format!("{days} {}", s.days_ago),
        _ if months < 36 => format!("{months} {}", s.months_ago),
        _ => format!("{} {}", days / 365, s.years_ago),
    }
}

/// [`last_used_label_in`] in the current language.
pub fn last_used_label(when: Option<DateTime<Utc>>, via: Option<&str>, autostart: bool, now: DateTime<Utc>) -> String {
    last_used_label_in(strings::lang(), when, via, autostart, now)
}

/// [`relative_date_in`], plus " (via link.exe)" when the date came from a subcomponent exe and
/// " (autostart)" when it came from an automatic start; both: " (autostart, via link.exe)".
pub fn last_used_label_in(
    lang: Lang,
    when: Option<DateTime<Utc>>,
    via: Option<&str>,
    autostart: bool,
    now: DateTime<Utc>,
) -> String {
    let date = relative_date_in(lang, when, now);
    if when.is_none() {
        return date;
    }
    match usage_notes_in(lang, via, autostart) {
        Some(notes) => format!("{date} ({notes})"),
        None => date,
    }
}

/// "autostart", "via link.exe" or "autostart, via link.exe"; `None` when neither applies.
pub fn usage_notes_in(lang: Lang, via: Option<&str>, autostart: bool) -> Option<String> {
    let s = texts(lang);
    let notes: Vec<String> =
        [autostart.then(|| s.autostart.to_string()), via.map(|exe| format!("{} {exe}", s.via))].into_iter().flatten().collect();
    (!notes.is_empty()).then(|| notes.join(", "))
}

/// [`human_size_in`] in the current language.
pub fn human_size(bytes: Option<u64>) -> String {
    human_size_in(strings::lang(), bytes)
}

/// "–" when unknown, whole MB below 1 GiB (at least 1 for non-empty), otherwise GB with
/// one decimal (a comma in Swedish, a point in English).
pub fn human_size_in(lang: Lang, bytes: Option<u64>) -> String {
    let s = texts(lang);
    let Some(bytes) = bytes else {
        return s.unknown_value.to_string();
    };
    if bytes < GIB {
        let mb = if bytes > 0 { (bytes / MIB).max(1) } else { 0 };
        format!("{mb} {}", s.mb)
    } else {
        let gb = format!("{:.1}", bytes as f64 / GIB as f64);
        let gb = match lang {
            Lang::Sv => gb.replace('.', ","),
            Lang::En => gb,
        };
        format!("{gb} {}", s.gb)
    }
}

/// "C: 38 MB · D: 41 MB"; "0 MB" when nothing is held on any drive.
pub fn drive_sizes(per_drive: &BTreeMap<char, u64>) -> String {
    if per_drive.is_empty() {
        return human_size(Some(0));
    }
    let parts: Vec<String> = per_drive.iter().map(|(d, &b)| format!("{d}: {}", human_size(Some(b)))).collect();
    parts.join(strings::t().list_separator)
}

/// "2024-01-20" or "–".
pub fn ymd(d: Option<NaiveDate>) -> String {
    d.map_or_else(|| strings::t().unknown_value.to_string(), |d| d.format("%Y-%m-%d").to_string())
}

/// Display name of where a usage date came from.
pub fn source_label(s: UsageSource) -> &'static str {
    source_label_in(strings::lang(), s)
}

/// [`source_label`] in `lang`.
pub fn source_label_in(lang: Lang, s: UsageSource) -> &'static str {
    let t = texts(lang);
    match s {
        UsageSource::Prefetch => t.source_prefetch,
        UsageSource::Bam => t.source_bam,
        UsageSource::UserAssist => t.source_userassist,
        UsageSource::FileTime => t.source_filetime,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone};

    use super::*;
    use crate::strings::Lang;

    /// `y-m-d h:mi` local time, as UTC.
    fn local(y: i32, m: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Local.with_ymd_and_hms(y, m, d, h, mi, 0).single().unwrap().with_timezone(&Utc)
    }

    fn now() -> DateTime<Utc> {
        local(2026, 10, 18, 12, 0)
    }

    #[test]
    fn relative_date_counts_local_calendar_days() {
        let just_after_midnight = local(2026, 10, 18, 0, 1);
        assert_eq!(relative_date(Some(local(2026, 10, 17, 23, 59)), just_after_midnight), "Igår");
        assert_eq!(relative_date(Some(local(2026, 10, 18, 0, 0)), local(2026, 10, 18, 23, 59)), "Idag");
        assert_eq!(relative_date(Some(local(2026, 10, 16, 23, 0)), just_after_midnight), "2 dagar sedan");
    }

    #[test]
    fn local_date_uses_local_time_zone() {
        assert_eq!(local_date(local(2026, 10, 17, 23, 59)), NaiveDate::from_ymd_opt(2026, 10, 17).unwrap());
        assert_eq!(local_date(local(2026, 10, 18, 0, 1)), NaiveDate::from_ymd_opt(2026, 10, 18).unwrap());
    }

    #[test]
    fn relative_dates() {
        let now = now();
        let ago = |d: i64| Some(now - Duration::days(d));
        assert_eq!(relative_date(None, now), "Aldrig sedd");
        assert_eq!(relative_date(Some(now), now), "Idag");
        assert_eq!(relative_date(ago(1), now), "Igår");
        assert_eq!(relative_date(ago(5), now), "5 dagar sedan");
        assert_eq!(relative_date(ago(29), now), "29 dagar sedan");
        assert_eq!(relative_date(ago(30), now), "1 mån sedan");
        assert_eq!(relative_date(ago(420), now), "14 mån sedan");
        assert_eq!(relative_date(ago(1079), now), "35 mån sedan");
        assert_eq!(relative_date(ago(1080), now), "2 år sedan");
        assert_eq!(relative_date(ago(1500), now), "4 år sedan");
    }

    #[test]
    fn last_used_label_shows_via() {
        let now = now();
        assert_eq!(last_used_label(Some(now), Some("link.exe"), false, now), "Idag (via link.exe)");
        assert_eq!(last_used_label(Some(now), None, false, now), "Idag");
        assert_eq!(last_used_label(None, Some("link.exe"), false, now), "Aldrig sedd");
    }

    #[test]
    fn last_used_label_shows_autostart() {
        let now = now();
        assert_eq!(last_used_label(Some(now), None, true, now), "Idag (autostart)");
        assert_eq!(last_used_label(None, None, true, now), "Aldrig sedd");
    }

    #[test]
    fn last_used_label_shows_autostart_and_via() {
        let now = now();
        assert_eq!(last_used_label(Some(now), Some("x.exe"), true, now), "Idag (autostart, via x.exe)");
    }

    #[test]
    fn both_languages_render_relative_dates() {
        let now = now();
        let ago = |d: i64| Some(now - Duration::days(d));
        assert_eq!(relative_date_in(Lang::Sv, ago(3), now), "3 dagar sedan");
        assert_eq!(relative_date_in(Lang::En, ago(3), now), "3 days ago");
        assert_eq!(relative_date_in(Lang::Sv, Some(now), now), "Idag");
        assert_eq!(relative_date_in(Lang::En, Some(now), now), "Today");
        assert_eq!(relative_date_in(Lang::En, ago(1), now), "Yesterday");
        assert_eq!(relative_date_in(Lang::En, ago(150), now), "5 mo ago");
        assert_eq!(relative_date_in(Lang::En, ago(1080), now), "2 yr ago");
        assert_eq!(relative_date_in(Lang::En, None, now), "Never seen");
        assert_eq!(last_used_label_in(Lang::Sv, Some(now), Some("link.exe"), true, now), "Idag (autostart, via link.exe)");
        assert_eq!(last_used_label_in(Lang::En, Some(now), Some("link.exe"), true, now), "Today (autostart, via link.exe)");
    }

    #[test]
    fn english_sizes_use_a_decimal_point() {
        assert_eq!(human_size_in(Lang::En, Some(1_288_490_189)), "1.2 GB");
        assert_eq!(human_size_in(Lang::Sv, Some(1_288_490_189)), "1,2 GB");
        assert_eq!(human_size_in(Lang::En, Some(58 * 1024 * 1024)), "58 MB");
    }

    #[test]
    fn future_dates_count_as_today() {
        let now = now();
        assert_eq!(relative_date(Some(now + Duration::days(3)), now), "Idag");
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(Some(58 * 1024 * 1024)), "58 MB");
        assert_eq!(human_size(Some(1_288_490_189)), "1,2 GB");
        assert_eq!(human_size(None), "–");
        assert_eq!(human_size(Some(0)), "0 MB");
        assert_eq!(human_size(Some(1)), "1 MB");
        assert_eq!(human_size(Some(1024 * 1024 * 1024 - 1)), "1023 MB");
        assert_eq!(human_size(Some(1024 * 1024 * 1024)), "1,0 GB");
    }

    #[test]
    fn source_labels() {
        assert_eq!(source_label(UsageSource::Prefetch), "Prefetch");
        assert_eq!(source_label(UsageSource::Bam), "BAM");
        assert_eq!(source_label(UsageSource::UserAssist), "UserAssist");
        assert_eq!(source_label(UsageSource::FileTime), "filtid");
    }

    #[test]
    fn drive_sizes_lists_each_drive() {
        let per = BTreeMap::from([('D', 41 * 1024 * 1024), ('C', 38 * 1024 * 1024)]);
        assert_eq!(drive_sizes(&per), "C: 38 MB · D: 41 MB");
        assert_eq!(drive_sizes(&BTreeMap::new()), "0 MB");
    }

    #[test]
    fn ymd_formats_or_dashes() {
        assert_eq!(ymd(NaiveDate::from_ymd_opt(2024, 1, 20)), "2024-01-20");
        assert_eq!(ymd(None), "–");
    }
}
