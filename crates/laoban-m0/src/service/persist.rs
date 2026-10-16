//! Keeping rounds of 意見收斂 across backend restarts.
//!
//! * Every change to a round is written to its `Round` resource before the
//!   request is answered; a refused write leaves the round as it was.
//! * At startup the rounds are read back. A round that was converging
//!   converges again; one that was being published is checked against the
//!   app (published, or failed so the initiator converges again; never
//!   published twice).
//! * A finished round (done or cancelled) is written, masked, to the
//!   `laoban-rounds` branch of the apps repository. Its resource stays
//!   until the app's next round starts, then it is deleted (once archived).

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::time::{Instant, SystemTime};

use serde_json::{json, Value};

use super::{commit_message, unix, M0Error, M0Service};
use crate::github::CommitFile;
use crate::plan::carry_layout;
use crate::rounds::{PlanRecipe, Round, RoundError, State};
use crate::state::{StateError, APP_LABEL, ROUNDS};
use crate::store::{BaseApp, StoredPlan};
use crate::time;

/// The branch of the apps repository finished rounds are archived to.
pub const ARCHIVE_BRANCH: &str = "laoban-rounds";
/// Directory on that branch: `rounds/<app>/<round>.json`.
pub const ARCHIVE_DIR: &str = "rounds";
/// Retry a failed archive after this many seconds, doubling, at most
/// [`ARCHIVE_MAX_BACKOFF`].
const ARCHIVE_BACKOFF: u64 = 10;
const ARCHIVE_MAX_BACKOFF: u64 = 600;
/// Shape of the stored spec and of the archive file.
const SCHEMA: u64 = 1;

type Rounds = std::collections::HashMap<String, Round>;

fn object_name(r: &Round) -> String {
    format!("{}-{}", r.app, r.id)
}

fn log_store_problem(what: &str, r: &Round, e: &StateError) {
    // Names and the error kind only; never opinion text.
    eprintln!("laoban-m0: {what} round {} of {} failed: {e}", r.id, r.app);
}

impl M0Service {
    pub(super) fn rounds_ready(&self) -> bool {
        self.rounds_ready.load(Ordering::SeqCst)
    }

    pub(super) fn require_rounds(&self) -> Result<(), M0Error> {
        if self.rounds_ready() {
            Ok(())
        } else {
            Err(M0Error::RoundsUnavailable)
        }
    }

    pub(super) fn state_error(&self, e: StateError) -> M0Error {
        match e {
            StateError::NotInstalled(_) => {
                self.rounds_ready.store(false, Ordering::SeqCst);
                M0Error::RoundsUnavailable
            }
            other => M0Error::State(other),
        }
    }

    fn spec_of(&self, r: &Round) -> Value {
        json!({
            "schema": SCHEMA,
            "app": r.app,
            "targetNamespace": self.settings.target_namespace,
            "state": r.state,
            "deadline": r.deadline,
            "round": r.to_stored(),
        })
    }

    /// Write the round to its resource (create it the first time).
    pub(super) fn write_round(&self, r: &mut Round) -> Result<(), StateError> {
        let spec = self.spec_of(r);
        let name = object_name(r);
        let version = match &r.store_version {
            Some(v) => self.state.update(ROUNDS, &name, v, &spec)?,
            None => {
                let labels = BTreeMap::from([(APP_LABEL.to_string(), r.app.clone())]);
                self.state.create(ROUNDS, &name, &labels, &spec)?
            }
        };
        r.store_version = Some(version);
        Ok(())
    }

    /// The round as stored now, if it is still there.
    fn read_round(&self, r: &Round) -> Result<Option<Round>, StateError> {
        let Some(o) = self.state.get(ROUNDS, &object_name(r))? else {
            return Ok(None);
        };
        Round::from_stored(o.spec["round"].clone(), Some(o.version), self.round_limits)
            .map(Some)
            .map_err(|_| StateError::Other("stored round cannot be read".into()))
    }

