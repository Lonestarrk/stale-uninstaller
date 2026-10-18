//! Row filtering, sorting and bucket counts for the program list (no egui).

use std::cmp::Ordering;
use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use crate::app::Row;

/// Days without use before a program counts as unused for six months.
const SIX_MONTHS_DAYS: i64 = 183;
/// Days without use before a program counts as unused for a year.
const ONE_YEAR_DAYS: i64 = 365;
/// Threshold of the "Över 1 GB" bucket.
const ONE_GB: u64 = 1024 * 1024 * 1024;

/// Last-used bucket. The `Over*` buckets include never-seen rows: they are candidates too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageBucket {
    All,
    OverSixMonths,
    OverOneYear,
    NeverSeen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeBucket {
    All,
    OverOneGb,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Filters {
    pub usage: UsageBucket,
    pub drive: Option<char>,
    pub size: SizeBucket,
    pub search: String,
}

impl Default for Filters {
    fn default() -> Self {
        Filters { usage: UsageBucket::All, drive: None, size: SizeBucket::All, search: String::new() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Publisher,
    Drive,
    Size,
    Installed,
    LastUsed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    pub ascending: bool,
}

impl Default for Sort {
    /// Oldest first, never-seen before everything.
    fn default() -> Self {
        Sort { key: SortKey::LastUsed, ascending: true }
    }
}

impl Sort {
    /// The sort after clicking `key`'s header: the same key flips direction, a new key starts ascending.
    pub fn toggled(self, key: SortKey) -> Sort {
        if self.key == key {
            Sort { key, ascending: !self.ascending }
        } else {
            Sort { key, ascending: true }
        }
    }
}

/// Number of rows in each filter-panel entry (each bucket counted on its own).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BucketCounts {
    pub all: usize,
    pub over_six_months: usize,
    pub over_one_year: usize,
    pub never_seen: usize,
    pub per_drive: BTreeMap<char, usize>,
    pub over_one_gb: usize,
}

/// True when the program was never seen in use or not for more than 183 days.
pub fn is_stale(last_used: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    unused_for_more_than(last_used, now, SIX_MONTHS_DAYS)
}

/// True for `None` or a time more than `days` whole days before `now`.
fn unused_for_more_than(last_used: Option<DateTime<Utc>>, now: DateTime<Utc>, days: i64) -> bool {
    last_used.is_none_or(|t| (now - t).num_days() > days)
}

/// Rows whose usage is not known yet (`row.usage == None`) match only `All`.
fn usage_matches(bucket: UsageBucket, row: &Row, now: DateTime<Utc>) -> bool {
    if row.usage.is_none() {
        return bucket == UsageBucket::All;
    }
    let last_used = row.last_used();
    match bucket {
        UsageBucket::All => true,
        UsageBucket::OverSixMonths => unused_for_more_than(last_used, now, SIX_MONTHS_DAYS),
        UsageBucket::OverOneYear => unused_for_more_than(last_used, now, ONE_YEAR_DAYS),
        UsageBucket::NeverSeen => last_used.is_none(),
    }
}

fn size_matches(bucket: SizeBucket, bytes: Option<u64>) -> bool {
    match bucket {
        SizeBucket::All => true,
        SizeBucket::OverOneGb => bytes.is_some_and(|b| b > ONE_GB),
    }
}

/// True when the row passes every filter; search is a case-insensitive substring of name or publisher.
pub fn row_matches(row: &Row, f: &Filters, now: DateTime<Utc>) -> bool {
    usage_matches(f.usage, row, now)
        && f.drive.is_none_or(|d| row.program.drive == Some(d))
        && size_matches(f.size, row.size_bytes())
        && search_matches(row, &f.search)
}

fn search_matches(row: &Row, search: &str) -> bool {
    let needle = search.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    row.program.name.to_lowercase().contains(&needle)
        || row.program.publisher.as_ref().is_some_and(|p| p.to_lowercase().contains(&needle))
}

/// Compares known values in `ascending` direction; unknown values sort last either way.
fn cmp_known_first<T: Ord>(a: Option<T>, b: Option<T>, ascending: bool) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => directed(a.cmp(&b), ascending),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn directed(o: Ordering, ascending: bool) -> Ordering {
    if ascending {
        o
    } else {
        o.reverse()
    }
}

fn cmp_by(a: &Row, b: &Row, s: Sort) -> Ordering {
    let lower = |t: &Option<String>| t.as_ref().map(|t| t.to_lowercase());
    match s.key {
        SortKey::Name => directed(a.program.name.to_lowercase().cmp(&b.program.name.to_lowercase()), s.ascending),
        SortKey::Publisher => cmp_known_first(lower(&a.program.publisher), lower(&b.program.publisher), s.ascending),
        SortKey::Drive => cmp_known_first(a.program.drive, b.program.drive, s.ascending),
        SortKey::Size => cmp_known_first(a.size_bytes(), b.size_bytes(), s.ascending),
        SortKey::Installed => cmp_known_first(a.program.install_date, b.program.install_date, s.ascending),
        // `None < Some`: never-seen first ascending, last descending.
        SortKey::LastUsed => directed(a.last_used().cmp(&b.last_used()), s.ascending),
    }
}

/// Indices of the rows passing `f`, in `s` order; stable, ties broken by name (case-insensitive).
pub fn visible_rows(rows: &[Row], f: &Filters, s: Sort, now: DateTime<Utc>) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..rows.len()).filter(|&i| row_matches(&rows[i], f, now)).collect();
    idx.sort_by(|&i, &j| {
        let (a, b) = (&rows[i], &rows[j]);
        cmp_by(a, b, s).then_with(|| a.program.name.to_lowercase().cmp(&b.program.name.to_lowercase()))
    });
    idx
}

