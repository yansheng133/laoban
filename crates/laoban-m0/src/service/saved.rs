//! Keeping token usage and plans across backend restarts (v0.1.19).
//!
//! * Token usage: one `TokenUsage` resource per month (`tokens-YYYY-MM`)
//!   holds the settled tokens of that month; [`StateUsage`] is the budget's
//!   [`UsageStore`].
//! * Plans: a plan waiting to be confirmed outside a round is written to a
//!   `Plan` resource (`plan-<id>`) and read back at startup, so a restart
//!   no longer loses it. A round's plan is not written: the round keeps
//!   what it needs and makes it again (`ensure_round_plan`).
//!   The resource is deleted when the apply starts and written again if
//!   the apply fails, so a plan whose apply was cut off by a restart is
//!   never applied twice (the user makes a new plan). Expired resources
//!   are deleted at startup and then every [`SWEEP_EVERY`] seconds.
//!
//! Failures to write are logged and leave the plan in memory only, as
//! before; they never fail the request.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use laoban_ai::{period_label, UsageStore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::persist::now_unix;
use super::M0Service;
use crate::codegen::parse_generated;
use crate::meta::{HistoryEntry, Layout};
use crate::plan::AppPlan;
use crate::state::{StateError, StateStore, APP_LABEL, PLANS, TOKEN_USAGES};
use crate::store::{BaseApp, StoredPlan, PLAN_TTL};

/// Shape of a stored `Plan` / `TokenUsage` spec.
const SCHEMA: u64 = 1;
/// Delete expired `Plan` resources this often (seconds).
pub(super) const SWEEP_EVERY: u64 = 600;
/// Attempts of a read-modify-write of the month's usage.
const USAGE_ATTEMPTS: usize = 5;

fn usage_name(period: u64) -> String {
    format!("tokens-{}", period_label(period))
}

/// The budget's usage, in `TokenUsage` resources.
pub struct StateUsage {
    state: Arc<dyn StateStore>,
}

impl StateUsage {
    pub fn new(state: Arc<dyn StateStore>) -> Self {
        Self { state }
    }
}

fn tokens_of(spec: &Value) -> u64 {
    spec["tokens"].as_u64().unwrap_or(0)
}

impl UsageStore for StateUsage {
    fn load(&self, period: u64) -> Result<u64, String> {
        match self.state.get(TOKEN_USAGES, &usage_name(period)) {
            Ok(Some(o)) => Ok(tokens_of(&o.spec)),
            Ok(None) => Ok(0),
            Err(e) => Err(e.to_string()),
        }
    }