    /// A user's change to the app's round: `f` runs on a copy, the copy is
    /// written, and only then it replaces the round. If someone else wrote
    /// the round meanwhile, `f` runs again on what they wrote. If its
    /// resource was deleted (by hand, or with the CRD), it is created again
    /// from memory: the round goes on rather than getting stuck.
    pub(super) fn change_round<T, F>(&self, app: &str, mut f: F) -> Result<T, M0Error>
    where
        F: FnMut(&mut Round) -> Result<T, RoundError>,
    {
        self.require_rounds()?;
        let mut rs = self.rounds();
        for attempt in 0..3 {
            let current = match rs.get(app) {
                Some(r) if r.state.active() || r.state == State::Done => r,
                Some(r) => return Err(M0Error::Round(RoundError::WrongState(r.state))),
                None => return Err(M0Error::Round(RoundError::WrongState(State::Cancelled))),
            };
            let mut next = current.clone();
            let out = f(&mut next).map_err(M0Error::Round)?;
            match self.write_round(&mut next) {
                Ok(()) => {
                    rs.insert(app.to_string(), next);
                    return Ok(out);
                }
                Err(StateError::Conflict) if attempt < 2 => {
                    match self.read_round(current).map_err(|e| self.state_error(e))? {
                        Some(fresh) => {
                            rs.insert(app.to_string(), fresh);
                        }
                        None => {
                            if let Some(r) = rs.get_mut(app) {
                                r.store_version = None;
                            }
                        }
                    }
                }
                Err(e) => return Err(self.state_error(e)),
            }
        }
        Err(M0Error::State(StateError::Conflict))
    }

    /// laoban's own step on a round (converging, converged, failed,
    /// publishing, done). These are not refused: when the write fails the
    /// round still moves on in memory and the problem is logged; a restart
    /// then resumes from the last state written.
    pub(super) fn update_round<F>(&self, rs: &mut Rounds, app: &str, round_id: &str, f: F) -> bool
    where
        F: FnOnce(&mut Round),
    {
        let Some(current) = rs.get(app).filter(|r| r.id == round_id) else {
            return false;
        };
        let mut next = current.clone();
        f(&mut next);
        let mut result = self.write_round(&mut next);
        if result == Err(StateError::Conflict) {
            // laoban's step wins: write it over the newer version, or
            // create the resource again if it was deleted.
            match self.read_round(&next) {
                Ok(Some(fresh)) => {
                    next.store_version = fresh.store_version;
                    result = self.write_round(&mut next);
                }
                Ok(None) => {
                    next.store_version = None;
                    result = self.write_round(&mut next);
                }
                Err(_) => {}
            }
        }
        if let Err(e) = &result {
            log_store_problem("saving", &next, e);
            if matches!(e, StateError::NotInstalled(_)) {
                self.rounds_ready.store(false, Ordering::SeqCst);
            }
        }
        rs.insert(app.to_string(), next);
        true
    }

    /// The app's previous round, replaced by a new one: delete its resource
    /// once it is archived.
    pub(super) fn retire_round(&self, old: Round) {
        if !matches!(old.state, State::Done | State::Cancelled) {
            // Should not happen (a new round needs the old one finished);
            // keep it stored rather than lose it.
            return;
        }
        if old.archived {
            if let Err(e) = self.state.delete(ROUNDS, &object_name(&old)) {
                log_store_problem("deleting", &old, &e);
                self.archive_queue().push(old);
            }
        } else {
            self.archive_queue().push(old);
        }
    }

