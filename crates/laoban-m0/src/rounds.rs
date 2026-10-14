//! 意見收斂: how several people change one app together (replaces the M0
//! edit lock; the requirement's "共同編輯" comes later).
//!
//! One round per app at a time:
//!
//! 1. Someone opens a round with the first opinion, a deadline and a scope
//!    (web, API or the whole app). Until the deadline anyone may add an
//!    opinion.
//! 2. At the deadline the AI converges all opinions into one change plan.
//! 3. Everyone who gave an opinion in this round confirms it, or objects
//!    with a comment: an objection adds it as an opinion and the round goes
//!    back to collecting for [`OBJECTION_MINUTES`]. The initiator may skip
//!    people who do not answer; the skip is recorded.
//! 4. When all have confirmed or been skipped, the AI writes the code and
//!    it is published like any change.
//!
//! This module is the state machine only (pure, no I/O, time passed in);
//! `service` runs the AI and GitHub steps. Rounds live in memory like plans:
//! a backend restart drops a round in progress.
//!
//! Users are the names the Kubernetes API server verified.

use std::collections::BTreeMap;

use serde::Serialize;

/// Longest single opinion, in characters.
pub const MAX_OPINION_CHARS: usize = 1_000;
/// All opinions of a round together, in characters (they go to the AI as
/// one change request).
pub const MAX_ROUND_CHARS: usize = 8_000;
pub const MAX_OPINIONS: usize = 30;
/// Longest deadline when opening a round.
pub const MAX_DEADLINE_MINUTES: u64 = 24 * 60;
/// After an objection, opinions are collected again for this long.
pub const OBJECTION_MINUTES: u64 = 10;
/// Log entries kept per round.
const MAX_LOG: usize = 100;

/// Which part of the app a round is about. Direct edits (the code editor)
/// to files outside it are not blocked while the round runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Part {
    Web,
    Api,
}

impl Part {
    /// The generated files of this part.
    pub fn files(self) -> &'static [&'static str] {
        match self {
            Part::Web => &["web/index.html", "web/app.js", "web/style.css"],
            Part::Api => &["api/handler.py"],
        }
    }
}