    fn add(&self, period: u64, tokens: u64) -> Result<(), String> {
        let name = usage_name(period);
        let spec = |total: u64| json!({ "schema": SCHEMA, "period": period_label(period), "tokens": total });
        let mut last = StateError::Conflict;
        for _ in 0..USAGE_ATTEMPTS {
            let result = match self.state.get(TOKEN_USAGES, &name) {
                Ok(Some(o)) => self
                    .state
                    .update(
                        TOKEN_USAGES,
                        &name,
                        &o.version,
                        &spec(tokens_of(&o.spec).saturating_add(tokens)),
                    )
                    .map(|_| ()),
                Ok(None) => self
                    .state
                    .create(TOKEN_USAGES, &name, &BTreeMap::new(), &spec(tokens))
                    .map(|_| ()),
                Err(e) => Err(e),
            };
            match result {
                Ok(()) => return Ok(()),
                // Someone else wrote it in between: read it again.
                Err(e @ (StateError::Conflict | StateError::Exists)) => last = e,
                Err(e) => return Err(e.to_string()),
            }
        }
        Err(last.to_string())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BaseSpec {
    /// The four files, in the shape [`parse_generated`] reads.
    files: Value,
    history: Vec<HistoryEntry>,
    #[serde(default)]
    layout: Layout,
}

/// `spec.plan` of a `Plan` resource. Holds only masked user text.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanBody {
    plan: AppPlan,
    masked_prompt: String,
    model: String,
    #[serde(default)]
    base: Option<BaseSpec>,
}

fn object_name(id: &str) -> String {
    format!("plan-{id}")
}

/// Plans that can be written: not a round's (those are made again from
/// the round).
fn kept(p: &StoredPlan) -> bool {
    p.label.is_none() && p.round_start.is_none()
}

fn to_spec(p: &StoredPlan, created_unix: u64) -> Result<Value, String> {
    let body = PlanBody {
        plan: p.plan.clone(),
        masked_prompt: p.masked_prompt.clone(),
        model: p.model.clone(),
        base: p.base.as_ref().map(|b| BaseSpec {
            files: b.files.to_json(),
            history: b.history.clone(),
            layout: b.layout.clone(),
        }),
    };
    Ok(json!({
        "schema": SCHEMA,
        "id": p.id,
        "app": p.app,
        "created": created_unix,
        "plan": serde_json::to_value(&body).map_err(|e| e.to_string())?,
    }))
}

/// The plan and when it was made (unix seconds), or why it cannot be read.
fn from_spec(spec: &Value) -> Result<(StoredPlan, u64), String> {
    if spec["schema"].as_u64() != Some(SCHEMA) {
        return Err("unknown schema".into());
    }
    let id = spec["id"].as_str().ok_or("no id")?.to_string();
    let app = spec["app"].as_str().ok_or("no app")?.to_string();
    let created = spec["created"].as_u64().ok_or("no created")?;
    let body: PlanBody = serde_json::from_value(spec["plan"].clone()).map_err(|e| e.to_string())?;
    let base = match body.base {
        Some(b) => Some(BaseApp {
            files: parse_generated(&b.files.to_string()).map_err(|e| e.0)?,
            history: b.history,
            layout: b.layout,
        }),
        None => None,
    };
    Ok((
        StoredPlan {
            id,
            app,
            plan: body.plan,
            masked_prompt: body.masked_prompt,
            model: body.model,
            base,
            round_start: None,
            label: None,
        },
        created,
    ))
}

fn log_plan_problem(what: &str, id: &str, e: &dyn std::fmt::Display) {
    // IDs and the error only; never plan or user text.
    eprintln!("laoban-m0: {what} plan {id} failed: {e}");
}

impl M0Service {
    fn plan_records(&self) -> std::sync::MutexGuard<'_, HashMap<String, (u64, bool)>> {
        self.plan_records.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Write a plan (outside a round) so a restart keeps it: when it is
    /// made, and again after a failed apply (with the time it was made, so
    /// it still expires an hour after that).
    pub(super) fn save_plan(&self, p: &StoredPlan) {
        if !kept(p) {
            return;
        }
        let made = *self
            .plan_records()
            .entry(p.id.clone())
            .or_insert((now_unix(), false));
        let saved = self.write_plan(p, made.0);
        self.plan_records().insert(p.id.clone(), (made.0, saved));
    }

    fn write_plan(&self, p: &StoredPlan, made: u64) -> bool {
        let spec = match to_spec(p, made) {
            Ok(s) => s,
            Err(e) => {
                log_plan_problem("saving", &p.id, &e);
                return false;
            }
        };
        let labels = BTreeMap::from([(APP_LABEL.to_string(), p.app.clone())]);
        match self
            .state
            .create(PLANS, &object_name(&p.id), &labels, &spec)
        {
            Ok(_) | Err(StateError::Exists) => true,
            Err(StateError::NotInstalled(_)) => {
                self.note_plans_not_installed();
                false
            }
            Err(e) => {
                log_plan_problem("saving", &p.id, &e);
                false
            }
        }
    }

    /// The apply of a plan starts: its stored copy goes, so a restart in
    /// the middle cannot apply it again. `false` when a copy exists and
    /// could not be deleted (the apply must not go ahead).
    pub(super) fn forget_plan(&self, p: &StoredPlan) -> bool {
        if !kept(p) {
            return true;
        }
        let stored = self.plan_records().get(&p.id).is_some_and(|r| r.1);
        if !stored {
            return true;
        }
        match self.state.delete(PLANS, &object_name(&p.id)) {
            Ok(()) | Err(StateError::NotInstalled(_)) => {
                if let Some(r) = self.plan_records().get_mut(&p.id) {
                    r.1 = false;
                }
                true
            }
            Err(e) => {
                log_plan_problem("deleting", &p.id, &e);
                false
            }
        }
    }

    /// The plan was applied; it is never kept again.
    pub(super) fn plan_applied(&self, p: &StoredPlan) {
        self.plan_records().remove(&p.id);
    }

