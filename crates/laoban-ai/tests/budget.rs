use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use laoban_ai::{BudgetError, FixedPeriod, PeriodClock, TokenBudget};

#[test]
fn reserve_settle_and_refund() {
    let b = TokenBudget::new(1_000, FixedPeriod(0));
    let r = b.reserve(600).unwrap();
    assert_eq!(b.used(), 600);
    r.settle(250);
    assert_eq!(b.used(), 250);

    let r = b.reserve(700).unwrap();
    assert_eq!(b.used(), 950);
    drop(r); // provider error path: full refund
    assert_eq!(b.used(), 250);
}

#[test]
fn over_cap_is_refused_with_typed_error() {
    let b = TokenBudget::new(1_000, FixedPeriod(0));
    let _r = b.reserve(900).unwrap();
    assert_eq!(
        b.reserve(101).unwrap_err(),
        BudgetError::Exhausted {
            requested: 101,
            used: 900,
            cap: 1_000
        }
    );
    // Exactly reaching the cap is allowed.
    let _r2 = b.reserve(100).unwrap();
    assert_eq!(b.used(), 1_000);
}

#[test]
fn overflowing_request_is_refused() {
    let b = TokenBudget::new(u64::MAX, FixedPeriod(0));
    let _r = b.reserve(10).unwrap();
    assert!(b.reserve(u64::MAX).is_err());
}

#[test]
fn actual_overrun_is_recorded() {
    let b = TokenBudget::new(1_000, FixedPeriod(0));
    b.reserve(100).unwrap().settle(300);
    assert_eq!(b.used(), 300);
}

struct ManualClock(Arc<AtomicU64>);
impl PeriodClock for ManualClock {
    fn current_period(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[test]
fn new_period_resets_usage() {
    let period = Arc::new(AtomicU64::new(5));
    let b = TokenBudget::new(1_000, ManualClock(Arc::clone(&period)));
    b.reserve(1_000).unwrap().settle(1_000);
    assert!(b.reserve(1).is_err());

    let in_flight = b.reserve(0).unwrap();
    period.store(6, Ordering::SeqCst);
    assert_eq!(b.used(), 0);
    // Settling a reservation from the old period does not charge the new one.
    in_flight.settle(500);
    assert_eq!(b.used(), 0);
    assert!(b.reserve(1_000).is_ok());
}

#[test]
fn concurrent_reservations_never_exceed_cap() {
    const CAP: u64 = 10_000;
    const EACH: u64 = 300;
    const THREADS: usize = 16;
    const PER_THREAD: usize = 20;

    let b = TokenBudget::new(CAP, FixedPeriod(0));
    let barrier = Arc::new(Barrier::new(THREADS));
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let b = Arc::clone(&b);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                let mut held = Vec::new();
                for _ in 0..PER_THREAD {
                    if let Ok(r) = b.reserve(EACH) {
                        assert!(b.used() <= CAP);
                        held.push(r);
                    }
                }
                held
            })
        })
        .collect();
    let held: Vec<_> = handles
        .into_iter()
        .flat_map(|h| h.join().unwrap())
        .collect();
    assert_eq!(held.len() as u64, CAP / EACH);
    assert_eq!(b.used(), (CAP / EACH) * EACH);
    assert!(b.used() <= CAP);
    drop(held);
    assert_eq!(b.used(), 0);
}
