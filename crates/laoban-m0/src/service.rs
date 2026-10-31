//! The M0 service behind `POST /api/plan`, `POST /api/apply` and
//! `GET /api/config`. Transport-independent: the HTTP layer only parses
//! JSON bodies and maps [`M0Error`] to status codes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use laoban_ai::{
    AiError, BudgetError, CompletionRequest, Dependencies, ModelSelection, Orchestrator,
    OrchestratorConfig, PlanError, PlanPolicy, Provider, ProviderError, RandomBoundary,
    RetryPolicy, ThreadSleeper, TokenBudget, UntrustedBlock, UtcMonthClock,
};
use laoban_mask::MaskerConfig;
use serde::Deserialize;
use serde_json::{json, Value};
use thiserror::Error;

use crate::codegen::{
    lost_edits, manual_edits, manual_edits_block, parse_generated, GeneratedFiles,
    CODEGEN_INSTRUCTIONS, EDIT_CODEGEN_INSTRUCTIONS, MANUAL_EDITS_INSTRUCTIONS,
};
use crate::config::M0Settings;
use crate::github::{CommitFile, GithubCommitter, GithubError};
use crate::lang::tr;
use crate::meta::{self, AppMeta, HistoryEntry, Layout, Pos, META_FILE};
use crate::plan::{
    architecture_with_layout, carry_layout, parse_app_plan, valid_app_name, EDIT_PLAN_INSTRUCTIONS,
    PLAN_INSTRUCTIONS, ROUND_PLAN_INSTRUCTIONS,
};
use crate::render::{infra_files, render, rollout_hashes, RenderInput};
use crate::rounds::{self, Limits, Part, PlanRecipe, Round, RoundError, RoundPlan, State, Text};
use crate::state::{MemoryStore, StateError, StateStore};
use crate::store::{BaseApp, PlanStore, StoreError, StoredPlan};
use crate::trf;
use crate::{time, InvalidOutput};

mod persist;
mod saved;

pub use persist::{valid_round_id, ARCHIVE_BRANCH, ARCHIVE_DIR, MAX_PAST_ROUNDS};
pub use saved::StateUsage;

/// Fleet objects created once by hand (design: Fleet).
pub const FLEET_WORKSPACE: &str = "fleet-default";
pub const FLEET_GITREPO: &str = "laoban-apps";

/// Longest accepted requirement, in characters.
pub const MAX_PROMPT_CHARS: usize = 4_000;
/// `max_tokens` for the plan call.
pub const PLAN_MAX_TOKENS: u32 = 4_096;
/// `max_tokens` for the code call. Four files of a small app fit well
/// within this; a truncated response is rejected, never committed.
pub const CODEGEN_MAX_TOKENS: u32 = 16_000;
/// Largest current code (four files together) sent along when an app is
/// changed. Keeps the request inside a 32k-token local model context
/// together with the plan and `CODEGEN_MAX_TOKENS` of output.
pub const MAX_EDIT_CODE_BYTES: usize = 40 * 1024;
/// Directory in the apps repository that Fleet deploys.
pub const APPS_DIR: &str = "apps";
/// Most apps listed.
pub const MAX_LISTED_APPS: usize = 100;
/// Most versions listed per app.
pub const MAX_VERSIONS: usize = 50;

/// There is one user in M0 and no identity is passed to the backend;
/// this fixed principal is what the orchestrator records.
const PRINCIPAL: &str = "laoban-m0";

const SYSTEM_PROMPT: &str = "You are laoban's application generator. laoban deploys small \
web applications to a Rancher-managed Kubernetes cluster from fixed templates.";

/// `POST /api/plan` body.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanRequestBody {
    pub prompt: String,
    #[serde(default)]
    pub app_name: Option<String>,
    /// Change this existing app instead of creating a new one.
    #[serde(default)]
    pub base_app: Option<String>,
    /// Answer at once with a job to read (`GET /api/plans/<jobId>`); the
    /// AI works in the background (see [`M0Service::start_job`]).
    #[serde(default)]
    pub background: bool,
}

/// `POST /api/apps/<name>/restore` body.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreRequest {
    /// Full SHA of the version's commit (from `GET .../versions`).
    pub commit: String,
}

/// `POST .../apps/<name>/round` body.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenRoundRequest {
    /// The first opinion (general). Either this or `items`, or both.
    #[serde(default)]
    pub opinion: String,
    /// First opinions, each optionally about one component.
    #[serde(default)]
    pub items: Vec<OpinionItem>,
    /// Minutes until the AI converges; 0 converges right away.
    pub deadline_minutes: u64,
    /// "web", "api" or "all" (default).
    #[serde(default)]
    pub scope: Option<String>,
}

/// `POST .../round/object` and `.../round/opinions/<id>/replies` body.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpinionRequest {
    pub text: String,
}

/// One opinion as sent: text and, optionally, the architecture node it is
/// about (the node id from the app's architecture).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpinionItem {
    pub text: String,
    #[serde(default)]
    pub target: Option<String>,
}

/// `POST .../round/opinions` body: one opinion (`text`, optional
/// `target`) or several (`items`, up to 10).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddOpinionsRequest {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub items: Vec<OpinionItem>,
}

/// `POST .../round/deadline` body: minutes from now; 0 converges now.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeadlineRequest {
    pub minutes: u64,
}

/// `POST .../round/drafts` body: all of the caller's drafts (replacing
/// the ones kept), and whether others may see them (default yes).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftsRequest {
    #[serde(default)]
    pub items: Vec<OpinionItem>,
    #[serde(default)]
    pub shared: Option<bool>,
}

/// `POST .../round/skip` body.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkipRequest {
    /// Whose confirmation the initiator skips.
    pub user: String,
}

/// `POST /api/apps/<name>/files` body.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveFilesRequest {
    /// Commit the edits started from (`GET .../files`).
    pub base_commit: String,
    /// `{"web": {...}, "api": {"handler.py": ...}}`, same shape as the AI's.
    pub files: Value,
    #[serde(default)]
    pub note: Option<String>,
}

/// `POST /api/apps/<name>/layout` body.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveLayoutRequest {
    pub base_commit: String,
    /// Node id to position.
    pub positions: Layout,
}

/// `POST /api/apply` body.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplyRequest {
    pub plan_id: String,
    /// As for [`PlanRequestBody::background`].
    #[serde(default)]
    pub background: bool,
}

/// What `POST .../plans/<id>/apply` may carry.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplyOptions {
    #[serde(default)]
    pub background: bool,
}

/// A request that runs the AI, done in the background (report 18, B8):
/// requests through the Kubernetes API server end after about 60 seconds.
struct Job {
    /// Only this user can read it ("" when there is no verified user).
    user: String,
    kind: &'static str,
    started: std::time::Instant,
    started_at: String,
    state: JobState,
}

enum JobState {
    Running,
    Done(Value),
    Failed {
        status: u16,
        code: &'static str,
        message: String,
    },
}

/// Jobs are kept this long after they start, then forgotten.
const JOB_TTL: std::time::Duration = std::time::Duration::from_secs(60 * 60);
const MAX_JOBS: usize = 200;

