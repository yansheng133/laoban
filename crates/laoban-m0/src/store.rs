//! In-memory plan store: keyed by plan ID, 1 hour TTL, at most 20 plans
//! (the oldest is evicted). Lost on restart, which M0 accepts for new apps;
//! a round keeps what its plan needs (`rounds::PlanRecipe`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::codegen::GeneratedFiles;
use crate::meta::HistoryEntry;
use crate::plan::AppPlan;

pub const PLAN_TTL: Duration = Duration::from_secs(60 * 60);
pub const MAX_PLANS: usize = 20;

/// What apply needs from the plan step. Holds only masked user text.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredPlan {
    pub id: String,
    pub app: String,
    pub plan: AppPlan,
    pub masked_prompt: String,
    pub model: String,
    /// Set when the plan changes an existing app.
    pub base: Option<BaseApp>,
    /// Set for a round's plan: the files when the round started, so hand
    /// edits made during the round can be told to the AI and checked.
    pub round_start: Option<GeneratedFiles>,
    /// Set for a round's plan: the opinions, as shown in the history
    /// (masked), instead of the request sent to the AI.
    pub label: Option<String>,
}

/// The existing app a plan changes, as read when the plan was made.
#[derive(Debug, Clone, PartialEq)]
pub struct BaseApp {
    pub files: GeneratedFiles,
    pub history: Vec<HistoryEntry>,
    /// Dragged 3D positions, carried over to the new version.
    pub layout: crate::meta::Layout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Pending,
    /// An apply is in flight; a concurrent apply gets `AlreadyApplied`.
    Applying,
    Applied,
}

#[derive(Debug)]
struct Entry {
    plan: StoredPlan,
    created: Instant,
    state: State,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    NotFound,
    AlreadyApplied,
}

#[derive(Debug)]
pub struct PlanStore {
    entries: HashMap<String, Entry>,
    ttl: Duration,
    max: usize,
}

impl Default for PlanStore {
    fn default() -> Self {
        Self::new(PLAN_TTL, MAX_PLANS)
    }
}

impl PlanStore {
    pub fn new(ttl: Duration, max: usize) -> Self {
        Self {
            entries: HashMap::new(),
            ttl,
            max: max.max(1),
        }
    }

    fn purge(&mut self, now: Instant) {
        let ttl = self.ttl;
        self.entries
            .retain(|_, e| now.saturating_duration_since(e.created) < ttl);
    }

    /// A copy of a plan (expired ones too, until purged).
    pub fn get(&self, id: &str) -> Option<StoredPlan> {
        self.entries.get(id).map(|e| e.plan.clone())
    }

    pub fn contains(&self, id: &str) -> bool {
        self.entries.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Insert, evicting expired plans and then the oldest ones.
    pub fn insert(&mut self, plan: StoredPlan, now: Instant) {
        self.purge(now);
        while self.entries.len() >= self.max {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(id, e)| (e.created, (*id).clone()))
                .map(|(id, _)| id.clone());
            match oldest {
                Some(id) => {
                    self.entries.remove(&id);
                }
                None => break,
            }
        }
        self.entries.insert(
            plan.id.clone(),
            Entry {
                plan,
                created: now,
                state: State::Pending,
            },
        );
    }

    /// Mark a plan as being applied and return it.
    pub fn begin_apply(&mut self, id: &str, now: Instant) -> Result<StoredPlan, StoreError> {
        self.purge(now);
        let entry = self.entries.get_mut(id).ok_or(StoreError::NotFound)?;
        match entry.state {
            State::Pending => {
                entry.state = State::Applying;
                Ok(entry.plan.clone())
            }
            State::Applying | State::Applied => Err(StoreError::AlreadyApplied),
        }
    }

    /// Finish an apply. A failed apply returns the plan to pending so the
    /// user can try again; a successful one can never be applied again.
    pub fn finish_apply(&mut self, id: &str, success: bool) {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.state = if success {
                State::Applied
            } else {
                State::Pending
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(id: &str) -> StoredPlan {
        StoredPlan {
            id: id.into(),
            app: "demo".into(),
            plan: AppPlan {
                app_name: None,
                summary: "s".into(),
                steps: vec!["x".into()],
                web_modules: vec![],
                api_modules: vec![],
                endpoints: vec![],
                notes: String::new(),
            },
            masked_prompt: String::new(),
            model: "m".into(),
            base: None,
            round_start: None,
            label: None,
        }
    }

    #[test]
    fn evicts_oldest_beyond_max() {
        let mut s = PlanStore::new(PLAN_TTL, 3);
        let t0 = Instant::now();
        for i in 0..4u64 {
            s.insert(plan(&format!("p{i}")), t0 + Duration::from_secs(i));
        }
        assert_eq!(s.len(), 3);
        assert!(!s.contains("p0"));
        assert!(s.contains("p3"));
    }

    #[test]
    fn expires_after_ttl() {
        let mut s = PlanStore::default();
        let t0 = Instant::now();
        s.insert(plan("p1"), t0);
        assert!(s.contains("p1"));
        assert_eq!(
            s.begin_apply("p1", t0 + PLAN_TTL),
            Err(StoreError::NotFound)
        );
    }

    #[test]
    fn apply_once() {
        let mut s = PlanStore::default();
        let t0 = Instant::now();
        s.insert(plan("p1"), t0);
        assert!(s.begin_apply("p1", t0).is_ok());
        // Concurrent apply while the first is in flight.
        assert_eq!(s.begin_apply("p1", t0), Err(StoreError::AlreadyApplied));
        s.finish_apply("p1", false);
        assert!(s.begin_apply("p1", t0).is_ok());
        s.finish_apply("p1", true);
        assert_eq!(s.begin_apply("p1", t0), Err(StoreError::AlreadyApplied));
        assert_eq!(s.begin_apply("nope", t0), Err(StoreError::NotFound));
    }
}
