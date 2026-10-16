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

/// `r-YYYYMMDD-sssssx`: a round of opinions. `sssss` is the second of the
/// (UTC) day in hex and `x` 4 random bits, so the IDs of one day sort by
/// time (the archive lists rounds by ID; report 21, B15). IDs this process
/// gives out only grow: one in the same second as the last takes the next
/// value (rounds are named after their ID, objects and archive files alike).
pub fn round_id(t: SystemTime) -> String {
    let mut h = RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    let day = secs(t).div_euclid(86_400);
    let drawn = (secs(t).rem_euclid(86_400) as u64) * 16 + (h.finish() & 0xf);
    let v = {
        let mut last = LAST_ROUND.lock().unwrap_or_else(|e| e.into_inner());
        let v = next_round_value(*last, day, drawn);
        *last = (day, v);
        v
    };
    format!("r-{}-{:05x}{:x}", ymd(t), v / 16, v % 16)
}

/// The value after `last` (day, second * 16 + x) for a drawn one: the drawn
/// value, or the next one when it is not above a recent last (the same
/// second, or a few before it). A clock set back further starts again.
fn next_round_value(last: (i64, u64), day: i64, drawn: u64) -> u64 {
    const RECENT: u64 = 60 * 16;
    let v = if last.0 == day && drawn <= last.1 && last.1 - drawn < RECENT {
        last.1 + 1
    } else {
        drawn
    };
    // Stays in the day (the last second of a day holds up to 16).
    v.min(86_400 * 16 - 1)
}

static LAST_ROUND: std::sync::Mutex<(i64, u64)> = std::sync::Mutex::new((i64::MIN, 0));

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

    #[test]
    fn round_ids_sort_by_time() {
        let day = UNIX_EPOCH + Duration::from_secs(20_739 * 86_400);
        let early = round_id(day + Duration::from_secs(9));
        let late = round_id(day + Duration::from_secs(86_399));
        assert!(early.starts_with("r-20261013-00009"), "{early}");
        assert!(late.starts_with("r-20261013-1517f"), "{late}");
        assert_eq!(early.len(), 17);
        assert!(late > early);
        assert!(crate::service::valid_round_id(&early));
    }

    #[test]
    fn round_ids_of_one_second_differ_and_grow() {
        let mut last = (5, 0);
        let mut seen = Vec::new();
        for drawn in [1600, 1603, 1601, 1600, 1615, 1602] {
            let v = next_round_value(last, 5, drawn);
            assert!(v > last.1 || seen.is_empty(), "{seen:?} {v}");
            seen.push(v);
            last = (5, v);
        }
        assert_eq!(seen, [1600, 1603, 1604, 1605, 1615, 1616]);
        // Another day, or a clock set back by more than a minute: as drawn.
        assert_eq!(next_round_value((5, 9000), 6, 10), 10);
        assert_eq!(next_round_value((5, 9000), 5, 10), 10);
    }
}