/// Errors with the codes and statuses of the design's error table.
/// `user_message` is for the page, in the backend's language (see
/// [`crate::lang`]); `Display` is for
/// tests and never contains secrets or user content.
#[derive(Debug, Error)]
pub enum M0Error {
    #[error("bad request: {0}")]
    BadRequest(&'static str),
    #[error("plan not found")]
    PlanNotFound,
    #[error("plan already applied")]
    AlreadyApplied,
    /// The stored copy of the plan could not be removed before applying,
    /// so applying could be repeated after a restart; nothing was changed.
    #[error("the stored plan could not be released for applying")]
    PlanStoreFailed,
    #[error("token budget exceeded")]
    BudgetExceeded,
    #[error("provider spend limit reached")]
    SpendLimit,
    #[error(transparent)]
    AiInvalidOutput(InvalidOutput),
    #[error("AI provider call failed: {0}")]
    AiUnavailable(&'static str),
    #[error(transparent)]
    Github(GithubError),
    #[error("internal: {0}")]
    Internal(String),
    /// A new app would overwrite an existing one. Carries the name for the
    /// user message only.
    #[error("app already exists")]
    AppExists(String),
    #[error("app not found")]
    AppNotFound,
    /// The app has no (valid) laoban.json, for example one made before it
    /// existed.
    #[error("app is not editable")]
    NotEditable,
    #[error("version not found")]
    VersionNotFound,
    /// A round of 意見收斂 is running on the app; changes go through it.
    #[error("a round is active")]
    RoundActive,
    /// A direct edit touches files the running round is about.
    #[error("files in the round's scope: {0}")]
    RoundScope(String),
    /// Applying a change met a file changed by hand since the plan.
    #[error("merge conflict in {0}")]
    MergeConflict(String),
    /// The AI's code drops hand edits made during the round.
    #[error("the AI's code drops hand edits in {0}")]
    ManualEditLost(String),
    #[error("round: {0:?}")]
    Round(RoundError),
    /// The request carried no verified user (it did not come through the
    /// Kubernetes API server).
    #[error("unauthenticated")]
    Unauthenticated,
    /// The app changed since the edits started.
    #[error("version conflict")]
    VersionConflict,
    /// Hand-edited files failed the checks AI output must pass.
    #[error("invalid files: {0}")]
    InvalidFiles(String),
    /// No such background job for this user (or it expired).
    #[error("job not found")]
    JobNotFound,
    /// Too many background jobs at once.
    #[error("busy")]
    Busy,
    /// Rounds cannot be stored: the `Round` CRD is not installed (or the
    /// stored rounds are still being read).
    #[error("rounds unavailable")]
    RoundsUnavailable,
    /// Writing a round to its resource failed.
    #[error(transparent)]
    State(StateError),
    /// No archived round with that ID for the app.
    #[error("past round not found")]
    PastRoundNotFound,
}

impl M0Error {
    pub fn code(&self) -> &'static str {
        match self {
            M0Error::BadRequest(_) => "bad_request",
            M0Error::PlanNotFound => "plan_not_found",
            M0Error::AlreadyApplied => "already_applied",
            M0Error::PlanStoreFailed => "plan_store_failed",
            M0Error::BudgetExceeded => "budget_exceeded",
            M0Error::SpendLimit => "spend_limit",
            M0Error::AiInvalidOutput(_) => "ai_invalid_output",
            M0Error::Github(_) => "github_failed",
            M0Error::AiUnavailable(_) | M0Error::Internal(_) => "internal",
            M0Error::AppExists(_) => "app_exists",
            M0Error::AppNotFound => "app_not_found",
            M0Error::NotEditable => "not_editable",
            M0Error::VersionNotFound => "version_not_found",
            M0Error::RoundActive => "round_active",
            M0Error::RoundScope(_) => "round_scope",
            M0Error::MergeConflict(_) | M0Error::ManualEditLost(_) => "merge_conflict",
            M0Error::Round(RoundError::Invalid(_)) => "bad_request",
            M0Error::Round(RoundError::TooLong { .. }) => "too_long",
            M0Error::Round(RoundError::Full { .. }) => "round_full",
            M0Error::Round(RoundError::WrongState(_)) => "round_state",
            M0Error::Round(RoundError::NotInitiator) => "not_initiator",
            M0Error::Round(RoundError::NotConfirmer) => "not_confirmer",
            M0Error::Round(RoundError::NoSuchOpinion) => "opinion_not_found",
            M0Error::Unauthenticated => "unauthenticated",
            M0Error::VersionConflict => "version_conflict",
            M0Error::InvalidFiles(_) => "invalid_files",
            M0Error::JobNotFound => "job_not_found",
            M0Error::Busy => "busy",
            M0Error::RoundsUnavailable => "rounds_unavailable",
            M0Error::State(StateError::Conflict) => "round_conflict",
            M0Error::State(StateError::TooLarge) => "round_full",
            M0Error::State(_) => "state_failed",
            M0Error::PastRoundNotFound => "round_not_found",
        }
    }

    /// Extra detail for the server log, for errors whose cause the user
    /// message hides (which GitHub step failed and how, why the AI call
    /// failed). These messages never contain the token, the API key or the
    /// user's prompt. For `ai_invalid_output` only the kind of problem is
    /// logged (not JSON, a key not allowed, cut off at max_tokens, …), never
    /// AI output: the check's wording up to the first `:` or `` ` ``, where
    /// parser messages and quoted values begin (report 25, N45).
    pub fn log_detail(&self) -> Option<String> {
        match self {
            M0Error::Github(_)
            | M0Error::AiUnavailable(_)
            | M0Error::Internal(_)
            | M0Error::State(_) => Some(self.to_string()),
            M0Error::AiInvalidOutput(InvalidOutput(why)) => {
                let kind = why.split([':', '`']).next().unwrap_or("").trim();
                let kind: String = kind.chars().take(120).collect();
                Some(if kind.is_empty() {
                    "unknown".into()
                } else {
                    kind
                })
            }
            _ => None,
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            M0Error::BadRequest(_) => 400,
            M0Error::PlanNotFound => 404,
            M0Error::AlreadyApplied => 409,
            M0Error::PlanStoreFailed => 503,
            M0Error::BudgetExceeded => 429,
            M0Error::SpendLimit => 402,
            M0Error::AiInvalidOutput(_) | M0Error::Github(_) => 502,
            M0Error::AiUnavailable(_) | M0Error::Internal(_) => 500,
            M0Error::AppExists(_) | M0Error::NotEditable => 409,
            M0Error::AppNotFound | M0Error::VersionNotFound => 404,
            M0Error::RoundActive
            | M0Error::RoundScope(_)
            | M0Error::MergeConflict(_)
            | M0Error::ManualEditLost(_)
            | M0Error::VersionConflict
            | M0Error::Round(RoundError::WrongState(_))
            | M0Error::Round(RoundError::NotConfirmer) => 409,
            M0Error::Round(RoundError::Invalid(_))
            | M0Error::Round(RoundError::TooLong { .. })
            | M0Error::Round(RoundError::Full { .. }) => 400,
            M0Error::Round(RoundError::NotInitiator) => 403,
            M0Error::Round(RoundError::NoSuchOpinion) => 404,
            M0Error::Unauthenticated => 401,
            M0Error::InvalidFiles(_) => 400,
            M0Error::JobNotFound => 404,
            M0Error::Busy => 503,
            M0Error::RoundsUnavailable => 503,
            M0Error::State(StateError::Conflict) => 409,
            M0Error::State(StateError::TooLarge) => 400,
            M0Error::State(_) => 503,
            M0Error::PastRoundNotFound => 404,
        }
    }

    pub fn user_message(&self) -> String {
        match self {
            M0Error::BadRequest(m) => (*m).to_string(),
            M0Error::PlanNotFound => tr(
                "This plan was not found; it may be over 1 hour old. Please generate the plan again.",
                "找不到這個計畫，可能已超過 1 小時，請重新產生計畫。",
            )
            .into(),
            M0Error::PlanStoreFailed => tr(
                "The plan could not be prepared for deploying; nothing was changed. Please try again in a moment.",
                "計畫暫時無法開始部署，沒有任何變更，請稍後再試。",
            )
            .into(),
            M0Error::AlreadyApplied => tr(
                "This plan has already been deployed (or is being deployed); it cannot be deployed twice.",
                "這個計畫已經部署過（或正在部署），不能重複部署。",
            )
            .into(),
            M0Error::BudgetExceeded => tr(
                "laoban's AI token limit for this month (UTC) is used up. Wait for next month (UTC) or ask an administrator to raise it.",
                "已達到 laoban 本月（UTC）的 AI token 使用上限，請等下個月（UTC），或請管理者調高上限。",
            )
            .into(),
            M0Error::SpendLimit => tr(
                "The Anthropic account reached its spend limit. Ask an administrator to check the Anthropic Console; the system does not retry by itself.",
                "Anthropic 帳號已達花費上限，請管理者到 Anthropic Console 檢查；系統不會自動重試。",
            )
            .into(),
            M0Error::AiInvalidOutput(_) => tr(
                "The AI's answer was not in the required format; nothing was written. Try again or say it another way.",
                "AI 回應的格式不符合要求，沒有寫入任何東西，請再試一次或換個說法。",
            )
            .into(),
            M0Error::AiUnavailable(m) => (*m).to_string(),
            M0Error::Github(_) => tr(
                "Writing to GitHub failed. Try again later; if it keeps failing, ask an administrator to check the token and repository settings.",
                "寫入 GitHub 失敗，請稍後再試；若持續失敗請管理者檢查 token 與 repo 設定。",
            )
            .into(),
            M0Error::Internal(_) => tr(
                "An internal error occurred in the backend.",
                "後端發生內部錯誤。",
            )
            .into(),
            M0Error::AppExists(app) => trf!(
                en: "An app named {app} already exists. Choose a different name, or change the existing app instead.",
                zh: "已經有名為 {app} 的應用。請換一個名稱，或改為修改現有的應用。"
            ),
            M0Error::AppNotFound => tr(
                "This app was not found; it may have been removed.",
                "找不到這個應用，可能已經被刪除。",
            )
            .into(),
            M0Error::VersionNotFound => {
                tr("This version was not found.", "找不到這個版本。").into()
            }
            M0Error::RoundActive => tr(
                "A round of opinions is running for this app. To change it, give an opinion on the app's detail page; until this round ends, the app cannot be restored and no other change plan can be made.",
                "這個應用正在進行一輪意見收斂，修改請到應用的詳細頁提意見；這一輪結束前不能還原或另外產生修改計畫。",
            )
            .into(),
            M0Error::RoundScope(files) => trf!(
                en: "This round of opinions is working on {files}; these files cannot be edited directly until the round ends. Other files can be edited as usual.",
                zh: "這一輪意見收斂正在處理 {files}，這一輪結束前不能直接修改這些檔案；其他檔案可以照常修改。"
            ),
            M0Error::MergeConflict(files) => trf!(
                en: "{files} was also edited directly during this round and conflicts with the AI's change; nothing was deployed. The initiator can converge again.",
                zh: "這一輪期間 {files} 也被直接修改過，和 AI 的修改衝突，沒有部署。請發起人重新收斂。"
            ),
            M0Error::ManualEditLost(files) => trf!(
                en: "The AI's change would overwrite the hand edits to {files} made during this round; nothing was deployed. The initiator can converge again, or undo the hand edit first if it is not needed.",
                zh: "AI 的修改會蓋掉這一輪期間對 {files} 的手動修改，沒有部署。請發起人重新收斂；如果手動修改不需要保留，請先把它改回去。"
            ),
            M0Error::Round(RoundError::Invalid(m)) => (*m).into(),
            M0Error::Round(RoundError::TooLong { what, max }) => match what {
                Text::Opinion => trf!(
                    en: "An opinion can be at most {max} characters. Please shorten it before sending.",
                    zh: "一則意見最多 {max} 字，請精簡後再送出。"
                ),
                Text::Reply => trf!(
                    en: "A reply can be at most {max} characters. Please shorten it before sending.",
                    zh: "一則回覆最多 {max} 字，請精簡後再送出。"
                ),
            },
            M0Error::Round(RoundError::Full {
                opinions,
                replies,
                chars,
            }) => trf!(
                en: "This round is full (at most {opinions} opinions, {replies} replies each, {chars} characters in all). Please converge it first, or wait for this round to end and start the next one.",
                zh: "這一輪的意見已滿（最多 {opinions} 則意見、每則 {replies} 則回覆，合計 {chars} 字），請先收斂，或等這一輪結束再開下一輪。"
            ),
            M0Error::Round(RoundError::WrongState(st)) => trf!(
                en: "This round cannot do that right now ({}). Please refresh the page.",
                zh: "這一輪目前不能這樣做（{}）。請重新整理頁面。",
                state_label(*st)
            ),
            M0Error::Round(RoundError::NotInitiator) => tr(
                "Only the person who started this round can do that.",
                "只有發起這一輪的人可以這樣做。",
            )
            .into(),
            M0Error::Round(RoundError::NoSuchOpinion) => tr(
                "This opinion was not found. Please refresh the page.",
                "找不到這則意見，請重新整理頁面。",
            )
            .into(),
            M0Error::Round(RoundError::NotConfirmer) => tr(
                "You do not need to confirm this round: you gave no opinion in it, or you have already answered.",
                "你不需要確認這一輪：你沒有在這一輪提意見，或已經回覆過。",
            )
            .into(),
            M0Error::Unauthenticated => tr(
                "Your identity could not be verified. Please use the laoban page in Rancher.",
                "無法確認你的身分：請從 Rancher 的 laoban 頁面操作。",
            )
            .into(),
            M0Error::VersionConflict => tr(
                "This app got a new version while you were editing. Reload it and make your change again.",
                "這個應用在你編輯期間有了新版本，請重新讀取後再改。",
            )
            .into(),
            M0Error::InvalidFiles(m) => trf!(
                en: "The code did not pass the checks: {m}",
                zh: "程式沒有通過檢查：{m}"
            ),
            M0Error::NotEditable => tr(
                "This app has no laoban edit record (it may have been made by an earlier version), so it cannot be edited. Please generate it again under a new name.",
                "這個應用沒有 laoban 的編輯紀錄（可能是較早版本產生的），無法編輯；請用新名稱重新產生。",
            )
            .into(),
            M0Error::JobNotFound => tr(
                "The progress of this job was not found; it may be over 1 hour old or the backend restarted. Check the result in the app list.",
                "找不到這項工作的進度，可能已超過 1 小時或後端已重新啟動。請到應用清單確認結果。",
            )
            .into(),
            M0Error::Busy => tr(
                "The backend has too much work right now. Try again later.",
                "後端目前有太多工作在進行，請稍後再試。",
            )
            .into(),
            M0Error::RoundsUnavailable => tr(
                "Rounds of opinions are not available right now: the cluster has no laoban Round resource. Ask an administrator to upgrade the laoban-crd chart first.",
                "意見收斂暫時無法使用：叢集裡沒有 laoban 的 Round 資源。請管理者先升級 laoban-crd chart。",
            )
            .into(),
            M0Error::State(StateError::Conflict) => tr(
                "Someone else just changed this round. Please refresh the page and try again.",
                "這一輪剛被別人改過，請重新整理頁面再試一次。",
            )
            .into(),
            M0Error::State(StateError::TooLarge) => tr(
                "This round's data is over the storage limit; nothing was written. Please shorten the opinions.",
                "這一輪的資料超過儲存上限，沒有寫入。請精簡意見。",
            )
            .into(),
            M0Error::State(_) => tr(
                "Saving the round's data failed; nothing was written. Try again later; if it keeps failing, ask an administrator to check the backend's permissions.",
                "儲存意見收斂的資料失敗，沒有寫入，請稍後再試；若持續失敗請管理者檢查後端的權限。",
            )
            .into(),
            M0Error::PastRoundNotFound => {
                tr("This round's record was not found.", "找不到這一輪的紀錄。").into()
            }
        }
    }
}

impl From<AiError> for M0Error {
    fn from(e: AiError) -> Self {
        match e {
            AiError::Budget(BudgetError::Exhausted { .. }) => M0Error::BudgetExceeded,
            AiError::Provider(p) => match p {
                ProviderError::SpendLimitReached { .. } => M0Error::SpendLimit,
                ProviderError::RateLimited { .. } | ProviderError::Overloaded => {
                    M0Error::AiUnavailable(tr(
                        "The AI service is busy right now. Try again later.",
                        "AI 服務目前忙碌中，請稍後再試。",
                    ))
                }
                ProviderError::Authentication { .. } => {
                    M0Error::AiUnavailable(tr(
                        "Authentication with the AI service failed. Ask an administrator to check the API key setting.",
                        "AI 服務認證失敗，請管理者檢查 API 金鑰設定。",
                    ))
                }
                ProviderError::InvalidRequest { .. } | ProviderError::Other { .. } => {
                    M0Error::AiUnavailable(tr(
                        "Calling the AI service failed. Try again later.",
                        "呼叫 AI 服務失敗，請稍後再試。",
                    ))
                }
            },
            AiError::Plan(PlanError::Truncated) => {
                M0Error::AiInvalidOutput(InvalidOutput("response truncated (max_tokens)".into()))
            }
            AiError::Plan(PlanError::Refused) => {
                M0Error::AiInvalidOutput(InvalidOutput("model refused".into()))
            }
            AiError::Plan(other) => M0Error::AiInvalidOutput(InvalidOutput(other.to_string())),
            AiError::Mask(_) | AiError::Model(_) | AiError::Prompt(_) => {
                M0Error::Internal(e.to_string())
            }
        }
    }
}

pub struct M0Service {
    settings: M0Settings,
    orchestrator: Orchestrator,
    budget: Arc<TokenBudget>,
    github: GithubCommitter,
    store: Mutex<PlanStore>,
    /// The latest round of 意見收斂 per app (active or finished), as
    /// written to its `Round` resource.
    rounds: Mutex<HashMap<String, Round>>,
    /// Where rounds are stored (Kubernetes in the server, memory in tests).
    state: Arc<dyn StateStore>,
    /// The stored rounds have been read and can be written.
    rounds_ready: AtomicBool,
    /// The stored rounds were read once (at startup); later reads keep
    /// what is in memory.
    restored_once: AtomicBool,
    round_limits: Limits,
    /// Replaced rounds waiting to be archived and deleted.
    archive_queue: Mutex<Vec<Round>>,
    /// Round ID → (failed archive attempts, next attempt in unix seconds).
    archive_retry: Mutex<HashMap<String, (u32, u64)>>,
    /// Set by [`M0Service::attach`] in the server: the AI work of a round
    /// (converging, writing the code and publishing) then runs on its own
    /// thread and the request returns at once. Requests through the
    /// Kubernetes API server are cut off after about 60 seconds (report 18,
    /// B8), and publishing can take longer. Without it (tests) the work
    /// runs in the request.
    me: std::sync::OnceLock<std::sync::Weak<M0Service>>,
    /// Background plan / apply requests, by job ID.
    jobs: Mutex<HashMap<String, Job>>,
    /// One lock per app: its versions are written one at a time, each
    /// checked against the version the previous one wrote (report 20, B11:
    /// two saves at once made the second fail as a GitHub problem).
    app_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// All apps share one branch: commits go one at a time, each on the
    /// head the previous one left, so two apps saving at once do not make
    /// one of them a version conflict. Taken after the app's lock.
    commit_lock: Mutex<()>,
    /// The stored plans were read back (see `saved`).
    plans_restored: AtomicBool,
    /// Unix seconds of the last sweep of expired stored plans.
    plans_swept_at: AtomicU64,
    /// The Plan resource is missing (logged once).
    plans_not_installed: AtomicBool,
    /// Reading the stored plans failed (logged once).
    plans_read_failed: AtomicBool,
    /// Plan ID → (made, unix seconds; a copy is in the state store).
    plan_records: Mutex<HashMap<String, (u64, bool)>>,
}

impl M0Service {
    pub fn new(
        settings: M0Settings,
        provider: Box<dyn Provider>,
        github: GithubCommitter,
    ) -> Result<Self, M0Error> {
        let budget = TokenBudget::new(settings.token_cap, UtcMonthClock);
        let orchestrator = Orchestrator::new(
            OrchestratorConfig {
                masker: MaskerConfig::default(),
                models: ModelSelection::Forced {
                    model: settings.model.clone(),
                },
                retry: RetryPolicy::default(),
                plan_policy: PlanPolicy::default(),
                system_prompt: SYSTEM_PROMPT.into(),
                max_tokens: PLAN_MAX_TOKENS,
            },
            Dependencies {
                provider,
                budget: Arc::clone(&budget),
                audit: Box::new(NoAudit),
                sleeper: Box::new(ThreadSleeper),
                boundary: Box::new(RandomBoundary::default()),
            },
        )
        .map_err(|e| M0Error::Internal(e.to_string()))?;
        Ok(Self {
            settings,
            orchestrator,
            budget,
            github,
            store: Mutex::new(PlanStore::default()),
            rounds: Mutex::new(HashMap::new()),
            state: Arc::new(MemoryStore::new()),
            rounds_ready: AtomicBool::new(true),
            restored_once: AtomicBool::new(false),
            round_limits: Limits::default(),
            archive_queue: Mutex::new(Vec::new()),
            archive_retry: Mutex::new(HashMap::new()),
            me: std::sync::OnceLock::new(),
            jobs: Mutex::new(HashMap::new()),
            app_locks: Mutex::new(HashMap::new()),
            commit_lock: Mutex::new(()),
            plans_restored: AtomicBool::new(false),
            plans_swept_at: AtomicU64::new(0),
            plans_not_installed: AtomicBool::new(false),
            plans_read_failed: AtomicBool::new(false),
            plan_records: Mutex::new(HashMap::new()),
        })
    }

