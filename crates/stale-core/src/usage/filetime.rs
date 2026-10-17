use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};

use super::{UsageHit, UsageProvider, UsageSource};

/// Seconds between 1601-01-01 and 1970-01-01.
const EPOCH_DIFF_SECS: u64 = 11_644_473_600;
/// 1990-01-01T00:00:00Z as unix seconds.
const MIN_UNIX_SECS: i64 = 631_152_000;

/// Converts a FILETIME (100 ns ticks since 1601) to UTC. Returns `None` for 0,
/// anything before 1990-01-01 and anything later than now + 1 day.
pub fn filetime_to_utc(ft: u64) -> Option<DateTime<Utc>> {
    let secs = (ft / 10_000_000).checked_sub(EPOCH_DIFF_SECS)?;
    let secs = i64::try_from(secs).ok()?;
    if secs < MIN_UNIX_SECS {
        return None;
    }
    let nanos = ((ft % 10_000_000) * 100) as u32;
    let dt = DateTime::from_timestamp(secs, nanos)?;
    if dt > Utc::now() + Duration::days(1) {
        return None;
    }
    Some(dt)
}

/// Last-modified times of exe files: a weak "last used" fallback. Last-access times are
/// ignored because indexers and antivirus scans bump them constantly.
pub struct FileTimeProvider {
    pub exes: Vec<PathBuf>,
}

impl UsageProvider for FileTimeProvider {
    fn name(&self) -> &'static str {
        "File times"
    }

    /// One hit per existing file, dated by its modified time; missing files are skipped.
    fn collect(&self) -> Result<Vec<UsageHit>, String> {
        Ok(self
            .exes
            .iter()
            .filter_map(|exe| {
                let meta = std::fs::metadata(exe).ok()?;
                Some(UsageHit {
                    exe_path: exe.clone(),
                    last_run: DateTime::<Utc>::from(meta.modified().ok()?),
                    source: UsageSource::FileTime,
                    run_count: None,
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime_zero_and_absurd_are_none() {
        assert!(filetime_to_utc(0).is_none());
        assert!(filetime_to_utc(u64::MAX).is_none());
    }

    #[test]
    fn filetime_old_and_future_are_none() {
        assert!(filetime_to_utc(100_000_000_000_000_000).is_none()); // 1917
        let future = (Utc::now() + Duration::days(3)).timestamp() as u64;
        assert!(filetime_to_utc((future + EPOCH_DIFF_SECS) * 10_000_000).is_none());
    }

    #[test]
    fn filetime_valid_converts() {
        let dt = filetime_to_utc(133_500_000_000_000_000).unwrap();
        assert_eq!(dt.timestamp(), 13_350_000_000 - EPOCH_DIFF_SECS as i64);
    }
}

#[cfg(test)]
mod provider_tests {
    use super::*;

    #[test]
    fn file_time_provider_reports_existing_files_and_skips_missing() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("app.exe");
        std::fs::write(&exe, b"MZ").unwrap();
        let p = FileTimeProvider { exes: vec![exe.clone(), dir.path().join("gone.exe")] };
        let hits = p.collect().unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].exe_path, exe);
        assert_eq!(hits[0].source, UsageSource::FileTime);
        assert_eq!(hits[0].run_count, None);
        let modified = std::fs::metadata(&exe).unwrap().modified().unwrap();
        assert_eq!(hits[0].last_run, DateTime::<Utc>::from(modified));
    }
}
