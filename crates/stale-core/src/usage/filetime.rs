use chrono::{DateTime, Duration, Utc};

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
