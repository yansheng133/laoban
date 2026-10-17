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
//!
//! With a [`UsageStore`] the settled usage of each period is also saved, so
//! a restart does not reset it: the stored total is read before the first
//! reservation (and again after a failed read, at most every
//! [`LOAD_RETRY`]), and each settled call adds its tokens. Tokens that
//! could not be saved are kept and added with the next save.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

/// `YYYY-MM` of a [`UtcMonthClock`] period.
pub fn period_label(period: u64) -> String {
    format!("{:04}-{:02}", 1970 + period / 12, period % 12 + 1)
}

/// Where settled usage is saved between restarts. Errors are short
/// descriptions for the server log.
pub trait UsageStore: Send + Sync {
    /// Tokens saved for `period` (0 when nothing is saved).
    fn load(&self, period: u64) -> Result<u64, String>;
    /// Add `tokens` to what is saved for `period`.
    fn add(&self, period: u64, tokens: u64) -> Result<(), String>;
}

/// After a failed read of the saved usage, wait this long before trying
/// again (the budget meanwhile counts from what it has).
pub const LOAD_RETRY: Duration = Duration::from_secs(30);

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
    /// The saved usage of `period` has been read into `used` (always true
    /// without a store).
    loaded: bool,
    /// Settled tokens of `period` not saved yet.
    unsaved: u64,
    /// Do not read the saved usage again before this.
    next_load: Option<Instant>,
    /// A failed save or read was logged; the next success is logged too.
    failing: bool,
}