    fn archive_queue(&self) -> std::sync::MutexGuard<'_, Vec<Round>> {
        self.archive_queue.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Read every stored round back (startup, or once the CRD is installed).
    ///
    /// Only the first successful read takes the stored rounds and resumes
    /// work cut off by the restart. A later read (the store was unreachable
    /// or the CRD missing for a while, with this process running) keeps
    /// what is in memory, where work may still be going on, and writes
    /// again the rounds missing from the store.
    pub fn restore_rounds(&self) -> Result<usize, M0Error> {
        if self.restored_once.load(Ordering::SeqCst) {
            return self.rewrite_missing_rounds();
        }
        let objects = match self.state.list(ROUNDS) {
            Ok(o) => o,
            Err(e) => {
                self.rounds_ready.store(false, Ordering::SeqCst);
                if !matches!(e, StateError::NotInstalled(_)) {
                    eprintln!("laoban-m0: reading the stored rounds failed: {e}");
                }
                return Err(self.state_error(e));
            }
        };
        let mut by_app: std::collections::HashMap<String, Vec<Round>> = Default::default();
        for o in objects {
            match Round::from_stored(o.spec["round"].clone(), Some(o.version), self.round_limits) {
                Ok(r) => by_app.entry(r.app.clone()).or_default().push(r),
                Err(_) => eprintln!(
                    "laoban-m0: stored round {} cannot be read; left as is",
                    o.name
                ),
            }
        }
        let count = by_app.values().map(Vec::len).sum();
        let mut resume = Vec::new();
        let mut recover = Vec::new();
        {
            let mut rs = self.rounds();
            for (app, mut list) in by_app {
                // The app's round is the active one (there is at most one),
                // else the newest; the others are replaced ones.
                list.sort_by(|a, b| {
                    (a.state.active(), a.opened, &a.id).cmp(&(b.state.active(), b.opened, &b.id))
                });
                let Some(current) = list.pop() else { continue };
                for old in list {
                    self.retire_round(old);
                }
                match current.state {
                    State::Converging => resume.push((app.clone(), current.id.clone())),
                    State::Applying => recover.push((app.clone(), current.id.clone())),
                    _ => {}
                }
                if let Some(mine) = rs.get(&app) {
                    if mine.id != current.id {
                        // Not expected; the stored one is what survives.
                        eprintln!(
                            "laoban-m0: round {} of {app} replaced by the stored one",
                            mine.id
                        );
                    }
                }
                rs.insert(app, current);
            }
        }
        self.restored_once.store(true, Ordering::SeqCst);
        self.rounds_ready.store(true, Ordering::SeqCst);
        let at = time::rfc3339(SystemTime::now());
        for (app, id) in resume {
            {
                let mut rs = self.rounds();
                self.update_round(&mut rs, &app, &id, |r| r.note_restarted(&at));
            }
            self.in_background("laoban-converge", move |s| s.converge(&app, &id));
        }
        for (app, id) in recover {
            self.in_background("laoban-recover", move |s| s.recover_applying(&app, &id));
        }
        Ok(count)
    }

    /// A later read of the store: the rounds in memory stay as they are;
    /// those whose resource is gone are created again.
    fn rewrite_missing_rounds(&self) -> Result<usize, M0Error> {
        let objects = match self.state.list(ROUNDS) {
            Ok(o) => o,
            Err(e) => {
                self.rounds_ready.store(false, Ordering::SeqCst);
                return Err(self.state_error(e));
            }
        };
        let names: std::collections::HashSet<String> =
            objects.into_iter().map(|o| o.name).collect();
        let mut rewritten = 0;
        {
            let mut rs = self.rounds();
            for r in rs.values_mut() {
                if !names.contains(&object_name(r)) {
                    r.store_version = None;
                    match self.write_round(r) {
                        Ok(()) => rewritten += 1,
                        Err(e) => log_store_problem("writing again", r, &e),
                    }
                }
            }
        }
        self.rounds_ready.store(true, Ordering::SeqCst);
        Ok(rewritten)
    }

    /// The backend restarted while a round was being published: see if the
    /// new version made it to GitHub. Never publishes again.
    fn recover_applying(&self, app: &str, round_id: &str) {
        let (label, since) = {
            let rs = self.rounds();
            match rs.get(app) {
                Some(r) if r.id == round_id && r.state == State::Applying => (
                    r.recipe.as_ref().and_then(|p| p.label.clone()),
                    r.log
                        .iter()
                        .rev()
                        .find(|l| l.code == "applying")
                        .map(|l| l.at.clone())
                        .unwrap_or_default(),
                ),
                _ => return,
            }
        };
        let published = self.load_app(app).and_then(|m| {
            // Any later entry: hand edits or a layout may have come after.
            let found = m.history.iter().rev().any(|h| {
                h.kind == "edit" && label.as_deref() == Some(h.request.as_str()) && h.at >= since
            });
            if found {
                let sha = self.latest_commit(app)?;
                Ok(Some(self.version_response(app, "edit", &sha, &m.files)))
            } else {
                Ok(None)
            }
        });
        let at = time::rfc3339(SystemTime::now());
        let mut rs = self.rounds();
        match published {
            Ok(Some(v)) => {
                self.update_round(&mut rs, app, round_id, |r| r.done(v, &at));
            }
            Ok(None) => {
                self.update_round(&mut rs, app, round_id, |r| {
                    r.fail_coded(
                        "restartUnpublished",
                        &[],
                        "後端在發布途中重新啟動，這一輪的修改沒有發布。請發起人重新收斂。",
                        &at,
                    )
                });
            }
            Err(_) => {
                self.update_round(&mut rs, app, round_id, |r| {
                    r.fail_coded(
                        "restartUnknown",
                        &[],
                        "後端在發布途中重新啟動，無法確認這一輪的修改是否已發布。請到版本紀錄確認；沒有新版本時請發起人重新收斂。",
                        &at,
                    )
                });
            }
        }
    }