    /// Keep rounds in `store` instead of memory. Call before serving; the
    /// stored rounds are read by [`M0Service::restore_rounds`] (the round
    /// timer does it first).
    /// Plans waiting for confirmation and the month's token usage are kept
    /// there too (v0.1.19).
    pub fn use_state_store(&mut self, store: Arc<dyn StateStore>) {
        self.budget
            .use_store(Arc::new(StateUsage::new(Arc::clone(&store))));
        self.state = store;
        self.rounds_ready
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Lower the size limits of rounds (see [`Limits`]). Values above the
    /// maximum are refused when the configuration is read.
    pub fn set_round_limits(&mut self, limits: Limits) {
        self.round_limits = limits;
    }

    /// Start `work` as a job and answer with it: still running when the
    /// service is attached, already finished otherwise (tests).
    pub fn start_job<F>(
        &self,
        kind: &'static str,
        user: Option<&str>,
        work: F,
    ) -> Result<Value, M0Error>
    where
        F: FnOnce(&M0Service) -> Result<Value, M0Error> + Send + 'static,
    {
        let now = SystemTime::now();
        let id = time::job_id(now);
        let user = user.unwrap_or_default().to_string();
        {
            let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            jobs.retain(|_, j| j.started.elapsed() < JOB_TTL);
            if jobs
                .values()
                .filter(|j| matches!(j.state, JobState::Running))
                .count()
                >= MAX_JOBS
                || jobs.len() >= MAX_JOBS * 5
            {
                return Err(M0Error::Busy);
            }
            jobs.insert(
                id.clone(),
                Job {
                    user: user.clone(),
                    kind,
                    started: std::time::Instant::now(),
                    started_at: time::rfc3339(now),
                    state: JobState::Running,
                },
            );
        }
        let job_id = id.clone();
        self.in_background("laoban-job", move |s| {
            let result = work(s);
            s.finish_job(&job_id, result);
        });
        self.job(&id, Some(&user))
    }

    fn finish_job(&self, id: &str, result: Result<Value, M0Error>) {
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(j) = jobs.get_mut(id) {
            j.state = match result {
                Ok(v) => JobState::Done(v),
                Err(e) => JobState::Failed {
                    status: {
                        log_failure("job", id, &e);
                        e.status()
                    },
                    code: e.code(),
                    message: e.user_message(),
                },
            };
        }
    }

    /// `GET /api/plans/<jobId>`: a job's state, for the user who started
    /// it. Others (and unknown or expired IDs) get "not found".
    pub fn job(&self, id: &str, user: Option<&str>) -> Result<Value, M0Error> {
        let jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let j = jobs
            .get(id)
            .filter(|j| j.user == user.unwrap_or_default() && j.started.elapsed() < JOB_TTL)
            .ok_or(M0Error::JobNotFound)?;
        let mut v = json!({
            "jobId": id,
            "kind": j.kind,
            "startedAt": j.started_at,
            "elapsedSeconds": j.started.elapsed().as_secs(),
        });
        match &j.state {
            JobState::Running => v["state"] = json!("running"),
            JobState::Done(result) => {
                v["state"] = json!("done");
                v["result"] = result.clone();
            }
            JobState::Failed {
                status,
                code,
                message,
            } => {
                v["state"] = json!("failed");
                v["error"] = json!({ "status": status, "code": code, "message": message });
            }
        }
        Ok(v)
    }

    /// `POST /api/plan` with `background`: the plan as a job.
    /// What can be refused at once is refused before the job starts, so
    /// the caller gets the error (for example `409 app_exists`) instead of
    /// a job that fails (report 23, N29).
    pub fn plan_job(&self, body: &PlanRequestBody, user: Option<&str>) -> Result<Value, M0Error> {
        match body
            .base_app
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
        {
            Some(app) => self.check_no_round(app)?,
            None => {
                if let Some(n) = body
                    .app_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|n| valid_app_name(n))
                {
                    if self.app_names()?.iter().any(|e| e == n) {
                        return Err(M0Error::AppExists(n.to_string()));
                    }
                }
            }
        }
        let body = body.clone();
        let owner = user.map(str::to_string);
        self.start_job("plan", user, move |s| s.plan(&body, owner.as_deref()))
    }

    /// `POST /api/apply` (or `.../plans/<id>/apply`) with `background`.
    pub fn apply_job(&self, body: &ApplyRequest, user: Option<&str>) -> Result<Value, M0Error> {
        // Answer now what the job would fail with: an expired or unknown
        // plan was accepted with 200 and failed later in the background,
        // leaving the page on "Plan ready" (report 24, N30).
        {
            let id = body.plan_id.trim();
            let mut store = self.lock_store();
            if !store.is_pending(id, Instant::now()) {
                return Err(if store.contains(id) {
                    M0Error::AlreadyApplied
                } else {
                    M0Error::PlanNotFound
                });
            }
        }
        let body = body.clone();
        let owner = user.map(str::to_string);
        self.start_job("apply", user, move |s| s.apply(&body, owner.as_deref()))
    }

    /// Run a round's AI work in the background from now on (see `me`).
    pub fn attach(self: &Arc<Self>) {
        let _ = self.me.set(Arc::downgrade(self));
    }

    /// `job` on its own thread when attached, else right here. Returns what
    /// `job` returned when it ran here.
    fn in_background<T, F>(&self, name: &str, job: F) -> Option<T>
    where
        F: FnOnce(&M0Service) -> T + Send + 'static,
        T: Send + 'static,
    {
        if let Some(me) = self.me.get().and_then(std::sync::Weak::upgrade) {
            // The job stays reachable here, so it is not lost if no thread
            // can be started: then it runs in this request after all,
            // rather than leaving the round stuck.
            let slot = Arc::new(Mutex::new(Some(job)));
            let theirs = Arc::clone(&slot);
            let spawned = std::thread::Builder::new()
                .name(name.into())
                .spawn(move || {
                    let job = theirs.lock().unwrap_or_else(|e| e.into_inner()).take();
                    if let Some(job) = job {
                        job(&me);
                    }
                });
            if spawned.is_ok() {
                return None;
            }
            let job = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
            return job.map(|job| job(self));
        }
        Some(job(self))
    }

    /// The lock for writing versions of `app` (see `app_locks`).
    fn app_lock(&self, app: &str) -> Arc<Mutex<()>> {
        let mut locks = self.app_locks.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(locks.entry(app.to_string()).or_default())
    }

    pub fn settings(&self) -> &M0Settings {
        &self.settings
    }

    /// `GET /api/config`. No secrets.
    pub fn config_json(&self) -> Value {
        json!({
            "model": self.settings.model,
            "targetNamespace": self.settings.target_namespace,
            "repo": self.settings.repo(),
            "tokenBudget": { "cap": self.budget.cap(), "used": self.budget.used() },
            "aiTimeoutSeconds": self.settings.ai_timeout_secs,
            "roundLimits": self.round_limits,
            "roundsAvailable": self.rounds_ready(),
        })
    }

    fn lock_store(&self) -> std::sync::MutexGuard<'_, PlanStore> {
        // A panic while holding the lock cannot leave the map inconsistent
        // in a way that matters here, so recover from poisoning.
        self.store.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// `POST /api/plan`. Changing an existing app this way is refused
    /// while a round of 意見收斂 runs on it.
    pub fn plan(&self, body: &PlanRequestBody, user: Option<&str>) -> Result<Value, M0Error> {
        if let Some(app) = body
            .base_app
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
        {
            self.check_no_round(app)?;
        }
        self.plan_with(body, user, None, MAX_PROMPT_CHARS)
    }

    /// Make a plan. `extra` adds instructions (a round's convergence);
    /// `max_chars` bounds the request.
    fn plan_with(
        &self,
        body: &PlanRequestBody,
        user: Option<&str>,
        round: Option<RoundInput>,
        max_chars: usize,
    ) -> Result<Value, M0Error> {
        let prompt = body.prompt.trim();
        if prompt.is_empty() {
            return Err(M0Error::BadRequest(tr(
                "Please enter a request.",
                "請輸入需求。",
            )));
        }
        if prompt.chars().count() > max_chars {
            return Err(M0Error::BadRequest(tr(
                "The request is too long. Please shorten it to 4000 characters or fewer.",
                "需求太長，請縮短到 4000 字以內。",
            )));
        }
        let requested_name = match body.app_name.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(n) if valid_app_name(n) => Some(n.to_string()),
            Some(_) => return Err(M0Error::BadRequest(
                tr("An app name can only use lowercase letters, digits and hyphens, must start with a letter, and must be 2 to 31 characters long.", "應用名稱只能用小寫英文字母、數字和連字號，以英文字母開頭，長度 2 到 31 個字元。"),
            )),
        };