/// Thread-safe budget. Share it with `Arc`.
pub struct TokenBudget {
    cap: u64,
    clock: Box<dyn PeriodClock>,
    state: Mutex<State>,
    store: Mutex<Option<Arc<dyn UsageStore>>>,
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
            state: Mutex::new(State {
                period,
                used: 0,
                loaded: true,
                unsaved: 0,
                next_load: None,
                failing: false,
            }),
            store: Mutex::new(None),
        })
    }

    /// Save settled usage in `store` and read what it already holds (now,
    /// or before a later reservation if it cannot be read now).
    pub fn use_store(&self, store: Arc<dyn UsageStore>) {
        *self.store.lock().unwrap() = Some(store);
        let mut state = self.state.lock().unwrap();
        state.loaded = false;
        state.next_load = None;
        self.roll(&mut state);
        self.ensure_loaded(&mut state);
    }

    fn current_store(&self) -> Option<Arc<dyn UsageStore>> {
        self.store.lock().unwrap().clone()
    }

    /// Read the saved usage of the current period if not read yet. Called
    /// with the state locked: the read is one small request, and nothing
    /// may be reserved before it is counted.
    fn ensure_loaded(&self, state: &mut State) {
        if state.loaded {
            return;
        }
        let Some(store) = self.current_store() else {
            state.loaded = true;
            return;
        };
        let now = Instant::now();
        if state.next_load.is_some_and(|t| now < t) {
            return;
        }
        match store.load(state.period) {
            Ok(saved) => {
                state.used = state.used.saturating_add(saved);
                state.loaded = true;
                state.next_load = None;
                if state.failing {
                    state.failing = false;
                    eprintln!("laoban-ai: token usage is read from storage again");
                }
                if state.unsaved > 0 {
                    match store.add(state.period, state.unsaved) {
                        Ok(()) => state.unsaved = 0,
                        Err(e) => eprintln!("laoban-ai: saving token usage failed: {e}"),
                    }
                }
            }
            Err(e) => {
                state.next_load = Some(now + LOAD_RETRY);
                if !state.failing {
                    state.failing = true;
                    eprintln!(
                        "laoban-ai: reading the saved token usage failed ({e}); \
                         counting from what this process has used, trying again later"
                    );
                }
            }
        }
    }

    pub fn cap(&self) -> u64 {
        self.cap
    }

    /// Tokens used or reserved in the current period.
    pub fn used(&self) -> u64 {
        let mut state = self.state.lock().unwrap();
        self.roll(&mut state);
        self.ensure_loaded(&mut state);
        state.used
    }

    fn roll(&self, state: &mut State) {
        let now = self.clock.current_period();
        if now != state.period {
            // Save what the old period still owes (best effort).
            if state.loaded && state.unsaved > 0 {
                if let Some(store) = self.current_store() {
                    if let Err(e) = store.add(state.period, state.unsaved) {
                        eprintln!("laoban-ai: saving token usage failed: {e}");
                    }
                }
            }
            state.period = now;
            state.used = 0;
            state.unsaved = 0;
            state.loaded = self.current_store().is_none();
            state.next_load = None;
        }
    }

    /// Reserve `amount` tokens, or refuse if that would exceed the cap.
    pub fn reserve(self: &Arc<Self>, amount: u64) -> Result<Reservation, BudgetError> {
        let mut state = self.state.lock().unwrap();
        self.roll(&mut state);
        self.ensure_loaded(&mut state);
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
        if actual == 0 {
            return;
        }
        state.unsaved = state.unsaved.saturating_add(actual);
        if !state.loaded {
            // Added once the saved total is read, so it is not counted twice.
            return;
        }
        let Some(store) = self.current_store() else {
            state.unsaved = 0;
            return;
        };
        let tokens = std::mem::take(&mut state.unsaved);
        drop(state);
        let result = store.add(period, tokens);
        let mut state = self.state.lock().unwrap();
        match result {
            Ok(()) => {
                if state.failing && state.period == period {
                    state.failing = false;
                    eprintln!("laoban-ai: token usage is saved again");
                }
            }
            Err(e) => {
                if state.period == period {
                    state.unsaved = state.unsaved.saturating_add(tokens);
                    if !state.failing {
                        state.failing = true;
                        eprintln!(
                            "laoban-ai: saving token usage failed ({e}); \
                             kept in memory and saved with the next call"
                        );
                    }
                }
            }
        }
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

    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[derive(Default)]
    struct FakeStore {
        saved: Mutex<HashMap<u64, u64>>,
        fail_loads: AtomicU64,
        fail_adds: AtomicU64,
    }

    impl UsageStore for FakeStore {
        fn load(&self, period: u64) -> Result<u64, String> {
            if self.fail_loads.load(Ordering::SeqCst) > 0 {
                self.fail_loads.fetch_sub(1, Ordering::SeqCst);
                return Err("down".into());
            }
            Ok(*self.saved.lock().unwrap().get(&period).unwrap_or(&0))
        }
        fn add(&self, period: u64, tokens: u64) -> Result<(), String> {
            if self.fail_adds.load(Ordering::SeqCst) > 0 {
                self.fail_adds.fetch_sub(1, Ordering::SeqCst);
                return Err("down".into());
            }
            *self.saved.lock().unwrap().entry(period).or_default() += tokens;
            Ok(())
        }
    }

    struct SharedPeriod(Arc<AtomicU64>);
    impl PeriodClock for SharedPeriod {
        fn current_period(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn period_labels() {
        assert_eq!(period_label(0), "1970-01");
        assert_eq!(period_label((2026 - 1970) * 12 + 9), "2026-10");
        assert_eq!(period_label((2026 - 1970) * 12 + 11), "2026-12");
    }

    #[test]
    fn usage_survives_a_restart() {
        let store = Arc::new(FakeStore::default());
        let b = TokenBudget::new(1_000, FixedPeriod(7));
        b.use_store(store.clone());
        b.reserve(300).unwrap().settle(120);
        b.reserve(300).unwrap().settle(80);
        assert_eq!(b.used(), 200);
        assert_eq!(store.saved.lock().unwrap()[&7], 200);

        // A new process with the same store.
        let b2 = TokenBudget::new(1_000, FixedPeriod(7));
        b2.use_store(store.clone());
        assert_eq!(b2.used(), 200);
        assert!(b2.reserve(801).is_err(), "the cap counts the saved usage");
        b2.reserve(800).unwrap().settle(10);
        assert_eq!(store.saved.lock().unwrap()[&7], 210);
    }

    #[test]
    fn refunded_reservations_are_not_saved() {
        let store = Arc::new(FakeStore::default());
        let b = TokenBudget::new(1_000, FixedPeriod(1));
        b.use_store(store.clone());
        drop(b.reserve(500).unwrap());
        assert_eq!(b.used(), 0);
        assert!(store.saved.lock().unwrap().get(&1).is_none());
    }

    #[test]
    fn failed_read_counts_nothing_twice() {
        let store = Arc::new(FakeStore::default());
        store.saved.lock().unwrap().insert(3, 400);
        store.fail_loads.store(1, Ordering::SeqCst);
        let b = TokenBudget::new(1_000, FixedPeriod(3));
        b.use_store(store.clone());
        // Not read yet: counts what this process uses, saves nothing yet.
        b.reserve(100).unwrap().settle(50);
        assert_eq!(b.used(), 50);
        assert_eq!(store.saved.lock().unwrap()[&3], 400);
        // The next read (after the wait) adds the saved total once and
        // saves what was kept.
        b.state.lock().unwrap().next_load = None;
        assert_eq!(b.used(), 450);
        assert_eq!(store.saved.lock().unwrap()[&3], 450);
        b.reserve(10).unwrap().settle(10);
        assert_eq!(b.used(), 460);
        assert_eq!(store.saved.lock().unwrap()[&3], 460);
    }

    #[test]
    fn failed_save_is_saved_with_the_next_call() {
        let store = Arc::new(FakeStore::default());
        let b = TokenBudget::new(1_000, FixedPeriod(2));
        b.use_store(store.clone());
        store.fail_adds.store(1, Ordering::SeqCst);
        b.reserve(100).unwrap().settle(30);
        assert!(store.saved.lock().unwrap().get(&2).is_none());
        b.reserve(100).unwrap().settle(20);
        assert_eq!(store.saved.lock().unwrap()[&2], 50);
        assert_eq!(b.used(), 50);
    }

    #[test]
    fn a_new_month_reads_its_own_total() {
        let store = Arc::new(FakeStore::default());
        store.saved.lock().unwrap().insert(11, 999);
        let p = Arc::new(AtomicU64::new(10));
        let b = TokenBudget::new(1_000, SharedPeriod(p.clone()));
        b.use_store(store.clone());
        b.reserve(100).unwrap().settle(100);
        p.store(11, Ordering::SeqCst);
        assert_eq!(b.used(), 999);
        assert!(b.reserve(2).is_err());
        assert_eq!(store.saved.lock().unwrap()[&10], 100);
    }
}