/// Counts for every entry of the filter panel.
pub fn bucket_counts(rows: &[Row], now: DateTime<Utc>) -> BucketCounts {
    let mut c = BucketCounts { all: rows.len(), ..BucketCounts::default() };
    for row in rows {
        c.over_six_months += usize::from(usage_matches(UsageBucket::OverSixMonths, row, now));
        c.over_one_year += usize::from(usage_matches(UsageBucket::OverOneYear, row, now));
        c.never_seen += usize::from(usage_matches(UsageBucket::NeverSeen, row, now));
        c.over_one_gb += usize::from(size_matches(SizeBucket::OverOneGb, row.size_bytes()));
        if let Some(d) = row.program.drive {
            *c.per_drive.entry(d).or_default() += 1;
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, NaiveDate, TimeZone};
    use stale_core::usage::matching::{LastUsed, ProgramUsage};
    use stale_core::{Hive, Program, ProgramId, RegKeyPath, Scope};

    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 18, 12, 0, 0).unwrap()
    }

    /// `days_ago: None` is a never-seen row (usage known, no date); see `pending` for unknown usage.
    fn row(name: &str, days_ago: Option<i64>) -> Row {
        Row {
            program: Program {
                account: None,
                id: ProgramId(name.to_string()),
                key: RegKeyPath { hive: Hive::LocalMachine, path: name.to_string() },
                name: name.to_string(),
                publisher: None,
                version: None,
                install_location: None,
                install_date: None,
                uninstall_string: None,
                quiet_uninstall_string: None,
                display_icon: None,
                is_msi: false,
                estimated_size_bytes: None,
                drive: Some('C'),
                hidden: false,
                scope: Scope::Machine,
            },
            usage: Some(ProgramUsage {
                last_used: LastUsed { when: days_ago.map(|d| now() - Duration::days(d)), source: None, confident: true },
                exes: Vec::new(),
            }),
            size: None,
        }
    }

    /// A row whose usage has not arrived yet.
    fn pending(name: &str) -> Row {
        Row { usage: None, ..row(name, None) }
    }

    fn with(mut r: Row, drive: Option<char>, size: Option<u64>, publisher: Option<&str>) -> Row {
        r.program.drive = drive;
        r.size = size;
        r.program.publisher = publisher.map(str::to_string);
        r
    }

    fn usage_filter(usage: UsageBucket) -> Filters {
        Filters { usage, ..Filters::default() }
    }

    fn names(rows: &[Row], idx: &[usize]) -> Vec<String> {
        idx.iter().map(|&i| rows[i].program.name.clone()).collect()
    }

    #[test]
    fn default_sort_puts_never_seen_first_then_oldest() {
        let rows = vec![row("today", Some(0)), row("old", Some(400)), row("never", None)];
        assert_eq!(visible_rows(&rows, &Filters::default(), Sort::default(), now()), vec![2, 1, 0]);
        let desc = Sort { key: SortKey::LastUsed, ascending: false };
        assert_eq!(visible_rows(&rows, &Filters::default(), desc, now()), vec![0, 1, 2]);
    }

    #[test]
    fn usage_buckets_include_never_seen() {
        let (n, r200, r10, r400) = (row("n", None), row("a", Some(200)), row("b", Some(10)), row("c", Some(400)));
        let six = usage_filter(UsageBucket::OverSixMonths);
        assert!(row_matches(&r200, &six, now()));
        assert!(row_matches(&n, &six, now()));
        assert!(!row_matches(&r10, &six, now()));

        let year = usage_filter(UsageBucket::OverOneYear);
        assert!(row_matches(&r400, &year, now()));
        assert!(row_matches(&n, &year, now()));
        assert!(!row_matches(&r200, &year, now()));

        let never = usage_filter(UsageBucket::NeverSeen);
        assert!(row_matches(&n, &never, now()));
        assert!(!row_matches(&r200, &never, now()));
        assert!(!row_matches(&r400, &never, now()));

        let all = usage_filter(UsageBucket::All);
        assert!([&n, &r200, &r10, &r400].iter().all(|r| row_matches(r, &all, now())));
    }

    #[test]
    fn pending_rows_match_only_all() {
        let p = pending("p");
        assert!(row_matches(&p, &usage_filter(UsageBucket::All), now()));
        for bucket in [UsageBucket::OverSixMonths, UsageBucket::OverOneYear, UsageBucket::NeverSeen] {
            assert!(!row_matches(&p, &usage_filter(bucket), now()), "{bucket:?}");
        }
        let c = bucket_counts(&[p, row("n", None)], now());
        assert_eq!((c.all, c.over_six_months, c.over_one_year, c.never_seen), (2, 1, 1, 1));
    }

    #[test]
    fn stale_threshold_is_183_days() {
        assert!(is_stale(None, now()));
        assert!(is_stale(Some(now() - Duration::days(184)), now()));
        assert!(!is_stale(Some(now() - Duration::days(183)), now()));
        assert!(!is_stale(Some(now()), now()));
    }

    #[test]
    fn drive_and_size_and_search_filters_combine_with_and() {
        let gb2 = Some(2 * ONE_GB);
        let rows = vec![
            with(row("Pass", None), Some('D'), gb2, Some("Contoso")),
            with(row("WrongDrive", None), Some('C'), gb2, Some("Contoso")),
            with(row("Small", None), Some('D'), Some(10), Some("Contoso")),
            with(row("NoMatch", None), Some('D'), gb2, Some("Fabrikam")),
        ];
        let f = Filters {
            usage: UsageBucket::All,
            drive: Some('D'),
            size: SizeBucket::OverOneGb,
            search: "CONTOSO".to_string(),
        };
        assert_eq!(visible_rows(&rows, &f, Sort::default(), now()), vec![0]);
    }

    #[test]
    fn search_matches_name_or_publisher_case_insensitively() {
        let r = with(row("Visual Studio Code", None), None, None, Some("Microsoft Corporation"));
        let search = |s: &str| Filters { search: s.to_string(), ..Filters::default() };
        assert!(row_matches(&r, &search("studio"), now()));
        assert!(row_matches(&r, &search("MICROSOFT"), now()));
        assert!(row_matches(&r, &search("  "), now()), "blank search matches everything");
        assert!(!row_matches(&r, &search("adobe"), now()));
    }

    #[test]
    fn size_prefers_estimate_over_measured() {
        let mut r = row("a", None);
        r.size = Some(2 * ONE_GB);
        assert_eq!(r.size_bytes(), Some(2 * ONE_GB));
        let big = Filters { size: SizeBucket::OverOneGb, ..Filters::default() };
        assert!(row_matches(&r, &big, now()));
        r.program.estimated_size_bytes = Some(5);
        assert_eq!(r.size_bytes(), Some(5));
        assert!(!row_matches(&r, &big, now()));
    }

    #[test]
    fn bucket_counts_match_filters() {
        let rows = vec![
            with(row("a", None), Some('C'), Some(2 * ONE_GB), None),
            with(row("b", Some(10)), Some('D'), Some(10), None),
            with(row("c", Some(200)), Some('C'), None, None),
            with(row("d", Some(400)), None, Some(ONE_GB + 1), None),
            with(row("e", None), Some('E'), None, None),
            pending("f"),
        ];
        let count = |f: Filters| rows.iter().filter(|r| row_matches(r, &f, now())).count();
        let c = bucket_counts(&rows, now());
        assert_eq!(c.all, count(Filters::default()));
        assert_eq!(c.over_six_months, count(usage_filter(UsageBucket::OverSixMonths)));
        assert_eq!(c.over_one_year, count(usage_filter(UsageBucket::OverOneYear)));
        assert_eq!(c.never_seen, count(usage_filter(UsageBucket::NeverSeen)));
        assert_eq!(c.over_one_gb, count(Filters { size: SizeBucket::OverOneGb, ..Filters::default() }));
        for (&d, &n) in &c.per_drive {
            assert_eq!(n, count(Filters { drive: Some(d), ..Filters::default() }), "drive {d}");
        }
        assert_eq!(c.per_drive, BTreeMap::from([('C', 3), ('D', 1), ('E', 1)]));
        assert_eq!((c.all, c.over_six_months, c.over_one_year, c.never_seen, c.over_one_gb), (6, 4, 3, 2, 2));
    }

    #[test]
    fn ties_break_by_name_and_drive_none_sorts_last() {
        let rows = vec![
            with(row("beta", None), Some('C'), None, None),
            with(row("none", None), None, None, None),
            with(row("Alpha", None), Some('C'), None, None),
            with(row("delta", None), Some('D'), None, None),
        ];
        let by =
            |key, ascending| names(&rows, &visible_rows(&rows, &Filters::default(), Sort { key, ascending }, now()));
        assert_eq!(by(SortKey::Drive, true), ["Alpha", "beta", "delta", "none"]);
        assert_eq!(by(SortKey::Drive, false), ["delta", "Alpha", "beta", "none"]);
        assert_eq!(by(SortKey::Name, true), ["Alpha", "beta", "delta", "none"]);
        assert_eq!(by(SortKey::Name, false), ["none", "delta", "beta", "Alpha"]);
        // All never-seen: last-used ties fall back to name.
        assert_eq!(by(SortKey::LastUsed, true), ["Alpha", "beta", "delta", "none"]);
    }

    #[test]
    fn size_and_installed_put_unknown_last_both_ways() {
        let mut rows = vec![
            with(row("unknown", None), None, None, None),
            with(row("small", None), None, Some(10), None),
            with(row("big", None), None, Some(ONE_GB), None),
        ];
        rows[1].program.install_date = NaiveDate::from_ymd_opt(2020, 1, 1);
        rows[2].program.install_date = NaiveDate::from_ymd_opt(2024, 1, 1);
        let by =
            |key, ascending| names(&rows, &visible_rows(&rows, &Filters::default(), Sort { key, ascending }, now()));
        assert_eq!(by(SortKey::Size, true), ["small", "big", "unknown"]);
        assert_eq!(by(SortKey::Size, false), ["big", "small", "unknown"]);
        assert_eq!(by(SortKey::Installed, true), ["small", "big", "unknown"]);
        assert_eq!(by(SortKey::Installed, false), ["big", "small", "unknown"]);
    }

    #[test]
    fn header_click_flips_same_key_and_starts_new_key_ascending() {
        let s = Sort::default();
        assert_eq!(s.toggled(SortKey::LastUsed), Sort { key: SortKey::LastUsed, ascending: false });
        let by_name = Sort { key: SortKey::Drive, ascending: false }.toggled(SortKey::Name);
        assert_eq!(by_name, Sort { key: SortKey::Name, ascending: true });
        assert_eq!(by_name.toggled(SortKey::Name), Sort { key: SortKey::Name, ascending: false });
    }

    #[test]
    fn publisher_sort_is_case_insensitive() {
        let rows =
            vec![with(row("x", None), None, None, Some("zeta")), with(row("y", None), None, None, Some("Alpha"))];
        let s = Sort { key: SortKey::Publisher, ascending: true };
        assert_eq!(visible_rows(&rows, &Filters::default(), s, now()), vec![1, 0]);
    }
}