    /// Put the round's plan back in the plan store (it is in memory and
    /// lost on restart, or expired after an hour) from what the round kept.
    pub(super) fn ensure_round_plan(
        &self,
        app: &str,
        round_id: &str,
        plan_id: &str,
    ) -> Result<(), M0Error> {
        // An expired plan counts as missing (it would be purged on apply).
        if self.lock_store().is_pending(plan_id, Instant::now()) {
            return Ok(());
        }
        let (recipe, start_commit): (PlanRecipe, String) = {
            let rs = self.rounds();
            match rs.get(app).filter(|r| r.id == round_id) {
                Some(r) => match &r.recipe {
                    Some(p) => (p.clone(), r.start_commit.clone()),
                    None => return Err(M0Error::PlanNotFound),
                },
                None => return Err(M0Error::PlanNotFound),
            }
        };
        let base = self.load_version(app, &recipe.base_commit)?;
        let round_start = if start_commit.is_empty() || start_commit == recipe.base_commit {
            None
        } else {
            Some(self.load_version(app, &start_commit)?.files)
        };
        let layout = carry_layout(&base.plan, &recipe.plan, app, &base.layout);
        self.lock_store().insert(
            StoredPlan {
                id: plan_id.to_string(),
                app: app.to_string(),
                plan: recipe.plan,
                masked_prompt: recipe.masked_prompt,
                model: recipe.model,
                base: Some(BaseApp {
                    files: base.files,
                    history: base.history,
                    layout,
                }),
                round_start,
                label: recipe.label,
            },
            Instant::now(),
        );
        Ok(())
    }

    /// Archive finished rounds that are not archived yet, and delete the
    /// resources of replaced rounds once archived. Called by the round
    /// timer (and tests); failures are retried with a growing pause.
    pub fn archive_tick(&self, now_unix: u64) {
        if !self.rounds_ready() {
            return;
        }
        let queued: Vec<Round> = self.archive_queue().clone();
        let current: Vec<Round> = self
            .rounds()
            .values()
            .filter(|r| matches!(r.state, State::Done | State::Cancelled) && !r.archived)
            .cloned()
            .collect();
        for r in queued {
            if !r.archived {
                if !self.archive_due(&r.id, now_unix) {
                    continue;
                }
                if let Err(e) = self.archive_round(&r, now_unix) {
                    self.archive_failed(&r, now_unix, &e.to_string());
                    continue;
                }
            }
            match self.state.delete(ROUNDS, &object_name(&r)) {
                Ok(()) => self
                    .archive_queue()
                    .retain(|q| q.id != r.id || q.app != r.app),
                Err(e) => {
                    log_store_problem("deleting", &r, &e);
                    if let Some(q) = self
                        .archive_queue()
                        .iter_mut()
                        .find(|q| q.id == r.id && q.app == r.app)
                    {
                        q.archived = true;
                    }
                }
            }
        }
        for r in current {
            if !self.archive_due(&r.id, now_unix) {
                continue;
            }
            match self.archive_round(&r, now_unix) {
                Ok(()) => {
                    let mut rs = self.rounds();
                    self.update_round(&mut rs, &r.app, &r.id, |c| c.archived = true);
                }
                Err(e) => self.archive_failed(&r, now_unix, &e.to_string()),
            }
        }
    }

    fn archive_due(&self, round_id: &str, now_unix: u64) -> bool {
        self.archive_retry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(round_id)
            .map_or(true, |(_, next)| *next <= now_unix)
    }

    fn archive_failed(&self, r: &Round, now_unix: u64, detail: &str) {
        let mut retry = self.archive_retry.lock().unwrap_or_else(|e| e.into_inner());
        let entry = retry.entry(r.id.clone()).or_insert((0, 0));
        entry.0 += 1;
        let pause = ARCHIVE_BACKOFF
            .saturating_mul(1u64 << entry.0.min(10))
            .min(ARCHIVE_MAX_BACKOFF);
        entry.1 = now_unix + pause;
        eprintln!(
            "laoban-m0: archiving round {} of {} failed ({detail}); retrying in {pause} s",
            r.id, r.app
        );
    }

