//! Per-app edit lock: only one person changes an app at a time (phase-1
//! "同一 Project 編輯鎖"; M0 has no Projects, so the unit is the app).
//!
//! In memory, like the plan store: one backend replica, lost on restart
//! (every lock then simply expires). A lock lasts [`LOCK_TTL`] unless the
//! holder renews it; the page renews while it is open.
//!
//! The holder name comes from the page (the Rancher user it is logged in
//! as). M0's backend does not know the caller's identity, so the name is
//! for display only and the lock is cooperative; the token is what
//! authorises a change. Phase 1 replaces the name with the Rancher
//! principal.

use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::time::{Duration, Instant, SystemTime};

/// How long a lock lasts without renewal.
pub const LOCK_TTL: Duration = Duration::from_secs(180);
/// Longest holder name kept.
pub const MAX_HOLDER_CHARS: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Lock {
    holder: String,
    token: String,
    since: SystemTime,
    expires: Instant,
}

/// Who holds a lock, for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockInfo {
    pub holder: String,
    pub since: SystemTime,
}

/// A granted lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockGrant {
    pub token: String,
    pub holder: String,
    pub ttl: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockError {
    /// Someone else holds it.
    Held(LockInfo),
    /// No (valid) token was given for an app that must be locked first,
    /// or the lock expired.
    NotHeld,
}

#[derive(Debug, Default)]
pub struct LockTable {
    locks: HashMap<String, Lock>,
}

fn new_token() -> String {
    let part = || {
        let mut h = RandomState::new().build_hasher();
        h.write_u128(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        h.finish()
    };
    format!("{:016x}{:016x}", part(), part())
}

/// Holder names are display text: one line, bounded.
pub fn clean_holder(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_HOLDER_CHARS)
        .collect();
    let s = s.trim().to_string();
    if s.is_empty() {
        "?".into()
    } else {
        s
    }
}

impl LockTable {
    fn live(&mut self, app: &str, now: Instant) -> Option<&Lock> {
        if self.locks.get(app).is_some_and(|l| l.expires <= now) {
            self.locks.remove(app);
        }
        self.locks.get(app)
    }

    /// Take the lock. A lock held under the same holder name is taken
    /// over (the same person reloading the page or opening a second tab);
    /// the old token stops working.
    pub fn acquire(
        &mut self,
        app: &str,
        holder: &str,
        now: Instant,
    ) -> Result<LockGrant, LockError> {
        let holder = clean_holder(holder);
        if let Some(l) = self.live(app, now) {
            if l.holder != holder {
                return Err(LockError::Held(LockInfo {
                    holder: l.holder.clone(),
                    since: l.since,
                }));
            }
        }
        let token = new_token();
        self.locks.insert(
            app.to_string(),
            Lock {
                holder: holder.clone(),
                token: token.clone(),
                since: SystemTime::now(),
                expires: now + LOCK_TTL,
            },
        );
        Ok(LockGrant {
            token,
            holder,
            ttl: LOCK_TTL,
        })
    }

    /// Extend the lock held with `token`.
    pub fn renew(&mut self, app: &str, token: &str, now: Instant) -> Result<(), LockError> {
        self.check(app, Some(token), now)?;
        if let Some(l) = self.locks.get_mut(app) {
            l.expires = now + LOCK_TTL;
        }
        Ok(())
    }

    /// Give the lock back. Releasing a lock one does not hold is a no-op.
    pub fn release(&mut self, app: &str, token: &str) {
        if self.locks.get(app).is_some_and(|l| l.token == token) {
            self.locks.remove(app);
        }
    }

    /// Is `token` the live lock on `app`?
    pub fn check(&mut self, app: &str, token: Option<&str>, now: Instant) -> Result<(), LockError> {
        match (self.live(app, now), token) {
            (Some(l), Some(t)) if l.token == t => Ok(()),
            (Some(l), _) => Err(LockError::Held(LockInfo {
                holder: l.holder.clone(),
                since: l.since,
            })),
            (None, _) => Err(LockError::NotHeld),
        }
    }

    /// Who holds the lock on `app`, if anyone.
    pub fn status(&mut self, app: &str, now: Instant) -> Option<LockInfo> {
        self.live(app, now).map(|l| LockInfo {
            holder: l.holder.clone(),
            since: l.since,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_holder_at_a_time() {
        let mut t = LockTable::default();
        let now = Instant::now();
        let a = t.acquire("todo", "alice", now).unwrap();
        assert_eq!(a.token.len(), 32);
        match t.acquire("todo", "bob", now) {
            Err(LockError::Held(info)) => assert_eq!(info.holder, "alice"),
            other => panic!("{other:?}"),
        }
        // Other apps are independent.
        assert!(t.acquire("shop", "bob", now).is_ok());
        assert!(t.check("todo", Some(&a.token), now).is_ok());
        assert!(matches!(
            t.check("todo", None, now),
            Err(LockError::Held(_))
        ));
        assert!(matches!(
            t.check("todo", Some("x"), now),
            Err(LockError::Held(_))
        ));
        assert_eq!(
            t.check("nolock", Some(&a.token), now),
            Err(LockError::NotHeld)
        );
    }

    #[test]
    fn same_holder_takes_over_and_old_token_stops_working() {
        let mut t = LockTable::default();
        let now = Instant::now();
        let first = t.acquire("todo", "alice", now).unwrap();
        let second = t.acquire("todo", "alice", now).unwrap();
        assert_ne!(first.token, second.token);
        assert!(t.check("todo", Some(&first.token), now).is_err());
        assert!(t.check("todo", Some(&second.token), now).is_ok());
    }

    #[test]
    fn expires_unless_renewed_and_release() {
        let mut t = LockTable::default();
        let now = Instant::now();
        let g = t.acquire("todo", "alice", now).unwrap();
        let later = now + LOCK_TTL - Duration::from_secs(1);
        t.renew("todo", &g.token, later).unwrap();
        // Renewed: still held after the original expiry.
        assert!(t
            .check(
                "todo",
                Some(&g.token),
                now + LOCK_TTL + Duration::from_secs(1)
            )
            .is_ok());
        // Expired: anyone can take it, the old token is gone.
        let much_later = later + LOCK_TTL + Duration::from_secs(1);
        assert_eq!(t.status("todo", much_later), None);
        assert!(t.acquire("todo", "bob", much_later).is_ok());
        assert!(t.renew("todo", &g.token, much_later).is_err());

        let b = t.acquire("shop", "bob", now).unwrap();
        t.release("shop", "wrong");
        assert!(t.status("shop", now).is_some());
        t.release("shop", &b.token);
        assert!(t.status("shop", now).is_none());
    }

    #[test]
    fn holder_names_are_cleaned() {
        assert_eq!(clean_holder("  sam\n"), "sam");
        assert_eq!(clean_holder(""), "?");
        assert_eq!(
            clean_holder(&"x".repeat(500)).chars().count(),
            MAX_HOLDER_CHARS
        );
    }
}
