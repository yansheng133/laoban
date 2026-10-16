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
//! `service` runs the AI and GitHub steps and keeps every round in a
//! `Round` resource (see `state`), so a round survives a backend restart.
//!
//! Users are the names the Kubernetes API server verified.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::plan::AppPlan;

/// Longest single opinion, in characters.
pub const MAX_OPINION_CHARS: usize = 1_000;
/// Longest reply, in characters.
pub const MAX_REPLY_CHARS: usize = 300;
/// All opinions and replies of a round together, in characters (they go to
/// the AI as one change request).
pub const MAX_ROUND_CHARS: usize = 8_000;
pub const MAX_OPINIONS: usize = 30;
/// Opinions sent together (one per component, for example).
pub const MAX_BATCH: usize = 10;
/// Replies under one opinion.
pub const MAX_REPLIES: usize = 20;
/// The converged plan as stored with the round (summary, steps,
/// architecture and what is needed to write the code), in bytes of JSON.
pub const MAX_PLAN_BYTES: usize = 150 * 1024;
/// Longest deadline when opening a round.
pub const MAX_DEADLINE_MINUTES: u64 = 24 * 60;
/// After an objection, opinions are collected again for this long.
pub const OBJECTION_MINUTES: u64 = 10;
/// Log entries kept per round.
const MAX_LOG: usize = 100;

/// The size limits of a round. The administrator may lower them (chart
/// values), never raise them above the constants: everything in a round
/// goes to the AI and is stored in one Kubernetes resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    pub opinion_chars: usize,
    pub reply_chars: usize,
    pub round_chars: usize,
    pub opinions: usize,
    pub replies: usize,
    pub batch: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            opinion_chars: MAX_OPINION_CHARS,
            reply_chars: MAX_REPLY_CHARS,
            round_chars: MAX_ROUND_CHARS,
            opinions: MAX_OPINIONS,
            replies: MAX_REPLIES,
            batch: MAX_BATCH,
        }
    }
}

impl Limits {
    /// The defaults are the highest values allowed.
    pub fn within_maximum(&self) -> bool {
        let max = Limits::default();
        let ok = |v: usize, m: usize| (1..=m).contains(&v);
        ok(self.opinion_chars, max.opinion_chars)
            && ok(self.reply_chars, max.reply_chars)
            && ok(self.round_chars, max.round_chars)
            && ok(self.opinions, max.opinions)
            && ok(self.replies, max.replies)
            && ok(self.batch, max.batch)
    }
}

/// Which part of the app a round is about. Direct edits (the code editor)
/// to files outside it are not blocked while the round runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// The component of the app's architecture an opinion is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// Node id in the architecture (as the 3D view uses it).
    pub id: String,
    /// The component's name, for people and the AI.
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub by: String,
    pub at: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Opinion {
    /// "o1", "o2", … within the round.
    pub id: String,
    pub by: String,
    pub at: String,
    pub text: String,
    /// None: about the app in general.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
    /// People who +1'd it (not the author).
    pub supporters: Vec<String>,
    pub replies: Vec<Reply>,
}

/// An opinion as sent: text and, optionally, the component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOpinion {
    pub text: String,
    pub target: Option<Target>,
}