    /// Write the finished round to the archive branch, with opinions,
    /// replies and plan comments masked like everything else laoban
    /// writes to GitHub.
    fn archive_round(&self, r: &Round, now_unix: u64) -> Result<(), M0Error> {
        let mut masked = r.clone();
        for o in &mut masked.opinions {
            o.text = self.orchestrator.mask_for_ai(&o.text).text;
            for reply in &mut o.replies {
                reply.text = self.orchestrator.mask_for_ai(&reply.text).text;
            }
        }
        for c in &mut masked.plan_comments {
            c.text = self.orchestrator.mask_for_ai(&c.text).text;
        }
        let doc = json!({
            "schema": SCHEMA,
            "app": r.app,
            "targetNamespace": self.settings.target_namespace,
            "archivedAt": time::rfc3339(std::time::UNIX_EPOCH + std::time::Duration::from_secs(now_unix)),
            "openedAt": time::rfc3339(std::time::UNIX_EPOCH + std::time::Duration::from_secs(r.opened)),
            "round": masked.to_public_json(),
        });
        let content = serde_json::to_string_pretty(&doc)
            .map_err(|e| M0Error::Internal(e.to_string()))?
            + "\n";
        self.github
            .commit_to_branch(
                ARCHIVE_BRANCH,
                &[CommitFile {
                    path: format!("{ARCHIVE_DIR}/{}/{}.json", r.app, r.id),
                    content,
                }],
                &commit_message(&r.app, &format!("意見收斂紀錄 {}", r.id)),
            )
            .map_err(M0Error::Github)?;
        self.archive_retry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&r.id);
        Ok(())
    }
}

/// Most past rounds listed per app.
pub const MAX_PAST_ROUNDS: usize = 50;

/// `r-YYYYMMDD-xxxxxx`, as round IDs are made.
pub fn valid_round_id(id: &str) -> bool {
    let b = id.as_bytes();
    b.len() == 17
        && id.starts_with("r-")
        && b[2..10].iter().all(u8::is_ascii_digit)
        && b[10] == b'-'
        && b[11..]
            .iter()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
}

impl M0Service {
    /// `GET .../apps/<name>/rounds`: the app's archived rounds, newest
    /// first (by ID: date, then the second of the day; rounds from before
    /// v0.1.17 have a random part instead and sort among their day).
    pub fn past_rounds(&self, app: &str) -> Result<Value, M0Error> {
        if !crate::plan::valid_app_name(app) {
            return Err(M0Error::BadRequest("應用名稱不正確。"));
        }
        let names = self
            .github
            .list_files_at(&format!("{ARCHIVE_DIR}/{app}"), ARCHIVE_BRANCH)
            .map_err(M0Error::Github)?;
        let mut ids: Vec<&str> = names
            .iter()
            .filter_map(|n| n.strip_suffix(".json"))
            .filter(|id| valid_round_id(id))
            .collect();
        ids.sort_unstable_by(|a, b| b[2..10].cmp(&a[2..10]).then(b.cmp(a)));
        let truncated = ids.len() > MAX_PAST_ROUNDS;
        let rounds: Vec<Value> = ids
            .into_iter()
            .take(MAX_PAST_ROUNDS)
            .map(|id| {
                json!({
                    "id": id,
                    "date": format!("{}-{}-{}", &id[2..6], &id[6..8], &id[8..10]),
                })
            })
            .collect();
        Ok(json!({ "rounds": rounds, "truncated": truncated }))
    }

    /// `GET .../apps/<name>/rounds/<id>`: one archived round (masked, as
    /// written to the archive).
    pub fn past_round(&self, app: &str, id: &str) -> Result<Value, M0Error> {
        if !crate::plan::valid_app_name(app) || !valid_round_id(id) {
            return Err(M0Error::BadRequest("輪次代號不正確。"));
        }
        let text = self
            .github
            .read_file_at(&format!("{ARCHIVE_DIR}/{app}/{id}.json"), ARCHIVE_BRANCH)
            .map_err(M0Error::Github)?
            .ok_or(M0Error::PastRoundNotFound)?;
        let doc: Value = serde_json::from_str(&text).map_err(|_| M0Error::PastRoundNotFound)?;
        if doc["app"] != app || !doc["round"].is_object() {
            return Err(M0Error::PastRoundNotFound);
        }
        Ok(json!({
            "round": doc["round"],
            "archivedAt": doc["archivedAt"],
            "openedAt": doc["openedAt"],
        }))
    }
}

/// Unix seconds now (for the timer).
pub(super) fn now_unix() -> u64 {
    unix(SystemTime::now())
}
