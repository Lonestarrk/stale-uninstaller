//! Swedish display formatting of dates and sizes (no egui).

use std::collections::BTreeMap;

use chrono::{DateTime, Local, NaiveDate, Utc};
use stale_core::usage::UsageSource;

use crate::strings;

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// The calendar date of `t` in the local time zone.
pub fn local_date(t: DateTime<Utc>) -> NaiveDate {
    t.with_timezone(&Local).date_naive()
}

/// "Idag", "Igår", "5 dagar sedan", "14 mån sedan", "4 år sedan" or "Aldrig sedd", counted
/// in local calendar days (yesterday 23:59 is "Igår" at 00:01). Future times count as today.
pub fn relative_date(when: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    let Some(when) = when else {
        return strings::NEVER_SEEN.to_string();
    };
    let days = (local_date(now) - local_date(when)).num_days().max(0);
    let months = days / 30;
    match days {
        0 => strings::TODAY.to_string(),
        1 => strings::YESTERDAY.to_string(),
        2..=29 => format!("{days} {}", strings::DAYS_AGO),
        _ if months < 36 => format!("{months} {}", strings::MONTHS_AGO),
        _ => format!("{} {}", days / 365, strings::YEARS_AGO),
    }
}

/// "–" when unknown, whole MB below 1 GiB (at least 1 for non-empty), otherwise GB with
/// one decimal and a Swedish comma.
pub fn human_size(bytes: Option<u64>) -> String {
    let Some(bytes) = bytes else {
        return strings::UNKNOWN_VALUE.to_string();
    };
    if bytes < GIB {
        let mb = if bytes > 0 { (bytes / MIB).max(1) } else { 0 };
        format!("{mb} {}", strings::MB)
    } else {
        let gb = format!("{:.1}", bytes as f64 / GIB as f64).replace('.', ",");
        format!("{gb} {}", strings::GB)
    }
}

/// "C: 38 MB · D: 41 MB"; "0 MB" when nothing is held on any drive.
pub fn drive_sizes(per_drive: &BTreeMap<char, u64>) -> String {
    if per_drive.is_empty() {
        return human_size(Some(0));
    }
    let parts: Vec<String> = per_drive.iter().map(|(d, &b)| format!("{d}: {}", human_size(Some(b)))).collect();
    parts.join(strings::LIST_SEPARATOR)
}

/// "2024-01-20" or "–".
pub fn ymd(d: Option<NaiveDate>) -> String {
    d.map_or_else(|| strings::UNKNOWN_VALUE.to_string(), |d| d.format("%Y-%m-%d").to_string())
}

/// Display name of where a usage date came from.
pub fn source_label(s: UsageSource) -> &'static str {
    match s {
        UsageSource::Prefetch => strings::SOURCE_PREFETCH,
        UsageSource::Bam => strings::SOURCE_BAM,
        UsageSource::UserAssist => strings::SOURCE_USERASSIST,
        UsageSource::FileTime => strings::SOURCE_FILETIME,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone};

    use super::*;

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