impl NewOpinion {
    pub fn general(text: &str) -> Self {
        NewOpinion {
            text: text.to_string(),
            target: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Answer {
    Pending,
    Confirmed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Confirmer {
    pub user: String,
    pub answer: Answer,
    /// When answered or skipped.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub at: String,
    pub by: String,
    /// The event in Traditional Chinese (fallback for clients).
    pub event: String,
    /// What happened, for the page to show in its language: `opened`,
    /// `opinion`, `objected`, `converging`, `converged`, `failed`,
    /// `confirmed`, `skipped`, `applying`, `done`, `cancelled`.
    pub code: String,
    /// Values the event refers to (`minutes`, `count`, `user`, `message`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<String, String>,
}

/// The plan a round converged to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoundPlan {
    pub plan_id: String,
    pub summary: String,
    pub steps: Vec<String>,
    pub architecture: serde_json::Value,
    /// The AI's notes: opinions not followed, or why nothing changes.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

/// What writing the code for a round's plan needs, kept with the round so
/// the plan can still be published after a backend restart (the plan
/// store itself is in memory). Holds only masked text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanRecipe {
    pub plan: AppPlan,
    pub masked_prompt: String,
    pub model: String,
    /// The opinions as shown in the history (masked).
    #[serde(default)]
    pub label: Option<String>,
    /// The app's commit the plan was made from.
    pub base_commit: String,
}

/// Fields kept in the stored round but not shown to the page.
const HIDDEN: [&str; 5] = ["lastOpinion", "startCommit", "recipe", "archived", "opened"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Why converging or applying failed (Traditional Chinese).
    pub message: String,
    /// The same as a code the page translates (`laoban.round.failures.*`),
    /// with the values it refers to; empty when there is no translation.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message_code: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub message_args: BTreeMap<String, String>,
    /// The apply response once published.
    pub result: Option<serde_json::Value>,
    pub log: Vec<LogEntry>,
    /// Number of the last opinion ID given out.
    #[serde(default)]
    last_opinion: u32,
    /// The app's commit when the round opened: its files are the starting
    /// point for telling hand edits made during the round.
    #[serde(default)]
    pub start_commit: String,
    /// Set once converged (see [`PlanRecipe`]).
    #[serde(default)]
    pub recipe: Option<PlanRecipe>,
    /// Finished and written to the archive branch in GitHub.
    #[serde(default)]
    pub archived: bool,
    /// Unix seconds when opened (orders the rounds of an app).
    #[serde(default)]
    pub opened: u64,
    /// The stored resource's version, for writes that must not overwrite
    /// someone else's (Kubernetes `resourceVersion`).
    #[serde(skip)]
    pub store_version: Option<String>,
    #[serde(skip)]
    limits: Limits,
}

/// What a length limit is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Text {
    Opinion,
    Reply,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoundError {
    /// Opinion empty, bad deadline or scope.
    Invalid(&'static str),
    /// An opinion or reply is longer than `max` characters.
    TooLong { what: Text, max: usize },
    /// Not allowed in the current state.
    WrongState(State),
    /// Only the initiator may do this.
    NotInitiator,
    /// The caller has no answer to give in this round.
    NotConfirmer,
    /// The round has too many opinions, replies or characters.
    Full {
        opinions: usize,
        replies: usize,
        chars: usize,
    },
    /// No opinion with that ID in this round.
    NoSuchOpinion,
}

/// Characters as the page counts them (Unicode scalar values).
pub fn char_count(text: &str) -> usize {
    text.trim().chars().count()
}

fn clean(text: &str, what: Text, max: usize) -> Result<String, RoundError> {
    let t = text.trim();
    if t.is_empty() {
        return Err(RoundError::Invalid(match what {
            Text::Opinion => "請輸入意見。",
            Text::Reply => "請輸入回覆。",
        }));
    }
    if t.chars().count() > max {
        return Err(RoundError::TooLong { what, max });
    }
    Ok(t.to_string())
}

impl Round {
    /// Read a stored round back. `version` is the resource's version.
    pub fn from_stored(
        v: serde_json::Value,
        version: Option<String>,
        limits: Limits,
    ) -> Result<Self, serde_json::Error> {
        let mut r: Round = serde_json::from_value(v)?;
        // IDs are never reused within a round, even if the counter was lost.
        let highest = r
            .opinions
            .iter()
            .filter_map(|o| o.id.trim_start_matches('o').parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        r.last_opinion = r.last_opinion.max(highest);
        r.store_version = version;
        r.limits = limits;
        Ok(r)
    }

    /// The round as stored (everything, including the hidden fields).
    pub fn to_stored(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
    }

    pub fn set_limits(&mut self, limits: Limits) {
        self.limits = limits;
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    fn full(&self) -> RoundError {
        RoundError::Full {
            opinions: self.limits.opinions,
            replies: self.limits.replies,
            chars: self.limits.round_chars,
        }
    }

    /// Open a round with the default limits. `now` is unix seconds, `at`
    /// its RFC 3339 form.
    #[allow(clippy::too_many_arguments)] // plain data, all required
    pub fn open(
        id: String,
        app: &str,
        user: &str,
        opinions: Vec<NewOpinion>,
        deadline_minutes: u64,
        scope: Vec<Part>,
        now: u64,
        at: &str,
    ) -> Result<Self, RoundError> {
        Self::open_with(
            Limits::default(),
            id,
            app,
            user,
            opinions,
            deadline_minutes,
            scope,
            now,
            at,
        )
    }

    /// [`Round::open`] with the given limits.
    #[allow(clippy::too_many_arguments)] // plain data, all required
    pub fn open_with(
        limits: Limits,
        id: String,
        app: &str,
        user: &str,
        opinions: Vec<NewOpinion>,
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
        let mut r = Round {
            id,
            app: app.to_string(),
            initiator: user.to_string(),
            scope,
            state: State::Collecting,
            deadline: now + deadline_minutes * 60,
            opinions: Vec::new(),
            confirmers: Vec::new(),
            plan: None,
            message: String::new(),
            message_code: String::new(),
            message_args: BTreeMap::new(),
            result: None,
            log: Vec::new(),
            last_opinion: 0,
            start_commit: String::new(),
            recipe: None,
            archived: false,
            opened: now,
            store_version: None,
            limits,
        };
        r.push_opinions(user, opinions, at)?;
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
            code: code.to_string(),
            args: args
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        });
        if self.log.len() > MAX_LOG {
            self.log.remove(0);
        }
    }

    /// Opinions and replies together, in characters.
    pub fn total_chars(&self) -> usize {
        self.opinions
            .iter()
            .map(|o| {
                o.text.chars().count()
                    + o.replies
                        .iter()
                        .map(|r| r.text.chars().count())
                        .sum::<usize>()
            })
            .sum()
    }

    /// Check and add opinions all together (none if one is refused).
    fn push_opinions(
        &mut self,
        user: &str,
        items: Vec<NewOpinion>,
        at: &str,
    ) -> Result<usize, RoundError> {
        if items.is_empty() {
            return Err(RoundError::Invalid("請輸入意見。"));
        }
        if items.len() > self.limits.batch {
            return Err(RoundError::Invalid("一次送出的意見太多。"));
        }
        let mut cleaned = Vec::with_capacity(items.len());
        for item in items {
            cleaned.push((
                clean(&item.text, Text::Opinion, self.limits.opinion_chars)?,
                item.target,
            ));
        }
        let added: usize = cleaned.iter().map(|(t, _)| t.chars().count()).sum();
        if self.opinions.len() + cleaned.len() > self.limits.opinions
            || self.total_chars() + added > self.limits.round_chars
        {
            return Err(self.full());
        }
        let count = cleaned.len();
        for (text, target) in cleaned {
            self.last_opinion += 1;
            self.opinions.push(Opinion {
                id: format!("o{}", self.last_opinion),
                by: user.to_string(),
                at: at.to_string(),
                text,
                target,
                supporters: Vec::new(),
                replies: Vec::new(),
            });
        }
        Ok(count)
    }

    /// Anyone may add opinions while collecting, several at once.
    pub fn add_opinions(
        &mut self,
        user: &str,
        items: Vec<NewOpinion>,
        at: &str,
    ) -> Result<(), RoundError> {
        if self.state != State::Collecting {
            return Err(RoundError::WrongState(self.state));
        }
        let count = self.push_opinions(user, items, at)?;
        self.note(
            at,
            user,
            "opinion",
            &[("count", count.to_string())],
            format!("提出 {count} 則意見"),
        );
        Ok(())
    }

    /// One opinion added while collecting.
    pub fn add_opinion(&mut self, user: &str, text: &str, at: &str) -> Result<(), RoundError> {
        self.add_opinions(user, vec![NewOpinion::general(text)], at)
    }

    fn discussable(&self) -> Result<(), RoundError> {
        match self.state {
            State::Collecting | State::Confirming => Ok(()),
            s => Err(RoundError::WrongState(s)),
        }
    }

    /// +1 an opinion, or take the +1 back. Returns whether the user now
    /// supports it. Allowed while collecting or confirming; replies and
    /// support while confirming are discussion only (the plan stays).
    pub fn toggle_support(
        &mut self,
        user: &str,
        opinion: &str,
        at: &str,
    ) -> Result<bool, RoundError> {
        self.discussable()?;
        let o = self
            .opinions
            .iter_mut()
            .find(|o| o.id == opinion)
            .ok_or(RoundError::NoSuchOpinion)?;
        if o.by == user {
            return Err(RoundError::Invalid("不能附議自己的意見。"));
        }
        let now = if let Some(i) = o.supporters.iter().position(|u| u == user) {
            o.supporters.remove(i);
            false
        } else {
            o.supporters.push(user.to_string());
            true
        };
        let id = o.id.clone();
        if now {
            self.note(
                at,
                user,
                "supported",
                &[("opinion", id.clone())],
                format!("附議意見 {id}"),
            );
        } else {
            self.note(
                at,
                user,
                "unsupported",
                &[("opinion", id.clone())],
                format!("取消附議意見 {id}"),
            );
        }
        Ok(now)
    }

    /// Reply under an opinion, while collecting or confirming.
    pub fn reply(
        &mut self,
        user: &str,
        opinion: &str,
        text: &str,
        at: &str,
    ) -> Result<(), RoundError> {
        self.discussable()?;
        let text = clean(text, Text::Reply, self.limits.reply_chars)?;
        let total = self.total_chars();
        let full = self.full();
        let limits = self.limits;
        let o = self
            .opinions
            .iter_mut()
            .find(|o| o.id == opinion)
            .ok_or(RoundError::NoSuchOpinion)?;
        if o.replies.len() >= limits.replies || total + text.chars().count() > limits.round_chars {
            return Err(full);
        }
        o.replies.push(Reply {
            by: user.to_string(),
            at: at.to_string(),
            text,
        });
        let id = o.id.clone();
        self.note(
            at,
            user,
            "replied",
            &[("opinion", id.clone())],
            format!("回覆意見 {id}"),
        );
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
        self.recipe = None;
        self.message.clear();
        self.message_code.clear();
        self.message_args.clear();
        self.note(
            at,
            "laoban",
            "converging",
            &[("count", self.opinions.len().to_string())],
            format!("AI 開始收斂 {} 則意見", self.opinions.len()),
        );
    }

    /// The change request the AI converges: the opinions grouped by
    /// component (general ones first), each with its author, the people
    /// who support it and the replies under it.
    pub fn change_request(&self) -> String {
        let mut s = format!(
            "以下是 {} 則對這個應用的意見（可能重複或互相衝突），依針對的元件分組；附議越多的意見越多人同意：\n",
            self.opinions.len()
        );
        let mut groups: Vec<(Option<&Target>, Vec<&Opinion>)> = Vec::new();
        for o in &self.opinions {
            let key = o.target.as_ref();
            match groups
                .iter_mut()
                .find(|(k, _)| k.map(|t| &t.id) == key.map(|t| &t.id))
            {
                Some((_, list)) => list.push(o),
                None => groups.push((key, vec![o])),
            }
        }
        groups.sort_by_key(|(k, _)| k.is_some());
        for (target, list) in groups {
            match target {
                None => s.push_str("\n【整個應用】\n"),
                Some(t) => s.push_str(&format!("\n【元件：{}】\n", t.name)),
            }
            for o in list {
                // "#2" as the page numbers it (the ID without its "o"),
                // so notes that cite an opinion can be found on the page.
                s.push_str(&format!(
                    "- #{}（{}）{}",
                    o.id.trim_start_matches('o'),
                    o.by,
                    o.text
                ));
                if !o.supporters.is_empty() {
                    s.push_str(&format!(
                        "　〔{} 人附議：{}〕",
                        o.supporters.len(),
                        o.supporters.join("、")
                    ));
                }
                s.push('\n');
                for r in &o.replies {
                    s.push_str(&format!("    回覆（{}）{}\n", r.by, r.text));
                }
            }
        }
        s
    }

    /// The opinions as a short list for the history: "（user）text；…".
    pub fn opinion_list(&self) -> String {
        self.opinions
            .iter()
            .map(|o| match &o.target {
                Some(t) => format!("（{}）［{}］{}", o.by, t.name, o.text),
                None => format!("（{}）{}", o.by, o.text),
            })
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
        self.fail_coded("", &[], message, at);
    }

    /// Fail with a code the page can show in its language (`message` is
    /// the Traditional Chinese text, kept for clients without it).
    pub fn fail_coded(&mut self, code: &str, args: &[(&str, String)], message: &str, at: &str) {
        self.state = State::Failed;
        self.message = message.to_string();
        self.message_code = code.to_string();
        self.message_args = args
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        let mut log_args: Vec<(&'static str, String)> = vec![("message", message.to_string())];
        if !code.is_empty() {
            log_args.push(("code", code.to_string()));
            for (k, v) in args {
                // Only the known argument names go into the log.
                if *k == "files" {
                    log_args.push(("files", v.clone()));
                }
            }
        }
        self.note(
            at,
            "laoban",
            "failed",
            &log_args,
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
        clean(text, Text::Opinion, self.limits.opinion_chars)?;
        self.confirmer_mut(user)?;
        self.push_opinions(user, vec![NewOpinion::general(text)], at)?;
        self.state = State::Collecting;
        self.deadline = now + OBJECTION_MINUTES * 60;
        self.plan = None;
        self.recipe = None;
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

    /// After an answer: if everyone has answered, start applying (in the
    /// same change as the answer, so no state has everyone answered but
    /// nothing going on). Returns whether it started.
    pub fn begin_applying_if_answered(&mut self, at: &str) -> bool {
        if self.all_answered() {
            self.start_applying(at);
            true
        } else {
            false
        }
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

    /// The backend restarted while the AI was converging: it starts again.
    pub fn note_restarted(&mut self, at: &str) {
        self.note(
            at,
            "laoban",
            "restarted",
            &[],
            "後端重新啟動，AI 重新開始收斂".into(),
        );
    }

    pub fn done(&mut self, result: serde_json::Value, at: &str) {
        self.state = State::Done;
        self.result = Some(result);
        // Written to GitHub; Fleet deploys it shortly after (report 20, N6).
        self.note(
            at,
            "laoban",
            "done",
            &[],
            "新版本已寫入 GitHub，等待 Fleet 部署".into(),
        );
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

    /// The round as shown: without the stored-only fields.
    pub fn to_public_json(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        if let Some(o) = v.as_object_mut() {
            for k in HIDDEN {
                o.remove(k);
            }
        }
        v
    }

    /// The round as JSON for `user`, with what that user may do, the
    /// limits and how much of them is used.
    pub fn to_json_for(&self, user: Option<&str>) -> serde_json::Value {
        let mut v = self.to_public_json();
        v["limits"] = serde_json::json!(self.limits);
        v["used"] = serde_json::json!({
            "chars": self.total_chars(),
            "opinions": self.opinions.len(),
        });
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
            "canDiscuss": matches!(self.state, State::Collecting | State::Confirming) && !me.is_empty(),
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
            vec![NewOpinion::general("加上到期日")],
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
            Round::open(
                "r".into(),
                "a",
                "u",
                vec![NewOpinion::general(" ")],
                5,
                vec![Part::Web],
                T0,
                "t"
            ),
            Err(RoundError::Invalid(_))
        ));
        assert!(matches!(
            Round::open(
                "r".into(),
                "a",
                "u",
                vec![NewOpinion::general("x")],
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
        assert!(matches!(
            r.add_opinion("u", &"x".repeat(1000), "t"),
            Err(RoundError::Full { chars: 8_000, .. })
        ));
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

    fn on(id: &str, name: &str, text: &str) -> NewOpinion {
        NewOpinion {
            text: text.into(),
            target: Some(Target {
                id: id.into(),
                name: name.into(),
            }),
        }
    }

    #[test]
    fn opinions_on_components_support_and_replies() {
        let mut r = open(10);
        // Several components at once, plus a general one.
        r.add_opinions(
            "u-b",
            vec![
                on("mod-web-list", "list", "清單加上篩選"),
                on("app-api", "todo-api", "API 回傳總數"),
                NewOpinion::general("整體字大一點"),
            ],
            "t1",
        )
        .unwrap();
        assert_eq!(r.opinions.len(), 4);
        assert_eq!(r.opinions[1].id, "o2");
        assert_eq!(r.opinions[2].target.as_ref().unwrap().name, "todo-api");
        assert_eq!(r.log.last().unwrap().args["count"], "3");

        // +1 and take it back; not on one's own.
        assert!(r.toggle_support("u-a", "o2", "t2").unwrap());
        assert!(r.toggle_support("u-c", "o2", "t2").unwrap());
        assert!(!r.toggle_support("u-c", "o2", "t3").unwrap());
        assert_eq!(r.opinions[1].supporters, ["u-a"]);
        assert!(matches!(
            r.toggle_support("u-b", "o2", "t"),
            Err(RoundError::Invalid(_))
        ));
        assert_eq!(
            r.toggle_support("u-a", "o99", "t"),
            Err(RoundError::NoSuchOpinion)
        );
        r.reply("u-a", "o3", "要含已完成的嗎？", "t4").unwrap();

        // Grouped for the AI: general first, then per component.
        let req = r.change_request();
        let general = req.find("【整個應用】").unwrap();
        let list = req.find("【元件：list】").unwrap();
        assert!(general < list, "{req}");
        assert!(
            req.contains("#2（u-b）清單加上篩選　〔1 人附議：u-a〕"),
            "{req}"
        );
        assert!(req.contains("回覆（u-a）要含已完成的嗎？"), "{req}");
        assert!(r.opinion_list().contains("（u-b）［list］清單加上篩選"));

        // Only authors confirm; support and replies go on while confirming.
        r.start_converging("t5");
        r.converged(plan(), "t6");
        let users: Vec<&str> = r.confirmers.iter().map(|c| c.user.as_str()).collect();
        assert_eq!(users, ["u-a", "u-b"]);
        assert!(r.toggle_support("u-c", "o1", "t7").unwrap());
        r.reply("u-c", "o1", "同意", "t7").unwrap();
        assert_eq!(r.state, State::Confirming);
        // Not after the round ends.
        r.cancel("u-a", "t8").unwrap();
        assert!(matches!(
            r.reply("u-c", "o1", "x", "t9"),
            Err(RoundError::WrongState(_))
        ));

        // Batch limits: all or nothing.
        let mut r = open(10);
        let many: Vec<NewOpinion> = (0..11)
            .map(|i| NewOpinion::general(&format!("x{i}")))
            .collect();
        assert!(matches!(
            r.add_opinions("u", many, "t"),
            Err(RoundError::Invalid(_))
        ));
        let bad = vec![NewOpinion::general("ok"), NewOpinion::general(" ")];
        assert!(r.add_opinions("u", bad, "t").is_err());
        assert_eq!(r.opinions.len(), 1, "nothing added");
    }

    #[test]
    fn text_limits_at_the_boundary() {
        let mut r = open(10);
        // 1000 characters is fine, 1001 is not; CJK and emoji count as one.
        r.add_opinion("u-b", &"字".repeat(1000), "t").unwrap();
        assert_eq!(
            r.add_opinion("u-b", &"😀".repeat(1001), "t"),
            Err(RoundError::TooLong {
                what: Text::Opinion,
                max: 1000
            })
        );
        // Surrounding spaces do not count.
        r.add_opinion("u-b", &format!("  {}  ", "x".repeat(1000)), "t")
            .unwrap();
        // Replies: 300.
        r.reply("u-c", "o1", &"回".repeat(300), "t").unwrap();
        assert_eq!(
            r.reply("u-c", "o1", &"回".repeat(301), "t"),
            Err(RoundError::TooLong {
                what: Text::Reply,
                max: 300
            })
        );
        assert!(matches!(
            r.reply("u-c", "o1", "  ", "t"),
            Err(RoundError::Invalid(_))
        ));
        // The total counts replies too: 4 + 1000 + 1000 + 300 used.
        assert_eq!(r.total_chars(), "加上到期日".chars().count() + 2300);
        let v = r.to_json_for(Some("u-b"));
        assert_eq!(v["used"]["chars"], r.total_chars());
        assert_eq!(v["used"]["opinions"], 3);
        assert_eq!(v["limits"]["replyChars"], 300);
        assert_eq!(v["limits"]["roundChars"], 8000);
    }

    #[test]
    fn replies_per_opinion_and_lower_limits() {
        let mut r = open(10);
        for i in 0..MAX_REPLIES {
            r.reply("u-b", "o1", &format!("r{i}"), "t").unwrap();
        }
        assert!(matches!(
            r.reply("u-b", "o1", "one more", "t"),
            Err(RoundError::Full { .. })
        ));

        let lower = Limits {
            opinion_chars: 10,
            reply_chars: 5,
            round_chars: 30,
            opinions: 3,
            replies: 1,
            batch: 2,
        };
        assert!(lower.within_maximum());
        assert!(!Limits {
            opinion_chars: 1001,
            ..Limits::default()
        }
        .within_maximum());
        assert!(!Limits {
            opinions: 0,
            ..Limits::default()
        }
        .within_maximum());
        let mut r = Round::open_with(
            lower,
            "r2".into(),
            "todo",
            "u-a",
            vec![NewOpinion::general("0123456789")],
            10,
            vec![Part::Web],
            T0,
            "t",
        )
        .unwrap();
        assert!(matches!(
            r.add_opinion("u-b", "01234567890", "t"),
            Err(RoundError::TooLong { max: 10, .. })
        ));
        assert!(matches!(
            r.add_opinions(
                "u-b",
                (0..3)
                    .map(|i| NewOpinion::general(&i.to_string()))
                    .collect(),
                "t"
            ),
            Err(RoundError::Invalid(_))
        ));
        r.add_opinion("u-b", "0123456789", "t").unwrap();
        r.add_opinion("u-b", "0123456789", "t").unwrap();
        assert_eq!(
            r.add_opinion("u-b", "x", "t"),
            Err(RoundError::Full {
                opinions: 3,
                replies: 1,
                chars: 30
            })
        );
    }

    #[test]
    fn stored_round_reads_back() {
        let mut r = open(0);
        r.add_opinions("u-b", vec![on("app-api", "todo-api", "API 回傳總數")], "t1")
            .unwrap();
        r.toggle_support("u-a", "o2", "t2").unwrap();
        r.reply("u-a", "o2", "好", "t3").unwrap();
        r.start_commit = "a".repeat(40);
        r.start_converging("t4");
        r.converged(plan(), "t5");
        r.recipe = Some(PlanRecipe {
            plan: crate::plan::AppPlan {
                app_name: None,
                summary: "s".into(),
                steps: vec!["x".into()],
                web_modules: vec![],
                api_modules: vec![],
                endpoints: vec![],
                notes: String::new(),
            },
            masked_prompt: "m".into(),
            model: "model-a-test".into(),
            label: Some("（u-a）加上到期日".into()),
            base_commit: "b".repeat(40),
        });
        let stored = r.to_stored();
        // The stored form keeps what the page does not see.
        assert_eq!(stored["lastOpinion"], 2);
        assert!(stored["recipe"].is_object());
        let page = r.to_json_for(Some("u-a"));
        for k in HIDDEN {
            assert!(page.get(k).is_none(), "{k} shown to the page");
        }
        let back = Round::from_stored(stored, Some("17".into()), Limits::default()).unwrap();
        assert_eq!(back.store_version.as_deref(), Some("17"));
        let mut expected = r.clone();
        expected.store_version = Some("17".into());
        assert_eq!(back, expected);

        // A lost counter never reuses an ID.
        let mut stored = r.to_stored();
        stored["lastOpinion"] = serde_json::json!(0);
        let mut back = Round::from_stored(stored, None, Limits::default()).unwrap();
        back.state = State::Collecting;
        back.add_opinion("u-c", "x", "t").unwrap();
        assert_eq!(back.opinions.last().unwrap().id, "o3");
    }
}