        let base_name = match body.base_app.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(n) if valid_app_name(n) => Some(n.to_string()),
            Some(_) => {
                return Err(M0Error::BadRequest(tr(
                    "The name of the app to edit is not valid.",
                    "要編輯的應用名稱不正確。",
                )))
            }
        };
        if let (Some(base), Some(req)) = (&base_name, &requested_name) {
            if base != req {
                return Err(M0Error::BadRequest(tr(
                    "The name cannot be changed when editing an existing app.",
                    "編輯現有應用時不能改名稱。",
                )));
            }
        }

        let masked = self.orchestrator.mask_for_ai(prompt);
        let mut parts: Vec<Part> = Vec::new();
        let (plan, completion, app, base) = match base_name {
            Some(base_app) => {
                // A round's plan is made from a known commit, so it can be
                // made again from the same files after a restart.
                let current = match round.as_ref().and_then(|r| r.base_commit.as_deref()) {
                    Some(commit) => self.load_version(&base_app, commit)?,
                    None => self.load_app(&base_app)?,
                };
                require_user(user)?;
                if current.files.total_bytes() > MAX_EDIT_CODE_BYTES {
                    return Err(M0Error::BadRequest(tr(
                        "This app's code is too large for the trial version to edit with AI.",
                        "這個應用的程式太大，試吃版無法用 AI 編輯。",
                    )));
                }
                // Without the last change's notes: they talk about earlier
                // opinions, and the AI would repeat them for this change.
                let mut current_json = current.plan.to_codegen_json(&base_app);
                if let Some(o) = current_json.as_object_mut() {
                    o.remove("notes");
                }
                let current_plan = serde_json::to_string_pretty(&current_json)
                    .map_err(|e| M0Error::Internal(e.to_string()))?;
                let completion = self.orchestrator.complete(&CompletionRequest {
                    principal: PRINCIPAL.into(),
                    requested_model: None,
                    instructions: match round {
                        Some(_) => format!(
                            "{PLAN_INSTRUCTIONS}\n{EDIT_PLAN_INSTRUCTIONS}\n{ROUND_PLAN_INSTRUCTIONS}"
                        ),
                        None => format!("{PLAN_INSTRUCTIONS}\n{EDIT_PLAN_INSTRUCTIONS}"),
                    },
                    blocks: vec![
                        UntrustedBlock {
                            kind: "current_plan",
                            label: None,
                            content: current_plan,
                        },
                        UntrustedBlock {
                            kind: "change_request",
                            label: None,
                            content: masked.text.clone(),
                        },
                    ],
                    max_tokens: Some(PLAN_MAX_TOKENS),
                })?;
                let plan = parse_app_plan(&completion.content).map_err(M0Error::AiInvalidOutput)?;
                // Positions follow renamed modules (shown in the plan and
                // published on apply).
                let layout = carry_layout(&current.plan, &plan, &base_app, &current.layout);
                parts = changed_parts(&current.plan, &plan);
                let base = BaseApp {
                    files: current.files,
                    history: current.history,
                    layout,
                };
                (plan, completion, base_app, Some(base))
            }
            None => {
                let existing = self.app_names()?;
                if let Some(n) = &requested_name {
                    if existing.contains(n) {
                        return Err(M0Error::AppExists(n.clone()));
                    }
                }
                let completion = self.orchestrator.complete(&CompletionRequest {
                    principal: PRINCIPAL.into(),
                    requested_model: None,
                    instructions: PLAN_INSTRUCTIONS.into(),
                    blocks: vec![UntrustedBlock {
                        kind: "user_requirement",
                        label: None,
                        content: masked.text.clone(),
                    }],
                    max_tokens: Some(PLAN_MAX_TOKENS),
                })?;
                let mut plan =
                    parse_app_plan(&completion.content).map_err(M0Error::AiInvalidOutput)?;
                let app = match requested_name {
                    Some(n) => n,
                    None => match plan.app_name.as_deref() {
                        Some(n) if valid_app_name(n) => unused_name(n, &existing)
                            .ok_or_else(|| M0Error::AppExists(n.to_string()))?,
                        _ => {
                            return Err(M0Error::AiInvalidOutput(InvalidOutput(
                                "appName missing or invalid".into(),
                            )))
                        }
                    },
                };
                // The plan names the app it is for: the name the user gave
                // (or the free one chosen), not the AI's suggestion (report
                // 24, N37: plan.appName water-tracker, app t24-water).
                plan.app_name = Some(app.clone());
                (plan, completion, app, None)
            }
        };
        let empty = Layout::new();
        let arch =
            architecture_with_layout(&plan, &app, base.as_ref().map_or(&empty, |b| &b.layout));

