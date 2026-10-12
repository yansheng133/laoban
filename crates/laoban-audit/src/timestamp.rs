//! RFC 5424 timestamps without a date/time dependency.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;

/// Largest supported instant: 9999-12-31T23:59:59Z. RFC 5424 `DATE-FULLYEAR`
/// is exactly four digits.
const MAX_UNIX_SECS: u64 = 253_402_300_799;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TimestampError {
    #[error("timestamp is before 1970-01-01T00:00:00Z")]
    BeforeEpoch,
    #[error("timestamp is after 9999-12-31T23:59:59Z")]
    AfterYear9999,
    #[error("microseconds must be below 1000000, got {0}")]
    InvalidMicros(u32),
}

/// A UTC instant with microsecond precision.
///
/// Formats as RFC 3339 / RFC 5424 section 6.2.3 `TIMESTAMP` with exactly six
/// fractional digits (the RFC 5424 maximum, `TIME-SECFRAC = "." 1*6DIGIT`)
/// and the `Z` offset, e.g. `2026-10-12T03:04:05.000120Z`.
///
/// The formatter never reads the clock; the caller supplies the instant, so
/// output is deterministic and testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AuditTimestamp {
    secs: u64,
    micros: u32,
}

impl AuditTimestamp {
    /// Build from seconds and microseconds since the Unix epoch (UTC).
    pub fn from_unix(secs: u64, micros: u32) -> Result<Self, TimestampError> {
        if micros >= 1_000_000 {
            return Err(TimestampError::InvalidMicros(micros));
        }
        if secs > MAX_UNIX_SECS {
            return Err(TimestampError::AfterYear9999);
        }
        Ok(Self { secs, micros })
    }

    /// Build from a [`SystemTime`], truncating to microseconds.
    pub fn from_system_time(t: SystemTime) -> Result<Self, TimestampError> {
        let d = t
            .duration_since(UNIX_EPOCH)
            .map_err(|_| TimestampError::BeforeEpoch)?;
        Self::from_unix(d.as_secs(), d.subsec_micros())
    }

    pub fn unix_secs(&self) -> u64 {
        self.secs
    }

    pub fn subsec_micros(&self) -> u32 {
        self.micros
    }
}

impl fmt::Display for AuditTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let days = (self.secs / 86_400) as i64;
        let rem = self.secs % 86_400;
        let (y, m, d) = civil_from_days(days);
        write!(
            f,
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:06}Z",
            rem / 3600,
            (rem % 3600) / 60,
            rem % 60,
            self.micros
        )
    }
}

/// Days since 1970-01-01 to (year, month, day) in the proleptic Gregorian
/// calendar. Howard Hinnant's `civil_from_days` algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