    fn note_plans_not_installed(&self) {
        if !self.plans_not_installed.swap(true, Ordering::SeqCst) {
            eprintln!(
                "laoban-m0: the Plan resource is not installed (upgrade the laoban-crd chart); \
                 plans are kept in memory only"
            );
        }
    }

    /// Read the stored plans into the plan store and delete expired ones.
    /// Returns whether the store could be read (else it is tried again on
    /// the next tick).
    pub fn restore_plans(&self) -> bool {
        let objects = match self.state.list(PLANS) {
            Ok(o) => o,
            Err(StateError::NotInstalled(_)) => {
                self.note_plans_not_installed();
                // Nothing to read; plans still work in memory.
                return true;
            }
            Err(e) => {
                // Tried again every tick; logged once.
                if !self.plans_read_failed.swap(true, Ordering::SeqCst) {
                    eprintln!("laoban-m0: reading the stored plans failed: {e}");
                }
                return false;
            }
        };
        let now_unix = now_unix();
        let now = Instant::now();
        let ttl = PLAN_TTL.as_secs();
        let mut restored = 0usize;
        for o in objects {
            match from_spec(&o.spec) {
                Ok((plan, created)) if created.saturating_add(ttl) > now_unix => {
                    let age = Duration::from_secs(now_unix.saturating_sub(created));
                    let made = now.checked_sub(age).unwrap_or(now);
                    let mut store = self.lock_store();
                    if !store.contains(&plan.id) {
                        self.plan_records().insert(plan.id.clone(), (created, true));
                        store.insert(plan, made);
                        restored += 1;
                    }
                }
                Ok(_) => {
                    let _ = self.state.delete(PLANS, &o.name);
                }
                Err(e) => {
                    eprintln!(
                        "laoban-m0: stored plan {} cannot be read ({e}); deleted",
                        o.name
                    );
                    let _ = self.state.delete(PLANS, &o.name);
                }
            }
        }
        if restored > 0 {
            eprintln!("laoban-m0: {restored} plan(s) waiting for confirmation read back");
        }
        true
    }

    /// Delete `Plan` resources whose plan has expired.
    pub(super) fn sweep_plans(&self, now_unix: u64) {
        let ttl = PLAN_TTL.as_secs();
        self.plan_records()
            .retain(|_, r| r.0.saturating_add(ttl) > now_unix);
        let Ok(objects) = self.state.list(PLANS) else {
            return;
        };
        for o in objects {
            let expired = o.spec["created"]
                .as_u64()
                .map_or(true, |c| c.saturating_add(ttl) <= now_unix);
            if expired {
                let _ = self.state.delete(PLANS, &o.name);
            }
        }
    }

    /// Called by the round timer: read the stored plans once, then sweep.
    pub(super) fn plans_tick(&self, now_unix: u64) {
        if !self.plans_restored.load(Ordering::SeqCst) {
            if self.restore_plans() {
                self.plans_restored.store(true, Ordering::SeqCst);
                self.plans_swept_at.store(now_unix, Ordering::SeqCst);
            }
            return;
        }
        let last = self.plans_swept_at.load(Ordering::SeqCst);
        if now_unix.saturating_sub(last) >= SWEEP_EVERY {
            self.plans_swept_at.store(now_unix, Ordering::SeqCst);
            self.sweep_plans(now_unix);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::MemoryStore;

    #[test]
    fn usage_adds_up_and_reads_back() {
        let mem: Arc<dyn StateStore> = Arc::new(MemoryStore::new());
        let u = StateUsage::new(mem.clone());
        assert_eq!(u.load(5).unwrap(), 0);
        u.add(5, 100).unwrap();
        u.add(5, 23).unwrap();
        assert_eq!(u.load(5).unwrap(), 123);
        assert_eq!(u.load(6).unwrap(), 0);
        let o = mem.get(TOKEN_USAGES, "tokens-1970-06").unwrap().unwrap();
        assert_eq!(o.spec["period"], "1970-06");
    }

    #[test]
    fn usage_reports_a_missing_resource() {
        let mem = MemoryStore::new();
        mem.set_installed(false);
        let u = StateUsage::new(Arc::new(mem));
        assert!(u.load(1).is_err());
        assert!(u.add(1, 5).is_err());
    }
}