/// Parse a scope from the request: "web", "api" or "all".
pub fn parse_scope(s: &str) -> Option<Vec<Part>> {
    match s.trim() {
        "web" => Some(vec![Part::Web]),
        "api" => Some(vec![Part::Api]),
        "all" | "" => Some(vec![Part::Web, Part::Api]),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Taking opinions until the deadline.
    Collecting,
    /// The AI is turning the opinions into a plan.
    Converging,
    /// Waiting for everyone who gave an opinion.
    Confirming,
    /// The AI is writing the code; then it is published.
    Applying,
    /// Published. `result` has the new version.
    Done,
    /// Converging or applying failed; the initiator may converge again.
    Failed,
    /// The initiator gave up.
    Cancelled,
}

impl State {
    /// A round in these states blocks a new round and direct edits in scope.
    pub fn active(self) -> bool {
        matches!(
            self,
            State::Collecting
                | State::Converging
                | State::Confirming
                | State::Applying
                | State::Failed
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Opinion {
    pub by: String,
    pub at: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Answer {
    Pending,
    Confirmed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Confirmer {
    pub user: String,
    pub answer: Answer,
    /// When answered or skipped.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogEntry {
    pub at: String,
    pub by: String,
    /// The event in Traditional Chinese (fallback for clients).
    pub event: String,
    /// What happened, for the page to show in its language: `opened`,
    /// `opinion`, `objected`, `converging`, `converged`, `failed`,
    /// `confirmed`, `skipped`, `applying`, `done`, `cancelled`.
    pub code: &'static str,
    /// Values the event refers to (`minutes`, `count`, `user`, `message`).
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub args: std::collections::BTreeMap<&'static str, String>,
}

/// The plan a round converged to.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoundPlan {
    pub plan_id: String,
    pub summary: String,
    pub steps: Vec<String>,
    pub architecture: serde_json::Value,
    /// The AI's notes: opinions not followed, or why nothing changes.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Round {
    pub id: String,
    pub app: String,
    pub initiator: String,
    pub scope: Vec<Part>,
    pub state: State,
    /// Unix seconds; meaningful while collecting.
    pub deadline: u64,
    pub opinions: Vec<Opinion>,
    pub confirmers: Vec<Confirmer>,
    pub plan: Option<RoundPlan>,
    /// Why converging or applying failed.
    pub message: String,
    /// The apply response once published.
    pub result: Option<serde_json::Value>,
    pub log: Vec<LogEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoundError {
    /// Opinion empty or too long, bad deadline or scope.
    Invalid(&'static str),
    /// Not allowed in the current state.
    WrongState(State),
    /// Only the initiator may do this.
    NotInitiator,
    /// The caller has no answer to give in this round.
    NotConfirmer,
    /// The round has too many opinions or characters.
    Full,
}

fn clean(text: &str) -> Result<String, RoundError> {
    let t = text.trim();
    if t.is_empty() {
        return Err(RoundError::Invalid("請輸入意見。"));
    }
    if t.chars().count() > MAX_OPINION_CHARS {
        return Err(RoundError::Invalid("一則意見最多 1000 字。"));
    }
    Ok(t.to_string())
}

impl Round {
    /// Open a round. `now` is unix seconds, `at` its RFC 3339 form.
    #[allow(clippy::too_many_arguments)] // plain data, all required
    pub fn open(
        id: String,
        app: &str,
        user: &str,
        opinion: &str,
        deadline_minutes: u64,
        scope: Vec<Part>,
        now: u64,
        at: &str,
    ) -> Result<Self, RoundError> {
        if deadline_minutes > MAX_DEADLINE_MINUTES {
            return Err(RoundError::Invalid("截止時間最長 24 小時。"));
        }
        if scope.is_empty() {
            return Err(RoundError::Invalid("範圍不正確。"));
        }
        let text = clean(opinion)?;
        let mut r = Round {
            id,
            app: app.to_string(),
            initiator: user.to_string(),
            scope,
            state: State::Collecting,
            deadline: now + deadline_minutes * 60,
            opinions: vec![Opinion {
                by: user.to_string(),
                at: at.to_string(),
                text,
            }],
            confirmers: Vec::new(),
            plan: None,
            message: String::new(),
            result: None,
            log: Vec::new(),
        };
        r.note(
            at,
            user,
            "opened",
            &[("minutes", deadline_minutes.to_string())],
            format!("開始一輪（{deadline_minutes} 分鐘後收斂）"),
        );
        Ok(r)
    }

    fn note(
        &mut self,
        at: &str,
        by: &str,
        code: &'static str,
        args: &[(&'static str, String)],
        event: String,
    ) {
        self.log.push(LogEntry {
            at: at.to_string(),
            by: by.to_string(),
            event,
            code,
            args: args.iter().cloned().collect(),
        });
        if self.log.len() > MAX_LOG {
            self.log.remove(0);
        }
    }

    fn total_chars(&self) -> usize {
        self.opinions.iter().map(|o| o.text.chars().count()).sum()
    }

    /// Anyone may add an opinion while collecting.
    pub fn add_opinion(&mut self, user: &str, text: &str, at: &str) -> Result<(), RoundError> {
        if self.state != State::Collecting {
            return Err(RoundError::WrongState(self.state));
        }
        let text = clean(text)?;
        if self.opinions.len() >= MAX_OPINIONS
            || self.total_chars() + text.chars().count() > MAX_ROUND_CHARS
        {
            return Err(RoundError::Full);
        }
        self.opinions.push(Opinion {
            by: user.to_string(),
            at: at.to_string(),
            text,
        });
        self.note(at, user, "opinion", &[], "提出意見".into());
        Ok(())
    }

    /// Collecting and past the deadline.
    pub fn due(&self, now: u64) -> bool {
        self.state == State::Collecting && now >= self.deadline
    }

    /// Start converging (the deadline passed, or the initiator retries a
    /// failed round).
    pub fn start_converging(&mut self, at: &str) {
        self.state = State::Converging;
        self.plan = None;
        self.message.clear();
        self.note(
            at,
            "laoban",
            "converging",
            &[("count", self.opinions.len().to_string())],
            format!("AI 開始收斂 {} 則意見", self.opinions.len()),
        );
    }

    /// The change request the AI converges: every opinion with its author.
    pub fn change_request(&self) -> String {
        let mut s = format!(
            "以下是 {} 則對這個應用的意見（可能重複或互相衝突）：\n",
            self.opinions.len()
        );
        for (i, o) in self.opinions.iter().enumerate() {
            s.push_str(&format!("{}. （{}）{}\n", i + 1, o.by, o.text));
        }
        s
    }

    /// The opinions as a short list for the history: "（user）text；…".
    pub fn opinion_list(&self) -> String {
        self.opinions
            .iter()
            .map(|o| format!("（{}）{}", o.by, o.text))
            .collect::<Vec<_>>()
            .join("；")
    }

    /// People who must confirm: everyone who gave an opinion, in order.
    fn opinion_givers(&self) -> Vec<String> {
        let mut users: Vec<String> = Vec::new();
        for o in &self.opinions {
            if !users.contains(&o.by) {
                users.push(o.by.clone());
            }
        }
        users
    }

    pub fn converged(&mut self, plan: RoundPlan, at: &str) {
        self.state = State::Confirming;
        self.plan = Some(plan);
        self.confirmers = self
            .opinion_givers()
            .into_iter()
            .map(|user| Confirmer {
                user,
                answer: Answer::Pending,
                at: String::new(),
            })
            .collect();
        self.note(
            at,
            "laoban",
            "converged",
            &[],
            "修改計畫已產生，等待確認".into(),
        );
    }

    pub fn fail(&mut self, message: &str, at: &str) {
        self.state = State::Failed;
        self.message = message.to_string();
        self.note(
            at,
            "laoban",
            "failed",
            &[("message", message.to_string())],
            format!("失敗：{message}"),
        );
    }

    fn confirmer_mut(&mut self, user: &str) -> Result<&mut Confirmer, RoundError> {
        if self.state != State::Confirming {
            return Err(RoundError::WrongState(self.state));
        }
        self.confirmers
            .iter_mut()
            .find(|c| c.user == user && c.answer == Answer::Pending)
            .ok_or(RoundError::NotConfirmer)
    }

    pub fn confirm(&mut self, user: &str, at: &str) -> Result<(), RoundError> {
        let c = self.confirmer_mut(user)?;
        c.answer = Answer::Confirmed;
        c.at = at.to_string();
        self.note(at, user, "confirmed", &[], "確認".into());
        Ok(())
    }

    /// The initiator skips someone who does not answer.
    pub fn skip(&mut self, by: &str, user: &str, at: &str) -> Result<(), RoundError> {
        if by != self.initiator {
            return Err(RoundError::NotInitiator);
        }
        let c = self.confirmer_mut(user)?;
        c.answer = Answer::Skipped;
        c.at = at.to_string();
        self.note(
            at,
            by,
            "skipped",
            &[("user", user.to_string())],
            format!("略過 {user} 的確認"),
        );
        Ok(())
    }

    /// A confirmer disagrees: the comment becomes an opinion and opinions
    /// are collected again for [`OBJECTION_MINUTES`].
    pub fn object(&mut self, user: &str, text: &str, now: u64, at: &str) -> Result<(), RoundError> {
        let text = clean(text)?;
        self.confirmer_mut(user)?;
        if self.total_chars() + text.chars().count() > MAX_ROUND_CHARS
            || self.opinions.len() >= MAX_OPINIONS
        {
            return Err(RoundError::Full);
        }
        self.opinions.push(Opinion {
            by: user.to_string(),
            at: at.to_string(),
            text,
        });
        self.state = State::Collecting;
        self.deadline = now + OBJECTION_MINUTES * 60;
        self.plan = None;
        self.confirmers.clear();
        self.note(
            at,
            user,
            "objected",
            &[("minutes", OBJECTION_MINUTES.to_string())],
            format!("有意見，{OBJECTION_MINUTES} 分鐘後重新收斂"),
        );
        Ok(())
    }

    /// Everyone confirmed or was skipped.
    pub fn all_answered(&self) -> bool {
        self.state == State::Confirming
            && !self.confirmers.is_empty()
            && self.confirmers.iter().all(|c| c.answer != Answer::Pending)
    }

    pub fn start_applying(&mut self, at: &str) {
        self.state = State::Applying;
        self.note(
            at,
            "laoban",
            "applying",
            &[],
            "全部確認，AI 開始修改程式".into(),
        );
    }

    pub fn done(&mut self, result: serde_json::Value, at: &str) {
        self.state = State::Done;
        self.result = Some(result);
        self.note(at, "laoban", "done", &[], "已部署新版本".into());
    }

    /// The initiator converges again after a failure.
    pub fn check_retry(&self, by: &str) -> Result<(), RoundError> {
        if by != self.initiator {
            return Err(RoundError::NotInitiator);
        }
        if self.state != State::Failed {
            return Err(RoundError::WrongState(self.state));
        }
        Ok(())
    }

    pub fn cancel(&mut self, by: &str, at: &str) -> Result<(), RoundError> {
        if by != self.initiator {
            return Err(RoundError::NotInitiator);
        }
        if !matches!(
            self.state,
            State::Collecting | State::Confirming | State::Failed
        ) {
            return Err(RoundError::WrongState(self.state));
        }
        self.state = State::Cancelled;
        self.note(at, by, "cancelled", &[], "取消這一輪".into());
        Ok(())
    }

    /// Files a direct edit may not touch while this round is active.
    pub fn blocked_files(&self) -> Vec<&'static str> {
        if !self.state.active() {
            return Vec::new();
        }
        self.scope
            .iter()
            .flat_map(|p| p.files().iter().copied())
            .collect()
    }

    /// Widen the scope to the parts the converged plan changes.
    pub fn widen_scope(&mut self, parts: &[Part]) {
        for p in parts {
            if !self.scope.contains(p) {
                self.scope.push(*p);
            }
        }
        self.scope.sort();
    }

    /// The round as JSON for `user`, with what that user may do.
    pub fn to_json_for(&self, user: Option<&str>) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        let me = user.unwrap_or("");
        let pending: BTreeMap<&str, bool> = self
            .confirmers
            .iter()
            .map(|c| (c.user.as_str(), c.answer == Answer::Pending))
            .collect();
        v["you"] = serde_json::json!({
            "user": me,
            "isInitiator": !me.is_empty() && me == self.initiator,
            "mustConfirm": pending.get(me).copied().unwrap_or(false),
            "canAddOpinion": self.state == State::Collecting && !me.is_empty(),
        });
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_800_000_000;

    fn open(minutes: u64) -> Round {
        Round::open(
            "r1".into(),
            "todo",
            "u-a",
            "加上到期日",
            minutes,
            vec![Part::Web],
            T0,
            "t0",
        )
        .unwrap()
    }

    fn plan() -> RoundPlan {
        RoundPlan {
            plan_id: "p-1".into(),
            summary: "s".into(),
            steps: vec!["x".into()],
            architecture: serde_json::json!({}),
            notes: String::new(),
        }
    }

    #[test]
    fn collect_converge_confirm() {
        let mut r = open(30);
        assert!(!r.due(T0 + 29 * 60));
        r.add_opinion("u-b", "也要能排序", "t1").unwrap();
        r.add_opinion("u-a", "排序用到期日", "t2").unwrap();
        assert!(r.due(T0 + 30 * 60));
        r.start_converging("t3");
        assert!(r.change_request().contains("（u-b）也要能排序"));
        assert_eq!(
            r.add_opinion("u-c", "x", "t"),
            Err(RoundError::WrongState(State::Converging))
        );
        r.converged(plan(), "t4");
        let users: Vec<&str> = r.confirmers.iter().map(|c| c.user.as_str()).collect();
        assert_eq!(users, ["u-a", "u-b"], "each opinion giver once");
        assert_eq!(r.confirm("u-c", "t"), Err(RoundError::NotConfirmer));
        r.confirm("u-a", "t5").unwrap();
        assert_eq!(
            r.confirm("u-a", "t"),
            Err(RoundError::NotConfirmer),
            "already answered"
        );
        assert!(!r.all_answered());
        assert_eq!(r.skip("u-b", "u-b", "t"), Err(RoundError::NotInitiator));
        r.skip("u-a", "u-b", "t6").unwrap();
        assert!(r.all_answered());
        assert!(r.log.iter().any(|l| l.event.contains("略過 u-b")));
    }

    #[test]
    fn objection_goes_back_to_collecting() {
        let mut r = open(0);
        assert!(r.due(T0));
        r.start_converging("t");
        r.converged(plan(), "t");
        r.object("u-a", "改成用優先順序排序", T0 + 100, "t")
            .unwrap();
        assert_eq!(r.state, State::Collecting);
        assert_eq!(r.deadline, T0 + 100 + OBJECTION_MINUTES * 60);
        assert_eq!(r.opinions.len(), 2);
        assert!(r.plan.is_none() && r.confirmers.is_empty());
    }

    #[test]
    fn limits_and_permissions() {
        assert!(matches!(
            Round::open("r".into(), "a", "u", " ", 5, vec![Part::Web], T0, "t"),
            Err(RoundError::Invalid(_))
        ));
        assert!(matches!(
            Round::open(
                "r".into(),
                "a",
                "u",
                "x",
                24 * 60 + 1,
                vec![Part::Web],
                T0,
                "t"
            ),
            Err(RoundError::Invalid(_))
        ));
        let mut r = open(10);
        assert!(r.add_opinion("u", &"x".repeat(1001), "t").is_err());
        for _ in 0..7 {
            r.add_opinion("u", &"x".repeat(1000), "t").unwrap();
        }
        assert_eq!(
            r.add_opinion("u", &"x".repeat(1000), "t"),
            Err(RoundError::Full)
        );
        assert_eq!(r.cancel("u-b", "t"), Err(RoundError::NotInitiator));
        r.cancel("u-a", "t").unwrap();
        assert!(!r.state.active());
        assert!(r.blocked_files().is_empty());
    }

    #[test]
    fn scope_and_view() {
        let mut r = open(10);
        assert_eq!(
            r.blocked_files(),
            ["web/index.html", "web/app.js", "web/style.css"]
        );
        r.widen_scope(&[Part::Api]);
        assert!(r.blocked_files().contains(&"api/handler.py"));
        assert_eq!(parse_scope("all").unwrap(), [Part::Web, Part::Api]);
        assert!(parse_scope("db").is_none());
        let v = r.to_json_for(Some("u-a"));
        assert_eq!(v["you"]["isInitiator"], true);
        assert_eq!(v["state"], "collecting");
        assert_eq!(v["scope"], serde_json::json!(["web", "api"]));
        assert_eq!(r.to_json_for(Some("u-b"))["you"]["isInitiator"], false);
    }
}
