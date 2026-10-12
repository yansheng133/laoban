//! Global token budget shared by every Project.
//!
//! Before a call the orchestrator reserves `estimated_input + max_tokens`.
//! A reservation that would push the period total over the cap is refused
//! before the provider is called. After the call the reservation is settled
//! to actual usage; a reservation that is dropped without settling (provider
//! error, panic) is refunded.
//!
//! The cap bounds what can be *reserved*. Actual usage can exceed a
//! reservation if the input estimate was low; that overrun is still
//! recorded (it was spent), so later reservations see it.

use std::sync::{Arc, Mutex};

use thiserror::Error;

/// Identifies the current budget period (for example, months since an
/// epoch). Injected so tests control period changes.
pub trait PeriodClock: Send + Sync {
    fn current_period(&self) -> u64;
}

/// A clock that never changes period. Useful for tests and for deployments
/// that reset the budget externally.
#[derive(Debug, Default, Clone, Copy)]
pub struct FixedPeriod(pub u64);

impl PeriodClock for FixedPeriod {
    fn current_period(&self) -> u64 {
        self.0
    }
}

/// Calendar month in UTC, counted from 1970-01, from the system clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct UtcMonthClock;

impl PeriodClock for UtcMonthClock {
    fn current_period(&self) -> u64 {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let (y, m) = civil_year_month(secs / 86_400);
        (y.saturating_sub(1970)) * 12 + (m - 1)
    }
}

/// Days since 1970-01-01 to (year, month). Howard Hinnant's algorithm.
fn civil_year_month(days: u64) -> (u64, u64) {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y.max(0) as u64, m as u64)
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BudgetError {
    #[error("token budget exceeded: requested {requested}, used {used} of {cap} in this period")]
    Exhausted { requested: u64, used: u64, cap: u64 },
}

#[derive(Debug)]
struct State {
    period: u64,
    /// Settled usage plus outstanding reservations.
    used: u64,
}

/// Thread-safe budget. Share it with `Arc`.
pub struct TokenBudget {
    cap: u64,
    clock: Box<dyn PeriodClock>,
    state: Mutex<State>,
}

impl std::fmt::Debug for TokenBudget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenBudget")
            .field("cap", &self.cap)
            .field("state", &self.state)
            .finish()
    }
}

impl TokenBudget {
    pub fn new(cap_per_period: u64, clock: impl PeriodClock + 'static) -> Arc<Self> {
        let period = clock.current_period();
        Arc::new(Self {
            cap: cap_per_period,
            clock: Box::new(clock),
            state: Mutex::new(State { period, used: 0 }),
        })
    }

    pub fn cap(&self) -> u64 {
        self.cap
    }

    /// Tokens used or reserved in the current period.
    pub fn used(&self) -> u64 {
        let mut state = self.state.lock().unwrap();
        self.roll(&mut state);
        state.used
    }

    fn roll(&self, state: &mut State) {
        let now = self.clock.current_period();
        if now != state.period {
            state.period = now;
            state.used = 0;
        }
    }

    /// Reserve `amount` tokens, or refuse if that would exceed the cap.
    pub fn reserve(self: &Arc<Self>, amount: u64) -> Result<Reservation, BudgetError> {
        let mut state = self.state.lock().unwrap();
        self.roll(&mut state);
        match state.used.checked_add(amount) {
            Some(total) if total <= self.cap => {
                state.used = total;
                Ok(Reservation {
                    budget: Arc::clone(self),
                    period: state.period,
                    amount,
                    settled: false,
                })
            }
            _ => Err(BudgetError::Exhausted {
                requested: amount,
                used: state.used,
                cap: self.cap,
            }),
        }
    }

    fn release(&self, period: u64, reserved: u64, actual: u64) {
        let mut state = self.state.lock().unwrap();
        self.roll(&mut state);
        if state.period != period {
            // The period rolled over while the call was in flight; the old
            // period's total was already discarded.
            return;
        }
        state.used = state.used.saturating_sub(reserved).saturating_add(actual);
    }
}

/// An outstanding reservation. Settle it with actual usage; dropping it
/// unsettled refunds the full amount.
#[derive(Debug)]
#[must_use = "a reservation is refunded when dropped; settle it after the call"]
pub struct Reservation {
    budget: Arc<TokenBudget>,
    period: u64,
    amount: u64,
    settled: bool,
}

impl Reservation {
    pub fn amount(&self) -> u64 {
        self.amount
    }

    /// Replace the reservation with the tokens actually used.
    pub fn settle(mut self, actual: u64) {
        self.settled = true;
        self.budget.release(self.period, self.amount, actual);
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.settled {
            self.budget.release(self.period, self.amount, 0);
        }
    }
}

/// Rough upper-leaning token estimate: one token per three UTF-8 bytes
/// (CJK characters are three bytes and usually about one token; English
/// averages about four characters per token), plus a fixed overhead.
/// Replace with the provider's token counting endpoint when the HTTP client
/// lands.
pub fn estimate_tokens(text: &str) -> u64 {
    (text.len() as u64).div_ceil(3) + 8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_index_known_dates() {
        // 1970-01-01
        assert_eq!(civil_year_month(0), (1970, 1));
        // 2026-10-12 is day 20_738
        assert_eq!(civil_year_month(20_738), (2026, 10));
        // 2024-02-29
        assert_eq!(civil_year_month(19_782), (2024, 2));
    }
}