        let now = SystemTime::now();
        let mut store = self.lock_store();
        let mut id = time::plan_id(now);
        while store.contains(&id) {
            id = time::plan_id(now);
        }
        let response = json!({
            "planId": id,
            "mode": if base.is_some() { "edit" } else { "create" },
            "appName": app,
            "summary": plan.summary,
            "steps": plan.steps,
            "architecture": arch,
            "maskedFindings": masked.findings.len(),
            "changedParts": parts,
            "notes": plan.notes,
            "usage": {
                "inputTokens": completion.input_tokens,
                "outputTokens": completion.output_tokens,
            },
            // Shown with the plan, so nobody mistakes a local model's work
            // for another model's.
            "model": completion.model,
        });
        let stored = StoredPlan {
            id,
            app,
            plan,
            masked_prompt: masked.text,
            model: completion.model,
            base,
            label: round
                .as_ref()
                .map(|r| self.orchestrator.mask_for_ai(&r.label).text),
            round_start: round.and_then(|r| r.start),
        };
        store.insert(stored.clone(), Instant::now());
        drop(store);
        // Kept across a restart (a round's plan is made again instead).
        self.save_plan(&stored);
        Ok(response)
    }

    /// The app's own address when `LAOBAN_APP_URL` is set, otherwise the
    /// Rancher service proxy path (same origin as Rancher).
    fn app_url(&self, app: &str) -> String {
        if let Some(u) = &self.settings.app_url {
            return u.url_for(app);
        }
        format!(
            "/k8s/clusters/{}/api/v1/namespaces/{}/services/http:{app}-web:8080/proxy/",
            self.settings.cluster_id, self.settings.target_namespace
        )
    }

    /// Names of the app directories in the repository.
    fn app_names(&self) -> Result<Vec<String>, M0Error> {
        self.github.list_dirs(APPS_DIR).map_err(M0Error::Github)
    }

    /// Read an app's `laoban.json`.
    fn load_app(&self, app: &str) -> Result<AppMeta, M0Error> {
        let path = format!("{APPS_DIR}/{app}/{META_FILE}");
        match self.github.read_file(&path).map_err(M0Error::Github)? {
            Some(text) => meta::parse(app, &text).map_err(|_| M0Error::NotEditable),
            None if self.app_names()?.iter().any(|n| n == app) => Err(M0Error::NotEditable),
            None => Err(M0Error::AppNotFound),
        }
    }

    fn app_links(&self, app: &str) -> Value {
        json!({
            "appUrl": self.app_url(app),
            "path": format!("{APPS_DIR}/{app}"),
            "fleet": {
                "workspace": FLEET_WORKSPACE,
                "gitRepo": FLEET_GITREPO,
                "bundlePrefix": format!("{FLEET_GITREPO}-{APPS_DIR}-{app}"),
            },
        })
    }

    /// `GET /api/apps`: every app directory, newest change first. Apps
    /// without a readable `laoban.json` are listed as not editable.
    pub fn list_apps(&self) -> Result<Value, M0Error> {
        let names = self.app_names()?;
        let mut apps: Vec<(String, Value)> = Vec::new();
        for name in names
            .iter()
            .filter(|n| valid_app_name(n))
            .take(MAX_LISTED_APPS)
        {
            let path = format!("{APPS_DIR}/{name}/{META_FILE}");
            let meta = match self.github.read_file(&path).map_err(M0Error::Github)? {
                Some(text) => meta::parse(name, &text).ok(),
                None => None,
            };
            let mut item = self.app_links(name);
            item["appName"] = json!(name);
            item["round"] = self
                .rounds()
                .get(name.as_str())
                .filter(|r| r.state.active())
                .map(|r| json!(r.state))
                .unwrap_or(Value::Null);
            match &meta {
                Some(m) => {
                    item["editable"] = json!(true);
                    item["summary"] = json!(m.plan.summary);
                    item["updatedAt"] = json!(m.updated_at());
                    item["revisions"] = json!(m.history.len());
                }
                None => {
                    item["editable"] = json!(false);
                    item["summary"] = json!("");
                    item["updatedAt"] = json!("");
                    item["revisions"] = json!(0);
                }
            }
            let key = meta
                .as_ref()
                .map(|m| m.updated_at().to_string())
                .unwrap_or_default();
            apps.push((key, item));
        }
        // Newest first; apps without a date (not editable) last, by name.
        apps.sort_by(|a, b| b.0.cmp(&a.0));
        Ok(json!({ "apps": apps.into_iter().map(|(_, v)| v).collect::<Vec<_>>() }))
    }

    /// `GET /api/apps/<name>`: what the page needs to show and change an app.
    pub fn get_app(&self, app: &str, user: Option<&str>) -> Result<Value, M0Error> {
        if !valid_app_name(app) {
            return Err(M0Error::BadRequest(tr(
                "The app name is not valid.",
                "應用名稱不正確。",
            )));
        }
        let m = self.load_app(app)?;
        let mut v = self.app_links(app);
        v["appName"] = json!(app);
        v["summary"] = json!(m.plan.summary);
        v["steps"] = json!(m.plan.steps);
        v["architecture"] = architecture_with_layout(&m.plan, app, &m.layout);
        v["fileDigests"] = json!(crate::codegen::file_digests(&m.files));
        v["round"] = self.round_json(app, user);
        v["updatedAt"] = json!(m.updated_at());
        v["history"] = json!(m.history.iter().rev().collect::<Vec<_>>());
        Ok(v)
    }

    /// `POST /api/apply`.
    pub fn apply(&self, body: &ApplyRequest, user: Option<&str>) -> Result<Value, M0Error> {
        self.apply_plan(body.plan_id.trim(), user, false)
    }

    /// Apply a stored plan. A change to an existing app outside a round is
    /// refused while a round runs on it.
    fn apply_plan(&self, id: &str, user: Option<&str>, from_round: bool) -> Result<Value, M0Error> {
        let stored = self
            .lock_store()
            .begin_apply(id, Instant::now())
            .map_err(|e| match e {
                StoreError::NotFound => M0Error::PlanNotFound,
                StoreError::AlreadyApplied => M0Error::AlreadyApplied,
            })?;
        // Gone from storage while it is applied: after a restart in the
        // middle it cannot be applied a second time. If its stored copy
        // cannot be removed, do not apply.
        if !from_round && !self.forget_plan(&stored) {
            self.lock_store().finish_apply(id, false);
            return Err(M0Error::PlanStoreFailed);
        }
        let result = if stored.base.is_some() && !from_round {
            self.check_no_round(&stored.app)
                .and_then(|()| self.apply_inner(&stored, user))
        } else {
            self.apply_inner(&stored, user)
        };
        if !from_round {
            if result.is_ok() {
                self.plan_applied(&stored);
            } else {
                // It can be confirmed again, so keep it again. Written while
                // the plan is still marked as being applied, so a second
                // confirm cannot start before the copy exists.
                self.save_plan(&stored);
            }
        }
        self.lock_store().finish_apply(id, result.is_ok());
        result
    }

    fn apply_inner(&self, stored: &StoredPlan, user: Option<&str>) -> Result<Value, M0Error> {
        if stored.base.is_some() {
            require_user(user)?;
        }
        let plan_json = serde_json::to_string_pretty(&stored.plan.to_codegen_json(&stored.app))
            .map_err(|e| M0Error::Internal(e.to_string()))?;
        let mut blocks = vec![UntrustedBlock {
            kind: "approved_plan",
            label: None,
            content: plan_json,
        }];
        let mut instructions = CODEGEN_INSTRUCTIONS.to_string();
        // Hand edits made during a round, before its plan was made.
        let edits = match (&stored.base, &stored.round_start) {
            (Some(base), Some(start)) => manual_edits(start, &base.files),
            _ => Vec::new(),
        };
        if let Some(base) = &stored.base {
            blocks.push(UntrustedBlock {
                kind: "current_code",
                label: None,
                content: serde_json::to_string_pretty(&base.files.to_json())
                    .map_err(|e| M0Error::Internal(e.to_string()))?,
            });
            instructions.push('\n');
            instructions.push_str(EDIT_CODEGEN_INSTRUCTIONS);
            if !edits.is_empty() {
                blocks.push(UntrustedBlock {
                    kind: "manual_edits",
                    label: None,
                    content: manual_edits_block(&edits),
                });
                instructions.push('\n');
                instructions.push_str(MANUAL_EDITS_INSTRUCTIONS);
            }
        }
        let completion = self.orchestrator.complete(&CompletionRequest {
            principal: PRINCIPAL.into(),
            requested_model: None,
            instructions,
            blocks,
            max_tokens: Some(CODEGEN_MAX_TOKENS),
        })?;
        let generated = parse_generated(&completion.content).map_err(M0Error::AiInvalidOutput)?;
        // From here on the app is read and written under its lock (not
        // during the AI call above, which can take minutes).
        let lock = self.app_lock(&stored.app);
        let _one_at_a_time = lock.lock().unwrap_or_else(|e| e.into_inner());
        // A new app must still not exist (another plan may have been
        // applied under the same name since it was made).
        if stored.base.is_none() && self.app_names()?.contains(&stored.app) {
            return Err(M0Error::AppExists(stored.app.clone()));
        }
        if let Some(base) = &stored.base {
            let lost = lost_edits(&edits, &base.files, &generated);
            if !lost.is_empty() {
                return Err(M0Error::ManualEditLost(lost.join("、")));
            }
        }

        let app = stored.app.as_str();
        // A change is merged into the app as it is now, file by file: files
        // edited by hand since the plan are kept unless the AI changed them
        // too (then nothing is published). History and layout are the
        // current ones.
        let (files, history, layout) = match &stored.base {
            Some(base) => {
                let current = self.load_app(app)?;
                let files = crate::codegen::merge_files(&base.files, &generated, &current.files)
                    .map_err(|names| M0Error::MergeConflict(names.join("、")))?;
                let layout = carry_layout(&current.plan, &stored.plan, app, &current.layout);
                (files, current.history, layout)
            }
            None => (generated, Vec::new(), Layout::new()),
        };
        let masked_summary = self.orchestrator.mask_for_ai(&stored.plan.summary).text;
        let masked_prompt = match &stored.label {
            Some(label) => label.clone(),
            None => self.orchestrator.mask_for_ai(&stored.masked_prompt).text,
        };
        let (kind, message) = if stored.base.is_some() {
            (
                "edit",
                commit_message(
                    app,
                    &trf!(en: "Change: {masked_prompt}", zh: "修改：{masked_prompt}"),
                ),
            )
        } else {
            ("create", commit_message(app, &masked_summary))
        };
        self.publish(Publish {
            app,
            plan: &stored.plan,
            files: &files,
            history,
            layout,
            kind,
            request: masked_prompt,
            summary: masked_summary,
            model: completion.model,
            message,
            by: user.unwrap_or_default().to_string(),
        })
    }

    /// Render, commit and describe one new version of `app`.
    fn publish(&self, p: Publish<'_>) -> Result<Value, M0Error> {
        let app = p.app;
        let generated_at = time::rfc3339(SystemTime::now());
        let mut history = p.history;
        history.push(HistoryEntry {
            at: generated_at.clone(),
            kind: p.kind.into(),
            request: p.request.clone(),
            summary: p.summary.clone(),
            model: p.model.clone(),
            by: p.by.clone(),
        });
        let meta_json = meta::to_json_with_layout(app, p.plan, p.files, &history, &p.layout);
        let commit_files = render(&RenderInput {
            app,
            namespace: &self.settings.target_namespace,
            files: p.files,
            masked_prompt: &p.request,
            masked_summary: &p.summary,
            model: &p.model,
            generated_at: &generated_at,
            meta_json: &meta_json,
            app_url: self.settings.app_url.as_ref(),
            network_policy: self.settings.app_network_policy,
        });
        let commit = self.commit_lock.lock().unwrap_or_else(|e| e.into_inner());
        let sha = self
            .github
            .commit(&commit_files, &p.message)
            .map_err(|e| match e {
                // The branch moved since it was read: someone pushed to it
                // outside laoban (laoban's own commits wait for each other
                // on `commit_lock`). Not a GitHub or token problem.
                GithubError::Status {
                    stage: "update ref",
                    status: 409 | 422,
                    ..
                } => M0Error::VersionConflict,
                other => M0Error::Github(other),
            });
        drop(commit);
        let sha = sha?;

        Ok(self.version_response(app, p.kind, &sha, p.files))
    }

    /// What publishing a version answers: the commit, the links and the
    /// deployments to watch.
    fn version_response(&self, app: &str, kind: &str, sha: &str, files: &GeneratedFiles) -> Value {
        let s = &self.settings;
        let rollout: Vec<Value> = rollout_hashes(files)
            .into_iter()
            .map(|(component, hash)| {
                let name = format!("{app}-{component}");
                json!({
                    "deployment": name,
                    "path": format!(
                        "/k8s/clusters/{}/v1/apps.deployments/{}/{name}",
                        s.cluster_id, s.target_namespace
                    ),
                    "contentHash": hash,
                })
            })
            .collect();
        json!({
            "mode": kind,
            "appName": app,
            "commit": sha,
            "commitUrl": self.commit_url(sha),
            "path": format!("{APPS_DIR}/{app}"),
            "appUrl": self.app_url(app),
            "fleet": {
                "workspace": FLEET_WORKSPACE,
                "gitRepo": FLEET_GITREPO,
                "bundlePrefix": format!("{FLEET_GITREPO}-{APPS_DIR}-{app}"),
            },
            "rollout": rollout,
        })
    }

    fn commit_url(&self, sha: &str) -> String {
        let s = &self.settings;
        format!(
            "https://github.com/{}/{}/commit/{sha}",
            s.repo_owner, s.repo_name
        )
    }

    fn meta_path(app: &str) -> String {
        format!("{APPS_DIR}/{app}/{META_FILE}")
    }

    /// `laoban.json` of `app` as it was in `commit`.
    fn load_version(&self, app: &str, commit: &str) -> Result<AppMeta, M0Error> {
        if !valid_app_name(app) {
            return Err(M0Error::BadRequest(tr(
                "The app name is not valid.",
                "應用名稱不正確。",
            )));
        }
        if !valid_commit(commit) {
            return Err(M0Error::BadRequest(tr(
                "The version ID is not valid.",
                "版本代號不正確。",
            )));
        }
        let text = self
            .github
            .read_file_at(&Self::meta_path(app), commit)
            .map_err(|e| match e {
                // Unknown commit: GitHub answers 404 or 422.
                GithubError::Status { status: 422, .. } => M0Error::VersionNotFound,
                other => M0Error::Github(other),
            })?
            .ok_or(M0Error::VersionNotFound)?;
        meta::parse(app, &text).map_err(|_| M0Error::VersionNotFound)
    }

    /// `GET /api/apps/<name>/versions`: one entry per commit that changed
    /// the app's laoban.json, newest first. Requests, summaries and models
    /// come from the history in the current laoban.json (newest entries
    /// line up with the newest commits); older entries fall back to the
    /// commit message.
    pub fn versions(&self, app: &str) -> Result<Value, M0Error> {
        let current = self.load_app(app)?;
        let commits = self
            .github
            .list_commits(&Self::meta_path(app), MAX_VERSIONS)
            .map_err(M0Error::Github)?;
        let history: Vec<&HistoryEntry> = current.history.iter().rev().collect();
        let total = commits.len();
        let versions: Vec<Value> = commits
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let h = history.get(i);
                json!({
                    "version": total - i,
                    "commit": c.sha,
                    "commitUrl": self.commit_url(&c.sha),
                    "at": h.map(|h| h.at.as_str()).unwrap_or(&c.date),
                    "kind": h.map(|h| h.kind.as_str()).unwrap_or("commit"),
                    "request": h.map(|h| h.request.as_str()).unwrap_or(&c.message),
                    "summary": h.map(|h| h.summary.as_str()).unwrap_or(""),
                    "model": h.map(|h| h.model.as_str()).unwrap_or(""),
                    "by": h.map(|h| h.by.as_str()).unwrap_or(""),
                    "current": i == 0,
                })
            })
            .collect();
        Ok(json!({
            "appName": app,
            "versions": versions,
            "truncated": total >= MAX_VERSIONS,
        }))
    }

    /// `GET /api/apps/<name>/versions/<commit>`: one version's plan and
    /// architecture (no code).
    pub fn version(&self, app: &str, commit: &str) -> Result<Value, M0Error> {
        let m = self.load_version(app, commit)?;
        Ok(json!({
            "appName": app,
            "commit": commit,
            "summary": m.plan.summary,
            "steps": m.plan.steps,
            "architecture": architecture_with_layout(&m.plan, app, &m.layout),
            "fileDigests": crate::codegen::file_digests(&m.files),
            "updatedAt": m.updated_at(),
        }))
    }

    fn rounds(&self) -> std::sync::MutexGuard<'_, HashMap<String, Round>> {
        self.rounds.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn round_json(&self, app: &str, user: Option<&str>) -> Value {
        self.rounds()
            .get(app)
            .map(|r| r.to_json_for(user))
            .unwrap_or(Value::Null)
    }

    /// Changes outside a round wait while one runs on the app.
    fn check_no_round(&self, app: &str) -> Result<(), M0Error> {
        match self.rounds().get(app) {
            Some(r) if r.state.active() => Err(M0Error::RoundActive),
            _ => Ok(()),
        }
    }

    /// `GET .../apps/<name>/round`: the latest round (or null), whether
    /// rounds can be used, and the limits for a new round.
    pub fn round(&self, app: &str, user: Option<&str>) -> Result<Value, M0Error> {
        if !valid_app_name(app) {
            return Err(M0Error::BadRequest(tr(
                "The app name is not valid.",
                "應用名稱不正確。",
            )));
        }
        Ok(json!({
            "round": self.round_json(app, user),
            "roundsAvailable": self.rounds_ready(),
            "limits": self.round_limits,
        }))
    }

    /// `POST .../apps/<name>/round`: open a round with the first opinion.
    /// With a deadline of 0 it converges right away.
    pub fn open_round(
        &self,
        app: &str,
        body: &OpenRoundRequest,
        user: Option<&str>,
    ) -> Result<Value, M0Error> {
        let user = require_user(user)?;
        if !valid_app_name(app) {
            return Err(M0Error::BadRequest(tr(
                "The app name is not valid.",
                "應用名稱不正確。",
            )));
        }
        self.require_rounds()?;
        let scope = rounds::parse_scope(body.scope.as_deref().unwrap_or("all")).ok_or(
            M0Error::BadRequest(tr(
                "The scope can only be web, api or all.",
                "範圍只能是 web、api 或 all。",
            )),
        )?;
        // Only apps laoban can change.
        let current = self.load_app(app)?;
        // The files at this commit tell hand edits made during the round.
        let start_commit = self.latest_commit(app)?;
        let mut items: Vec<OpinionItem> = Vec::new();
        if !body.opinion.trim().is_empty() || body.items.is_empty() {
            items.push(OpinionItem {
                text: body.opinion.clone(),
                target: None,
            });
        }
        items.extend(body.items.iter().cloned());
        let opinions = self.new_opinions(app, &current, &items)?;
        let now = SystemTime::now();
        let at = time::rfc3339(now);
        {
            let mut rs = self.rounds();
            if rs.get(app).is_some_and(|r| r.state.active()) {
                return Err(M0Error::RoundActive);
            }
            let id = time::round_id(now);
            let mut r = Round::open_with(
                self.round_limits,
                id,
                app,
                user,
                opinions,
                body.deadline_minutes,
                scope,
                unix(now),
                &at,
            )
            .map_err(M0Error::Round)?;
            r.start_commit = start_commit;
            self.write_round(&mut r).map_err(|e| self.state_error(e))?;
            if let Some(old) = rs.insert(app.to_string(), r) {
                self.retire_round(old);
            }
        }
        if body.deadline_minutes == 0 {
            self.converge_if_due_soon(app, unix(now));
        }
        self.round(app, Some(user))
    }

    /// `POST .../round/opinions`: anyone adds an opinion while collecting.
    pub fn add_opinion(
        &self,
        app: &str,
        body: &AddOpinionsRequest,
        user: Option<&str>,
    ) -> Result<Value, M0Error> {
        let user = require_user(user)?;
        self.require_rounds()?;
        let mut items = body.items.clone();
        if !body.text.trim().is_empty() || body.target.is_some() || items.is_empty() {
            items.insert(
                0,
                OpinionItem {
                    text: body.text.clone(),
                    target: body.target.clone(),
                },
            );
        }
        // Targets are checked against the app as it is now.
        let needs_app = items.iter().any(|i| i.target.is_some());
        let opinions = if needs_app {
            let current = self.load_app(app)?;
            self.new_opinions(app, &current, &items)?
        } else {
            items
                .iter()
                .map(|i| rounds::NewOpinion::general(&i.text))
                .collect()
        };
        let at = time::rfc3339(SystemTime::now());
        self.change_round(app, |r| r.add_opinions(user, opinions.clone(), &at))?;
        self.round(app, Some(user))
    }

    /// `POST .../round/opinions/<id>/support`: +1 an opinion or take it back.
    pub fn support_opinion(
        &self,
        app: &str,
        opinion: &str,
        user: Option<&str>,
    ) -> Result<Value, M0Error> {
        let user = require_user(user)?;
        let at = time::rfc3339(SystemTime::now());
        self.change_round(app, |r| r.toggle_support(user, opinion, &at).map(|_| ()))?;
        self.round(app, Some(user))
    }

    /// `POST .../round/opinions/<id>/replies` `{text}`.
    pub fn reply_to_opinion(
        &self,
        app: &str,
        opinion: &str,
        body: &OpinionRequest,
        user: Option<&str>,
    ) -> Result<Value, M0Error> {
        let user = require_user(user)?;
        let at = time::rfc3339(SystemTime::now());
        self.change_round(app, |r| r.reply(user, opinion, &body.text, &at))?;
        self.round(app, Some(user))
    }

    /// `POST .../round/opinions/<id>/edit` `{text}`: the author changes it.
    pub fn edit_opinion(
        &self,
        app: &str,
        opinion: &str,
        body: &OpinionRequest,
        user: Option<&str>,
    ) -> Result<Value, M0Error> {
        let user = require_user(user)?;
        let at = time::rfc3339(SystemTime::now());
        self.change_round(app, |r| r.edit_opinion(user, opinion, &body.text, &at))?;
        self.round(app, Some(user))
    }

    /// `POST .../round/opinions/<id>/withdraw`: the author takes it back.
    pub fn withdraw_opinion(
        &self,
        app: &str,
        opinion: &str,
        user: Option<&str>,
    ) -> Result<Value, M0Error> {
        let user = require_user(user)?;
        let at = time::rfc3339(SystemTime::now());
        self.change_round(app, |r| r.withdraw_opinion(user, opinion, &at))?;
        self.round(app, Some(user))
    }

    /// `POST .../round/deadline` `{minutes}`: the initiator moves the
    /// deadline; 0 converges now.
    pub fn set_round_deadline(
        &self,
        app: &str,
        body: &DeadlineRequest,
        user: Option<&str>,
    ) -> Result<Value, M0Error> {
        let user = require_user(user)?;
        let now = SystemTime::now();
        let at = time::rfc3339(now);
        self.change_round(app, |r| r.set_deadline(user, body.minutes, unix(now), &at))?;
        if body.minutes == 0 {
            self.converge_if_due_soon(app, unix(now));
        }
        self.round(app, Some(user))
    }

    /// `POST .../round/drafts` `{items, shared}`: keep the caller's drafts
    /// in the round, so they are there on another computer and (shared)
    /// others see them.
    pub fn save_drafts(
        &self,
        app: &str,
        body: &DraftsRequest,
        user: Option<&str>,
    ) -> Result<Value, M0Error> {
        let user = require_user(user)?;
        self.require_rounds()?;
        let items = if body.items.iter().any(|i| i.target.is_some()) {
            let current = self.load_app(app)?;
            self.new_opinions(app, &current, &body.items)?
        } else {
            body.items
                .iter()
                .map(|i| rounds::NewOpinion::general(&i.text))
                .collect()
        };
        let shared = body.shared.unwrap_or(true);
        let at = time::rfc3339(SystemTime::now());
        self.change_round(app, |r| r.set_drafts(user, items.clone(), shared, &at))?;
        self.round(app, Some(user))
    }

    /// `POST .../round/plan/steps/<n>/comments` `{text}`: comment on a step
    /// of the converged plan while confirming.
    pub fn comment_on_plan(
        &self,
        app: &str,
        step: usize,
        body: &OpinionRequest,
        user: Option<&str>,
    ) -> Result<Value, M0Error> {
        let user = require_user(user)?;
        let at = time::rfc3339(SystemTime::now());
        self.change_round(app, |r| r.add_plan_comment(user, step, &body.text, &at))?;
        self.round(app, Some(user))
    }

    /// `POST .../round/plan/comments/<id>/support`: +1 a comment, or take
    /// it back.
    pub fn support_plan_comment(
        &self,
        app: &str,
        comment: &str,
        user: Option<&str>,
    ) -> Result<Value, M0Error> {
        let user = require_user(user)?;
        self.change_round(app, |r| r.toggle_comment_support(user, comment).map(|_| ()))?;
        self.round(app, Some(user))
    }

    /// `GET /api/rounds/mine`: the rounds that wait for the caller, on any
    /// app: to confirm, or (initiator) for others to confirm or to retry a
    /// failed one. From memory only; no GitHub call.
    pub fn rounds_waiting(&self, user: Option<&str>) -> Result<Value, M0Error> {
        let user = require_user(user)?;
        let mut items: Vec<Value> = Vec::new();
        for (app, r) in self.rounds().iter() {
            let pending: Vec<&str> = r
                .confirmers
                .iter()
                .filter(|c| c.answer == rounds::Answer::Pending)
                .map(|c| c.user.as_str())
                .collect();
            let confirmer = r.confirmers.iter().any(|c| c.user == user);
            let reason = match r.state {
                State::Confirming if pending.contains(&user) => "confirm",
                // Answered (or started it): waiting for the rest.
                State::Confirming if (confirmer || r.initiator == user) && !pending.is_empty() => {
                    "othersConfirm"
                }
                State::Failed if r.initiator == user => "retry",
                // Those who confirmed see that it failed too (report 21).
                State::Failed if confirmer => "failed",
                _ => continue,
            };
            items.push(json!({
                "app": app,
                "roundId": r.id,
                "state": r.state,
                "reason": reason,
                "pending": pending.len(),
            }));
        }
        items.sort_by(|a, b| a["app"].as_str().cmp(&b["app"].as_str()));
        // What the caller must do comes first.
        items.sort_by_key(|v| v["reason"] != "confirm");
        Ok(json!({ "rounds": items, "roundsAvailable": self.rounds_ready() }))
    }

    /// Opinions as sent, with each target checked against the app's
    /// architecture and named as the 3D view names it.
    fn new_opinions(
        &self,
        app: &str,
        current: &AppMeta,
        items: &[OpinionItem],
    ) -> Result<Vec<rounds::NewOpinion>, M0Error> {
        let arch = architecture_with_layout(&current.plan, app, &current.layout);
        let names: HashMap<String, String> = arch["nodes"]
            .as_array()
            .map(|nodes| {
                nodes
                    .iter()
                    .filter_map(|n| {
                        let id = n["id"].as_str()?;
                        let name = n["name"].as_str().unwrap_or(id);
                        Some((id.to_string(), name.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        items
            .iter()
            .map(|i| {
                let target = match i.target.as_deref().map(str::trim) {
                    None | Some("") => None,
                    Some(id) => {
                        let name = names.get(id).ok_or(M0Error::BadRequest(
                            tr("The component the opinion is about is not in this app's architecture. Please refresh the page.", "意見針對的元件不在這個應用的架構裡，請重新整理頁面。"),
                        ))?;
                        Some(rounds::Target {
                            id: id.to_string(),
                            name: name.clone(),
                        })
                    }
                };
                Ok(rounds::NewOpinion {
                    text: i.text.clone(),
                    target,
                })
            })
            .collect()
    }

    /// `POST .../round/confirm`. The last answer starts the AI writing the
    /// code; the response then has `applied` when it ran in the request.
    pub fn confirm_round(&self, app: &str, user: Option<&str>) -> Result<Value, M0Error> {
        let user = require_user(user)?;
        let at = time::rfc3339(SystemTime::now());
        let applying = self.change_round(app, |r| {
            r.confirm(user, &at)?;
            Ok(r.begin_applying_if_answered(&at))
        })?;
        self.finish_if_answered(app, user, applying)
    }

    /// `POST .../round/skip` `{user}`: the initiator skips someone.
    pub fn skip_in_round(
        &self,
        app: &str,
        body: &SkipRequest,
        user: Option<&str>,
    ) -> Result<Value, M0Error> {
        let user = require_user(user)?;
        let at = time::rfc3339(SystemTime::now());
        let applying = self.change_round(app, |r| {
            r.skip(user, body.user.trim(), &at)?;
            Ok(r.begin_applying_if_answered(&at))
        })?;
        self.finish_if_answered(app, user, applying)
    }

    /// `POST .../round/object` `{text}`: disagree; back to collecting.
    pub fn object_in_round(
        &self,
        app: &str,
        body: &OpinionRequest,
        user: Option<&str>,
    ) -> Result<Value, M0Error> {
        let user = require_user(user)?;
        let now = SystemTime::now();
        let at = time::rfc3339(now);
        self.change_round(app, |r| r.object(user, &body.text, unix(now), &at))?;
        self.round(app, Some(user))
    }

    /// `POST .../round/cancel`: the initiator gives up.
    pub fn cancel_round(&self, app: &str, user: Option<&str>) -> Result<Value, M0Error> {
        let user = require_user(user)?;
        let at = time::rfc3339(SystemTime::now());
        self.change_round(app, |r| r.cancel(user, &at))?;
        self.round(app, Some(user))
    }

    /// `POST .../round/converge`: the initiator converges again after a
    /// failure.
    pub fn retry_round(&self, app: &str, user: Option<&str>) -> Result<Value, M0Error> {
        let user = require_user(user)?;
        let at = time::rfc3339(SystemTime::now());
        let id = self.change_round(app, |r| {
            r.check_retry(user)?;
            r.start_converging(&at);
            Ok(r.id.clone())
        })?;
        let app_owned = app.to_string();
        self.in_background("laoban-converge", move |s| s.converge(&app_owned, &id));
        self.round(app, Some(user))
    }

    /// Converge every round whose deadline has passed. Called by the
    /// background timer (see [`M0Service::start_round_timer`]) and tests.
    /// Until the stored rounds could be read, it tries to read them.
    pub fn tick(&self, now_unix: u64) {
        if !self.rounds_ready() {
            let _ = self.restore_rounds();
            return;
        }
        let due: Vec<String> = self
            .rounds()
            .iter()
            .filter(|(_, r)| r.due(now_unix))
            .map(|(app, _)| app.clone())
            .collect();
        for app in due {
            self.converge_if_due(&app, now_unix);
        }
    }

    /// Read the stored rounds, then check for due rounds every 10 seconds,
    /// one at a time, and archive finished ones.
    pub fn start_round_timer(self: &Arc<Self>) {
        self.attach();
        let _ = self.restore_rounds();
        self.plans_tick(persist::now_unix());
        let service = Arc::downgrade(self);
        let _ = std::thread::Builder::new()
            .name("laoban-rounds".into())
            .spawn(move || {
                let mut ticks: u64 = 0;
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(10));
                    match service.upgrade() {
                        Some(s) => {
                            let now = persist::now_unix();
                            s.tick(now);
                            s.archive_tick(now);
                            s.plans_tick(now);
                            ticks += 1;
                            if ticks % PRUNE_EVERY_TICKS == 0 {
                                let _ = s.prune_orphan_rounds();
                            }
                        }
                        None => return,
                    }
                }
            });
        self.start_infra_sync();
    }

    /// Run [`sync_infra`](Self::sync_infra) in the background until it
    /// succeeds once (GitHub may be unreachable at startup).
    fn start_infra_sync(self: &Arc<Self>) {
        let service = Arc::downgrade(self);
        let _ = std::thread::Builder::new()
            .name("laoban-infra-sync".into())
            .spawn(move || loop {
                let Some(s) = service.upgrade() else { return };
                let synced = match s.sync_infra() {
                    Ok(0) => {
                        eprintln!("laoban-m0: app Ingress/NetworkPolicy files match the settings");
                        true
                    }
                    Ok(n) => {
                        eprintln!("laoban-m0: updated {n} app Ingress/NetworkPolicy file(s) to match the settings");
                        true
                    }
                    Err(e) => {
                        eprintln!(
                            "laoban-m0: syncing app Ingress/NetworkPolicy files failed ({}); retrying in {INFRA_SYNC_RETRY_SECS} s",
                            e.log_detail().unwrap_or_else(|| e.to_string())
                        );
                        false
                    }
                };
                if synced {
                    // Rounds of apps deleted while the backend was down go
                    // now, not only at the next periodic check.
                    let n = s.prune_orphan_rounds();
                    eprintln!("laoban-m0: deleted {n} finished round(s) of apps no longer in the repository");
                    return;
                }
                drop(s);
                std::thread::sleep(std::time::Duration::from_secs(INFRA_SYNC_RETRY_SECS));
            });
    }

    /// Bring every app's Ingress and NetworkPolicy files in the repository
    /// in line with the settings (`LAOBAN_APP_URL`, its Ingress options and
    /// `LAOBAN_APP_NETWORK_POLICY`): write missing or outdated ones, delete
    /// the ones the settings no longer want, in one commit. These files
    /// depend only on the app name and the settings, so a changed setting
    /// reaches every app without touching its code. Returns how many files
    /// changed.
    pub fn sync_infra(&self) -> Result<usize, M0Error> {
        let s = &self.settings;
        let branch = self.github.config().branch.clone();
        let mut files = Vec::new();
        let mut deletes = Vec::new();
        for app in self.app_names()? {
            if !valid_app_name(&app) {
                continue;
            }
            let dir = format!("{APPS_DIR}/{app}");
            let present = self
                .github
                .list_files_at(&dir, &branch)
                .map_err(M0Error::Github)?;
            for (name, want) in infra_files(&app, s.app_url.as_ref(), s.app_network_policy) {
                let path = format!("{dir}/{name}");
                let exists = present.iter().any(|f| f == name);
                match want {
                    Some(content) => {
                        let current = if exists {
                            self.github.read_file(&path).map_err(M0Error::Github)?
                        } else {
                            None
                        };
                        if current.as_deref() != Some(content.as_str()) {
                            files.push(CommitFile { path, content });
                        }
                    }
                    None if exists => deletes.push(path),
                    None => {}
                }
            }
        }
        let changed = files.len() + deletes.len();
        if changed == 0 {
            return Ok(0);
        }
        let _commit = self.commit_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.github
            .commit_changes(&files, &deletes, INFRA_SYNC_MESSAGE)
            .map_err(M0Error::Github)?;
        Ok(changed)
    }

    /// Mark a due round converging (written before anything else).
    fn begin_converging(&self, app: &str, now_unix: u64) -> Option<String> {
        let mut rs = self.rounds();
        let r = rs.get(app).filter(|r| r.due(now_unix))?;
        let id = r.id.clone();
        let empty = r.active().count() == 0;
        let at = time::rfc3339(SystemTime::now());
        if empty {
            // Every opinion was withdrawn: nothing to converge.
            self.update_round(&mut rs, app, &id, |r| r.cancel_empty(&at));
            return None;
        }
        self.update_round(&mut rs, app, &id, |r| r.start_converging(&at));
        Some(id)
    }

    /// Like [`Self::converge_if_due`], for a request: the round is marked
    /// converging now (so the response says so) and the AI runs in the
    /// background.
    fn converge_if_due_soon(&self, app: &str, now_unix: u64) {
        if let Some(id) = self.begin_converging(app, now_unix) {
            let app_owned = app.to_string();
            self.in_background("laoban-converge", move |s| s.converge(&app_owned, &id));
        }
    }

    fn converge_if_due(&self, app: &str, now_unix: u64) {
        if let Some(id) = self.begin_converging(app, now_unix) {
            self.converge(app, &id);
        }
    }

    /// The AI turns the round's opinions into one change plan, made from
    /// the app's latest commit; the round keeps what publishing it needs.
    fn converge(&self, app: &str, round_id: &str) {
        let (request, overhead, label, initiator, start_commit) = {
            let rs = self.rounds();
            match rs.get(app) {
                Some(r) if r.id == round_id && r.state == State::Converging => (
                    r.change_request(),
                    r.comments_overhead(),
                    r.opinion_list(),
                    r.initiator.clone(),
                    r.start_commit.clone(),
                ),
                _ => return,
            }
        };
        let result = self.latest_commit(app).and_then(|base_commit| {
            let start = if start_commit.is_empty() || start_commit == base_commit {
                None
            } else {
                Some(self.load_version(app, &start_commit)?.files)
            };
            let body = PlanRequestBody {
                prompt: request,
                app_name: None,
                base_app: Some(app.to_string()),
                background: false,
            };
            let v = self.plan_with(
                &body,
                Some(&initiator),
                Some(RoundInput {
                    label,
                    start,
                    base_commit: Some(base_commit.clone()),
                }),
                // The round's text, its framing, and the plan steps the
                // comments are on (not counted in the round's space).
                rounds::MAX_ROUND_CHARS + 2_000 + overhead,
            )?;
            Ok((v, base_commit))
        });
        let at = time::rfc3339(SystemTime::now());
        let mut rs = self.rounds();
        if !rs
            .get(app)
            .is_some_and(|r| r.id == round_id && r.state == State::Converging)
        {
            return;
        }
        match result {
            Ok((v, base_commit)) => {
                let parts: Vec<Part> = v["changedParts"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|p| match p.as_str() {
                                Some("web") => Some(Part::Web),
                                Some("api") => Some(Part::Api),
                                _ => None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let plan_id = v["planId"].as_str().unwrap_or_default().to_string();
                let plan = RoundPlan {
                    plan_id: plan_id.clone(),
                    summary: v["summary"].as_str().unwrap_or_default().to_string(),
                    steps: v["steps"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|s| s.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default(),
                    architecture: v["architecture"].clone(),
                    notes: v["notes"].as_str().unwrap_or_default().to_string(),
                };
                let recipe = self.lock_store().get(&plan_id).map(|p| PlanRecipe {
                    plan: p.plan,
                    masked_prompt: p.masked_prompt,
                    model: p.model,
                    label: p.label,
                    base_commit,
                });
                let size = json!({ "plan": &plan, "recipe": &recipe })
                    .to_string()
                    .len();
                if size > rounds::MAX_PLAN_BYTES {
                    self.update_round(&mut rs, app, round_id, |r| {
                        r.fail_coded(
                            "planTooLarge",
                            &[],
                            tr("The AI's change plan is too large (over 150 KB). The initiator can narrow the scope or shorten the opinions and converge again.", "AI 產生的修改計畫太大（超過 150 KB），請發起人縮小範圍或精簡意見後重新收斂。"),
                            &at,
                        )
                    });
                } else {
                    self.update_round(&mut rs, app, round_id, |r| {
                        r.widen_scope(&parts);
                        r.converged(plan, &at);
                        r.recipe = recipe;
                    });
                }
            }
            Err(e) => {
                log_failure("converge", &format!("{app} {round_id}"), &e);
                self.update_round(&mut rs, app, round_id, |r| fail_with(r, &e, &at));
            }
        }
    }

    /// After the last answer (already written as "applying", in the same
    /// write as the answer): the AI writes the code and it is published,
    /// in the background when attached (the response then says
    /// "applying"; the page reads the round until it is done).
    fn finish_if_answered(&self, app: &str, user: &str, applying: bool) -> Result<Value, M0Error> {
        if !applying {
            return self.round(app, Some(user));
        }
        let (plan_id, initiator, round_id) = {
            let rs = self.rounds();
            let Some(r) = rs.get(app).filter(|r| r.state == State::Applying) else {
                drop(rs);
                return self.round(app, Some(user));
            };
            let plan_id = r
                .plan
                .as_ref()
                .map(|p| p.plan_id.clone())
                .unwrap_or_default();
            (plan_id, r.initiator.clone(), r.id.clone())
        };
        let app_owned = app.to_string();
        let applied = self
            .in_background("laoban-publish", move |s| {
                s.publish_round(&app_owned, &round_id, &plan_id, &initiator)
            })
            .flatten();
        let mut v = self.round(app, Some(user))?;
        if let Some(applied) = applied {
            v["applied"] = applied;
        }
        Ok(v)
    }

    /// Write the code for a round's plan, publish it, and record the
    /// result on the round. Returns the apply response when it worked.
    fn publish_round(
        &self,
        app: &str,
        round_id: &str,
        plan_id: &str,
        initiator: &str,
    ) -> Option<Value> {
        let result = self
            .ensure_round_plan(app, round_id, plan_id)
            .and_then(|()| self.apply_plan(plan_id, Some(initiator), true));
        let at = time::rfc3339(SystemTime::now());
        let mut rs = self.rounds();
        self.update_round(&mut rs, app, round_id, |r| match &result {
            Ok(v) => r.done(v.clone(), &at),
            Err(M0Error::PlanNotFound) => r.fail_coded(
                "planMissing",
                &[],
                tr(
                    "This round's change plan was not found. The initiator can converge again.",
                    "找不到這一輪的修改計畫，請發起人重新收斂。",
                ),
                &at,
            ),
            Err(e) => {
                log_failure("publish", &format!("{app} {round_id}"), e);
                fail_with(r, e, &at)
            }
        });
        result.ok()
    }

    /// Namespace the apps are deployed to; the API is served under it.
    pub fn target_namespace(&self) -> &str {
        &self.settings.target_namespace
    }

    /// Full SHA of the commit that last changed the app.
    fn latest_commit(&self, app: &str) -> Result<String, M0Error> {
        self.github
            .list_commits(&Self::meta_path(app), 1)
            .map_err(M0Error::Github)?
            .into_iter()
            .next()
            .map(|c| c.sha)
            .ok_or(M0Error::NotEditable)
    }

    fn check_base(&self, app: &str, base_commit: &str) -> Result<(), M0Error> {
        if !valid_commit(base_commit.trim()) {
            return Err(M0Error::BadRequest(tr(
                "The version ID is not valid.",
                "版本代號不正確。",
            )));
        }
        if self.latest_commit(app)? != base_commit.trim() {
            return Err(M0Error::VersionConflict);
        }
        Ok(())
    }

    /// `GET /api/apps/<name>/files`: the current code, for the editor.
    pub fn files(&self, app: &str) -> Result<Value, M0Error> {
        if !valid_app_name(app) {
            return Err(M0Error::BadRequest(tr(
                "The app name is not valid.",
                "應用名稱不正確。",
            )));
        }
        let m = self.load_app(app)?;
        let commit = self.latest_commit(app)?;
        Ok(json!({ "appName": app, "commit": commit, "files": m.files.to_json() }))
    }

    /// `POST /api/apps/<name>/files`: hand-edited code as a new version.
    /// Checked like AI output; no AI call.
    pub fn save_files(
        &self,
        app: &str,
        body: &SaveFilesRequest,
        user: Option<&str>,
    ) -> Result<Value, M0Error> {
        if !valid_app_name(app) {
            return Err(M0Error::BadRequest(tr(
                "The app name is not valid.",
                "應用名稱不正確。",
            )));
        }
        require_user(user)?;
        let files = parse_generated(&body.files.to_string())
            .map_err(|e| M0Error::InvalidFiles(files_problem(&e.0)))?;
        let lock = self.app_lock(app);
        let _one_at_a_time = lock.lock().unwrap_or_else(|e| e.into_inner());
        let current = self.load_app(app)?;
        self.check_base(app, &body.base_commit)?;
        if files == current.files {
            return Err(M0Error::BadRequest(tr(
                "The code has no changes.",
                "程式沒有任何變更。",
            )));
        }
        // Files the running round is about stay as they are until it ends.
        let blocked = self
            .rounds()
            .get(app)
            .map(|r| r.blocked_files())
            .unwrap_or_default();
        let touched: Vec<&str> = crate::codegen::changed_files(&current.files, &files)
            .into_iter()
            .filter(|f| blocked.contains(f))
            .collect();
        if !touched.is_empty() {
            return Err(M0Error::RoundScope(touched.join("、")));
        }
        let note = body
            .note
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(|n| n.chars().take(200).collect::<String>())
            .unwrap_or_else(|| tr("Edited the code by hand", "手動修改程式").into());
        let request = self.orchestrator.mask_for_ai(&note).text;
        let summary = self.orchestrator.mask_for_ai(&current.plan.summary).text;
        let message = commit_message(
            app,
            &trf!(en: "Hand edit: {request}", zh: "手動修改：{request}"),
        );
        self.publish(Publish {
            app,
            plan: &current.plan,
            files: &files,
            history: current.history,
            layout: current.layout,
            kind: "manual",
            request,
            summary,
            model: String::new(),
            message,
            by: user.unwrap_or_default().to_string(),
        })
    }

    /// `POST /api/apps/<name>/layout`: dragged 3D positions as a new
    /// version. Code and plan are unchanged; no AI call.
    pub fn save_layout(
        &self,
        app: &str,
        body: &SaveLayoutRequest,
        user: Option<&str>,
    ) -> Result<Value, M0Error> {
        if !valid_app_name(app) {
            return Err(M0Error::BadRequest(tr(
                "The app name is not valid.",
                "應用名稱不正確。",
            )));
        }
        require_user(user)?;
        if body.positions.is_empty() || body.positions.len() > meta::MAX_LAYOUT_ENTRIES {
            return Err(M0Error::BadRequest(tr(
                "The layout data is not valid.",
                "版面資料不正確。",
            )));
        }
        let lock = self.app_lock(app);
        let _one_at_a_time = lock.lock().unwrap_or_else(|e| e.into_inner());
        let current = self.load_app(app)?;
        self.check_base(app, &body.base_commit)?;
        let arch = architecture_with_layout(&current.plan, app, &Layout::new());
        let ids: Vec<&str> = arch["nodes"]
            .as_array()
            .map(|n| n.iter().filter_map(|n| n["id"].as_str()).collect())
            .unwrap_or_default();
        let mut layout = current.layout.clone();
        for (id, pos) in &body.positions {
            if !ids.contains(&id.as_str()) || !pos.valid() {
                return Err(M0Error::BadRequest(tr(
                    "The layout data is not valid.",
                    "版面資料不正確。",
                )));
            }
            layout.insert(id.clone(), Pos { x: pos.x, z: pos.z });
        }
        if layout == current.layout {
            return Err(M0Error::BadRequest(tr(
                "The layout has no changes.",
                "版面沒有任何變更。",
            )));
        }
        let request = tr("Rearranged the 3D layout", "調整 3D 版面").to_string();
        let summary = self.orchestrator.mask_for_ai(&current.plan.summary).text;
        let message = commit_message(app, &request);
        self.publish(Publish {
            app,
            plan: &current.plan,
            files: &current.files,
            history: current.history,
            layout,
            kind: "layout",
            request,
            summary,
            model: String::new(),
            message,
            by: user.unwrap_or_default().to_string(),
        })
    }

    /// `POST /api/apps/<name>/restore`: publish an earlier version's plan and
    /// code again as a new version. No AI call; history is kept.
    pub fn restore(
        &self,
        app: &str,
        body: &RestoreRequest,
        user: Option<&str>,
    ) -> Result<Value, M0Error> {
        let commit = body.commit.trim();
        let target = self.load_version(app, commit)?;
        let lock = self.app_lock(app);
        let _one_at_a_time = lock.lock().unwrap_or_else(|e| e.into_inner());
        let current = self.load_app(app)?;
        require_user(user)?;
        self.check_no_round(app)?;
        if target.files == current.files
            && target.plan == current.plan
            && target.layout == current.layout
        {
            return Err(M0Error::BadRequest(tr(
                "The app already has this version's content.",
                "目前已經是這個版本的內容。",
            )));
        }
        let short: String = commit.chars().take(7).collect();
        let request = trf!(en: "Restore to the version of {} ({short})", zh: "還原到 {} 的版本（{short}）", target.updated_at());
        let model = target
            .history
            .last()
            .map(|h| h.model.clone())
            .unwrap_or_default();
        let summary = self.orchestrator.mask_for_ai(&target.plan.summary).text;
        let message = commit_message(app, &trf!(en: "Restore: {request}", zh: "還原：{request}"));
        self.publish(Publish {
            app,
            plan: &target.plan,
            files: &target.files,
            history: current.history,
            layout: target.layout.clone(),
            kind: "restore",
            request,
            summary,
            model,
            message,
            by: user.unwrap_or_default().to_string(),
        })
    }
}

/// Everything [`M0Service::publish`] needs for one new version.
struct Publish<'a> {
    app: &'a str,
    plan: &'a crate::plan::AppPlan,
    files: &'a crate::codegen::GeneratedFiles,
    /// History before this version.
    history: Vec<HistoryEntry>,
    /// Dragged 3D positions of this version.
    layout: Layout,
    /// Verified user who asked for it ("" if unknown).
    by: String,
    /// `create`, `edit` or `restore`.
    kind: &'static str,
    /// Masked request text recorded in history and README.
    request: String,
    /// Masked plan summary.
    summary: String,
    model: String,
    message: String,
}

/// Changes need a verified caller.
/// A round failed with `e`: its message, and a code the page translates
/// when it has one (`laoban.round.failures.*`).
/// Background work has no request log line: say what failed and why, as
/// that line would (report 21, B16). Never the token, key or user text.
fn log_failure(what: &str, subject: &str, e: &M0Error) {
    match e.log_detail() {
        Some(d) => eprintln!("laoban-m0: {what} {subject} failed: {} ({d})", e.code()),
        None => eprintln!("laoban-m0: {what} {subject} failed: {}", e.code()),
    }
}

fn fail_with(r: &mut Round, e: &M0Error, at: &str) {
    let (code, args): (&str, Vec<(&str, String)>) = match e {
        M0Error::MergeConflict(files) => ("mergeConflict", vec![("files", files.clone())]),
        M0Error::ManualEditLost(files) => ("manualEditLost", vec![("files", files.clone())]),
        M0Error::AiInvalidOutput(_) => ("aiInvalidOutput", vec![]),
        M0Error::AiUnavailable(_) => ("aiUnavailable", vec![]),
        M0Error::BudgetExceeded => ("budgetExceeded", vec![]),
        M0Error::SpendLimit => ("spendLimit", vec![]),
        M0Error::Github(_) => ("githubFailed", vec![]),
        M0Error::VersionConflict => ("versionConflict", vec![]),
        _ => ("", vec![]),
    };
    r.fail_coded(code, &args, &e.user_message(), at);
}

fn require_user(user: Option<&str>) -> Result<&str, M0Error> {
    user.filter(|u| !u.is_empty())
        .ok_or(M0Error::Unauthenticated)
}

fn state_label(s: State) -> &'static str {
    match s {
        State::Collecting => tr("Collecting opinions", "收集意見中"),
        State::Converging => tr("AI converging", "AI 收斂中"),
        State::Confirming => tr("Waiting for confirmation", "等待確認"),
        State::Applying => tr("AI changing the code", "AI 修改程式中"),
        State::Done => tr("Done", "已完成"),
        State::Failed => tr("Failed", "失敗"),
        State::Cancelled => tr("Cancelled", "已取消"),
    }
}

/// Parts of the app whose plan changed (the code of those parts will).
fn changed_parts(old: &crate::plan::AppPlan, new: &crate::plan::AppPlan) -> Vec<Part> {
    let mut parts = Vec::new();
    if old.web_modules != new.web_modules {
        parts.push(Part::Web);
    }
    if old.api_modules != new.api_modules || old.endpoints != new.endpoints {
        parts.push(Part::Api);
    }
    parts
}

/// A user-facing reason for hand-edited files failing the checks.
fn files_problem(detail: &str) -> String {
    if detail.contains("handle()") {
        tr(
            "handler.py must define handle(method, path, body).",
            "handler.py 必須定義 handle(method, path, body)。",
        )
        .into()
    } else if detail.contains("exceed") {
        tr(
            "The files are too large (at most 64 KiB each and 192 KiB in all).",
            "檔案太大（每個檔 64 KiB、全部 192 KiB 以內）。",
        )
        .into()
    } else {
        tr("Only the four files index.html, app.js, style.css and handler.py are allowed, and all must be text.", "只能有 index.html、app.js、style.css、handler.py 四個檔，內容都要是文字。").into()
    }
}

/// A full commit SHA (40 or 64 lowercase hex digits).
fn valid_commit(c: &str) -> bool {
    (c.len() == 40 || c.len() == 64)
        && c.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `name` if it is free, otherwise `name-2` … `name-9` (shortened so the
/// result is still a valid app name). `None` if all are taken.
pub fn unused_name(name: &str, existing: &[String]) -> Option<String> {
    if !existing.iter().any(|e| e == name) {
        return Some(name.to_string());
    }
    let stem: String = name.chars().take(29).collect();
    let stem = stem.trim_end_matches('-');
    (2..=9)
        .map(|n| format!("{stem}-{n}"))
        .find(|c| valid_app_name(c) && !existing.iter().any(|e| e == c))
}

/// `laoban: <app> — <first 60 characters of the summary>` plus the
/// `Generated-by: laoban` trailer.
/// Commit message of [`M0Service::sync_infra`].
pub const INFRA_SYNC_MESSAGE: &str =
    "laoban: sync app Ingress and NetworkPolicy files with the backend settings\n\nGenerated-by: laoban\n";
/// Retry a failed startup sync after this many seconds.
const INFRA_SYNC_RETRY_SECS: u64 = 60;
/// Look for rounds of deleted apps every this many 10-second ticks.
const PRUNE_EVERY_TICKS: u64 = 60;

pub fn commit_message(app: &str, summary: &str) -> String {
    let one_line: String = summary
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(60)
        .collect();
    format!("laoban: {app} — {one_line}\n\nGenerated-by: laoban\n")
}

/// M0 does not export audit events (design: 範圍).
struct NoAudit;

impl laoban_ai::AiAuditSink for NoAudit {
    fn record(&self, _event: laoban_ai::AiAuditEvent) {}
}

/// What a round adds to [`M0Service::plan_with`].
struct RoundInput {
    /// The opinions, for the history.
    label: String,
    /// The files when the round started.
    start: Option<GeneratedFiles>,
    /// The app's commit the plan is made from.
    base_commit: Option<String>,
}

fn unix(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_message_shape() {
        let m = commit_message("demo", "早餐店\n點餐  頁面");
        assert_eq!(
            m,
            "laoban: demo — 早餐店 點餐 頁面\n\nGenerated-by: laoban\n"
        );
        let long = "字".repeat(100);
        let m = commit_message("demo", &long);
        assert_eq!(
            m.lines().next().unwrap().chars().count(),
            "laoban: demo — ".chars().count() + 60
        );
    }

    #[test]
    fn error_codes_and_statuses() {
        let cases: Vec<(M0Error, &str, u16)> = vec![
            (M0Error::BadRequest("x"), "bad_request", 400),
            (M0Error::PlanNotFound, "plan_not_found", 404),
            (M0Error::PlanStoreFailed, "plan_store_failed", 503),
            (M0Error::AlreadyApplied, "already_applied", 409),
            (M0Error::BudgetExceeded, "budget_exceeded", 429),
            (M0Error::SpendLimit, "spend_limit", 402),
            (
                M0Error::AiInvalidOutput(InvalidOutput("x".into())),
                "ai_invalid_output",
                502,
            ),
            (M0Error::Github(GithubError::BadPath), "github_failed", 502),
            (M0Error::Internal("x".into()), "internal", 500),
        ];
        for (e, code, status) in cases {
            assert_eq!((e.code(), e.status()), (code, status));
            assert!(!e.user_message().is_empty());
        }
    }

    #[test]
    fn log_detail_only_for_hidden_causes() {
        assert_eq!(
            M0Error::Github(GithubError::BadPath)
                .log_detail()
                .as_deref(),
            Some("refusing to commit an unsafe path")
        );
        assert!(M0Error::AiUnavailable("transport")
            .log_detail()
            .unwrap()
            .contains("transport"));
        // AI output is not logged.
        assert_eq!(
            M0Error::AiInvalidOutput(InvalidOutput(
                "files are not JSON: invalid type: string \"secret\", expected map".into()
            ))
            .log_detail()
            .as_deref(),
            Some("files are not JSON")
        );
        assert_eq!(
            M0Error::AiInvalidOutput(InvalidOutput("plan touches a denied kind `Secret`".into()))
                .log_detail()
                .as_deref(),
            Some("plan touches a denied kind")
        );
        assert_eq!(
            M0Error::AiInvalidOutput(InvalidOutput("web has a key that is not allowed".into()))
                .log_detail()
                .as_deref(),
            Some("web has a key that is not allowed")
        );
        assert_eq!(
            M0Error::AiInvalidOutput(InvalidOutput(": x".into()))
                .log_detail()
                .as_deref(),
            Some("unknown")
        );
        assert_eq!(M0Error::PlanNotFound.log_detail(), None);
    }
}
