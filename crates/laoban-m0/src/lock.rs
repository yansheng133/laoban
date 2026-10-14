//! Per-app edit lock: only one person changes an app at a time (phase-1
//! "同一 Project 編輯鎖"; M0 has no Projects, so the unit is the app).
//!
//! In memory, like the plan store: one backend replica, lost on restart
//! (every lock then simply expires). A lock lasts [`LOCK_TTL`] unless the
//! holder renews it; the page renews while it is open.
//!
//! The holder is the caller's user name as verified by the Kubernetes API
//! server (the backend is an aggregated API, see docs/m0-design.md), so a
//! lock cannot be taken or used under someone else's name. The same user
//! in two tabs shares the lock.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

/// How long a lock lasts without renewal.
pub const LOCK_TTL: Duration = Duration::from_secs(180);
/// Longest holder name kept.
pub const MAX_HOLDER_CHARS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Lock {
    holder: String,
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
    pub holder: String,
    pub ttl: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockError {
    /// Someone else holds it.
    Held(LockInfo),
    /// The caller does not hold it (never taken, or expired).
    NotHeld,
}

#[derive(Debug, Default)]
pub struct LockTable {
    locks: HashMap<String, Lock>,
}

/// User names are display text too: one line, bounded.
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

    /// Take the lock, or extend it when the caller already holds it.
    pub fn acquire(&mut self, app: &str, user: &str, now: Instant) -> Result<LockGrant, LockError> {
        let holder = clean_holder(user);
        let since = match self.live(app, now) {
            Some(l) if l.holder != holder => {
                return Err(LockError::Held(LockInfo {
                    holder: l.holder.clone(),
                    since: l.since,
                }))
            }
            Some(l) => l.since,
            None => SystemTime::now(),
        };
        self.locks.insert(
            app.to_string(),
            Lock {
                holder: holder.clone(),
                since,
                expires: now + LOCK_TTL,
            },
        );
        Ok(LockGrant {
            holder,
            ttl: LOCK_TTL,
        })
    }

    /// Extend the lock `user` holds.
    pub fn renew(&mut self, app: &str, user: &str, now: Instant) -> Result<(), LockError> {
        self.check(app, Some(user), now)?;
        if let Some(l) = self.locks.get_mut(app) {
            l.expires = now + LOCK_TTL;
        }
        Ok(())
    }

    /// Give the lock back. Releasing a lock one does not hold is a no-op.
    pub fn release(&mut self, app: &str, user: &str) {
        let holder = clean_holder(user);
        if self.locks.get(app).is_some_and(|l| l.holder == holder) {
            self.locks.remove(app);
        }
    }

    /// Does `user` hold the live lock on `app`?
    pub fn check(&mut self, app: &str, user: Option<&str>, now: Instant) -> Result<(), LockError> {
        let user = user.map(clean_holder);
        match (self.live(app, now), user) {
            (Some(l), Some(u)) if l.holder == u => Ok(()),
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
        t.acquire("todo", "u-alice", now).unwrap();
        match t.acquire("todo", "u-bob", now) {
            Err(LockError::Held(info)) => assert_eq!(info.holder, "u-alice"),
            other => panic!("{other:?}"),
        }
        // Other apps are independent.
        assert!(t.acquire("shop", "u-bob", now).is_ok());
        assert!(t.check("todo", Some("u-alice"), now).is_ok());
        assert!(matches!(
            t.check("todo", None, now),
            Err(LockError::Held(_))
        ));
        assert!(matches!(
            t.check("todo", Some("u-bob"), now),
            Err(LockError::Held(_))
        ));
        assert_eq!(
            t.check("nolock", Some("u-alice"), now),
            Err(LockError::NotHeld)
        );
    }

    #[test]
    fn same_user_extends_and_keeps_since() {
        let mut t = LockTable::default();
        let now = Instant::now();
        t.acquire("todo", "u-alice", now).unwrap();
        let since = t.status("todo", now).unwrap().since;
        let later = now + LOCK_TTL - Duration::from_secs(1);
        t.acquire("todo", "u-alice", later).unwrap();
        assert_eq!(t.status("todo", later).unwrap().since, since);
        assert!(t
            .check(
                "todo",
                Some("u-alice"),
                now + LOCK_TTL + Duration::from_secs(1)
            )
            .is_ok());
    }

    #[test]
    fn expires_unless_renewed_and_release() {
        let mut t = LockTable::default();
        let now = Instant::now();
        t.acquire("todo", "u-alice", now).unwrap();
        let later = now + LOCK_TTL - Duration::from_secs(1);
        t.renew("todo", "u-alice", later).unwrap();
        assert!(t
            .check(
                "todo",
                Some("u-alice"),
                now + LOCK_TTL + Duration::from_secs(1)
            )
            .is_ok());
        let much_later = later + LOCK_TTL + Duration::from_secs(1);
        assert_eq!(t.status("todo", much_later), None);
        assert!(t.acquire("todo", "u-bob", much_later).is_ok());
        assert!(t.renew("todo", "u-alice", much_later).is_err());

        t.acquire("shop", "u-bob", now).unwrap();
        t.release("shop", "u-alice");
        assert!(t.status("shop", now).is_some());
        t.release("shop", "u-bob");
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
