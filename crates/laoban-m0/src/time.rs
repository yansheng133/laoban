//! UTC calendar helpers (no time zone database needed) and plan IDs.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Days since 1970-01-01 to (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

fn secs(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `YYYYMMDD` in UTC.
pub fn ymd(t: SystemTime) -> String {
    let (y, m, d) = civil_from_days(secs(t).div_euclid(86_400));
    format!("{y:04}{m:02}{d:02}")
}

/// RFC 3339 UTC timestamp with second precision.
pub fn rfc3339(t: SystemTime) -> String {
    let s = secs(t);
    let (y, m, d) = civil_from_days(s.div_euclid(86_400));
    let rem = s.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// `p-YYYYMMDD-xxxxxx`: date plus 24 random bits from the standard
/// library's randomly keyed hasher. An identifier, not a secret.
pub fn plan_id(t: SystemTime) -> String {
    let mut h = RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    h.write_u128(
        t.duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    format!("p-{}-{:06x}", ymd(t), h.finish() & 0xff_ffff)
}

/// `j-xxxxxxxxxxxxxxxx`: a background job (64 random bits). Not a
/// secret either: a job is only shown to the user who started it.
pub fn job_id(t: SystemTime) -> String {
    let mut h = RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    h.write_u128(
        t.duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    format!("j-{:016x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn known_dates() {
        let t = UNIX_EPOCH + Duration::from_secs(20_739 * 86_400 + 3_661);
        assert_eq!(ymd(t), "20261013");
        assert_eq!(rfc3339(t), "2026-10-13T01:01:01Z");
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        // 2024-02-29
        assert_eq!(
            ymd(UNIX_EPOCH + Duration::from_secs(19_782 * 86_400)),
            "20240229"
        );
    }

    #[test]
    fn plan_ids() {
        let t = UNIX_EPOCH + Duration::from_secs(20_739 * 86_400);
        let a = plan_id(t);
        let b = plan_id(t);
        assert!(a.starts_with("p-20261013-") && a.len() == 17, "{a}");
        assert_ne!(a, b);
    }
}
