//! Retry policy. The delay is a pure function so tests never sleep.

use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RetryPolicy {
    /// Retries after the first attempt. 0 disables retrying.
    pub max_retries: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            base_delay_ms: 1_000,
            max_delay_ms: 30_000,
        }
    }
}

/// Delay before retry number `retry` (0-based). A server-supplied
/// `retry-after` wins, because retrying earlier is documented to fail.
/// Otherwise exponential: `base * 2^retry`, capped at `max_delay_ms`.
pub fn backoff_delay(policy: &RetryPolicy, retry: u32, retry_after: Option<Duration>) -> Duration {
    if let Some(d) = retry_after {
        return d;
    }
    let factor = 1u64.checked_shl(retry.min(63)).unwrap_or(u64::MAX);
    let ms = policy
        .base_delay_ms
        .saturating_mul(factor)
        .min(policy.max_delay_ms);
    Duration::from_millis(ms)
}

/// Injected so tests can record delays instead of sleeping.
pub trait Sleeper: Send + Sync {
    fn sleep(&self, d: Duration);
}

/// Real sleeper for production use.
#[derive(Debug, Default, Clone, Copy)]
pub struct ThreadSleeper;

impl Sleeper for ThreadSleeper {
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// Records requested delays and returns immediately.
#[derive(Debug, Default)]
pub struct RecordingSleeper {
    delays: Mutex<Vec<Duration>>,
}

impl RecordingSleeper {
    pub fn delays(&self) -> Vec<Duration> {
        self.delays.lock().unwrap().clone()
    }
}

impl Sleeper for RecordingSleeper {
    fn sleep(&self, d: Duration) {
        self.delays.lock().unwrap().push(d);
    }
}

impl<S: Sleeper + ?Sized> Sleeper for std::sync::Arc<S> {
    fn sleep(&self, d: Duration) {
        (**self).sleep(d)
    }
}
