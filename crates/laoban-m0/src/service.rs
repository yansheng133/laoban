//! The M0 service behind `POST /api/plan`, `POST /api/apply` and
//! `GET /api/config`. Transport-independent: the HTTP layer only parses
//! JSON bodies and maps [`M0Error`] to status codes.

use std::collections::HashMap;
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
use crate::github::{GithubCommitter, GithubError};
use crate::meta::{self, AppMeta, HistoryEntry, Layout, Pos, META_FILE};
use crate::plan::{
    architecture_with_layout, carry_layout, parse_app_plan, valid_app_name, EDIT_PLAN_INSTRUCTIONS,
    PLAN_INSTRUCTIONS, ROUND_PLAN_INSTRUCTIONS,
};
use crate::render::{render, rollout_hashes, RenderInput};
use crate::rounds::{self, Part, Round, RoundError, RoundPlan, State};
use crate::store::{BaseApp, PlanStore, StoreError, StoredPlan};
use crate::{time, InvalidOutput};

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
    /// The first opinion.
    pub opinion: String,
    /// Minutes until the AI converges; 0 converges right away.
    pub deadline_minutes: u64,
    /// "web", "api" or "all" (default).
    #[serde(default)]
    pub scope: Option<String>,
}

/// `POST .../round/opinions` and `.../round/object` body.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpinionRequest {
    pub text: String,
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
}

/// Errors with the codes and statuses of the design's error table.
/// `user_message` is Traditional Chinese for the page; `Display` is for
/// tests and never contains secrets or user content.
#[derive(Debug, Error)]
pub enum M0Error {
    #[error("bad request: {0}")]
    BadRequest(&'static str),
    #[error("plan not found")]
    PlanNotFound,
    #[error("plan already applied")]
    AlreadyApplied,
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
}

impl M0Error {
    pub fn code(&self) -> &'static str {
        match self {
            M0Error::BadRequest(_) => "bad_request",
            M0Error::PlanNotFound => "plan_not_found",
            M0Error::AlreadyApplied => "already_applied",
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
            M0Error::Round(RoundError::Invalid(_)) | M0Error::Round(RoundError::Full) => {
                "bad_request"
            }
            M0Error::Round(RoundError::WrongState(_)) => "round_state",
            M0Error::Round(RoundError::NotInitiator) => "not_initiator",
            M0Error::Round(RoundError::NotConfirmer) => "not_confirmer",
            M0Error::Unauthenticated => "unauthenticated",
            M0Error::VersionConflict => "version_conflict",
            M0Error::InvalidFiles(_) => "invalid_files",
        }
    }

    /// Extra detail for the server log, for errors whose cause the user
    /// message hides (which GitHub step failed and how, why the AI call
    /// failed). These messages never contain the token, the API key or the
    /// user's prompt; AI output (`ai_invalid_output`) is deliberately left out.
    pub fn log_detail(&self) -> Option<String> {
        match self {
            M0Error::Github(_) | M0Error::AiUnavailable(_) | M0Error::Internal(_) => {
                Some(self.to_string())
            }
            _ => None,
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            M0Error::BadRequest(_) => 400,
            M0Error::PlanNotFound => 404,
            M0Error::AlreadyApplied => 409,
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
            M0Error::Round(RoundError::Invalid(_)) | M0Error::Round(RoundError::Full) => 400,
            M0Error::Round(RoundError::NotInitiator) => 403,
            M0Error::Unauthenticated => 401,
            M0Error::InvalidFiles(_) => 400,
        }
    }

    pub fn user_message(&self) -> String {
        match self {
            M0Error::BadRequest(m) => (*m).to_string(),
            M0Error::PlanNotFound => {
                "找不到這個計畫，可能已超過 1 小時或後端已重新啟動，請重新產生計畫。".into()
            }
            M0Error::AlreadyApplied => "這個計畫已經部署過（或正在部署），不能重複部署。".into(),
            M0Error::BudgetExceeded => {
                "已達到 laoban 本期的 AI token 使用上限，請等下個月或請管理者調高上限。".into()
            }
            M0Error::SpendLimit => {
                "Anthropic 帳號已達花費上限，請管理者到 Anthropic Console 檢查；系統不會自動重試。"
                    .into()
            }
            M0Error::AiInvalidOutput(_) => {
                "AI 回應的格式不符合要求，沒有寫入任何東西，請再試一次或換個說法。".into()
            }
            M0Error::AiUnavailable(m) => (*m).to_string(),
            M0Error::Github(_) => {
                "寫入 GitHub 失敗，請稍後再試；若持續失敗請管理者檢查 token 與 repo 設定。".into()
            }
            M0Error::Internal(_) => "後端發生內部錯誤。".into(),
            M0Error::AppExists(app) => format!(
                "已經有名為 {app} 的應用。要修改它，請在下方應用清單按「編輯」；要另外建立，請換一個名稱。"
            ),
            M0Error::AppNotFound => "找不到這個應用，可能已經被刪除。".into(),
            M0Error::VersionNotFound => "找不到這個版本。".into(),
            M0Error::RoundActive => {
                "這個應用正在進行一輪意見收斂，修改請到應用的詳細頁提意見；這一輪結束前不能還原或另外產生修改計畫。".into()
            }
            M0Error::RoundScope(files) => format!(
                "這一輪意見收斂正在處理 {files}，這一輪結束前不能直接修改這些檔案；其他檔案可以照常修改。"
            ),
            M0Error::MergeConflict(files) => format!(
                "這一輪期間 {files} 也被直接修改過，和 AI 的修改衝突，沒有部署。請發起人重新收斂。"
            ),
            M0Error::ManualEditLost(files) => format!(
                "AI 的修改會蓋掉這一輪期間對 {files} 的手動修改，沒有部署。請發起人重新收斂；如果手動修改不需要保留，請先把它改回去。"
            ),
            M0Error::Round(RoundError::Invalid(m)) => (*m).into(),
            M0Error::Round(RoundError::Full) => {
                "這一輪的意見已達上限（30 則或 8000 字），請等這一輪結束再開下一輪。".into()
            }
            M0Error::Round(RoundError::WrongState(st)) => format!(
                "這一輪目前不能這樣做（{}）。請重新整理頁面。",
                state_label(*st)
            ),
            M0Error::Round(RoundError::NotInitiator) => "只有發起這一輪的人可以這樣做。".into(),
            M0Error::Round(RoundError::NotConfirmer) => {
                "你不需要確認這一輪：你沒有在這一輪提意見，或已經回覆過。".into()
            }
            M0Error::Unauthenticated => {
                "無法確認你的身分：請從 Rancher 的 laoban 頁面操作。".into()
            }
            M0Error::VersionConflict => {
                "這個應用在你編輯期間有了新版本，請重新讀取後再改。".into()
            }
            M0Error::InvalidFiles(m) => format!("程式沒有通過檢查：{m}"),
            M0Error::NotEditable => {
                "這個應用沒有 laoban 的編輯紀錄（可能是較早版本產生的），無法編輯；請用新名稱重新產生。"
                    .into()
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
                    M0Error::AiUnavailable("AI 服務目前忙碌中，請稍後再試。")
                }
                ProviderError::Authentication { .. } => {
                    M0Error::AiUnavailable("AI 服務認證失敗，請管理者檢查 API 金鑰設定。")
                }
                ProviderError::InvalidRequest { .. } | ProviderError::Other { .. } => {
                    M0Error::AiUnavailable("呼叫 AI 服務失敗，請稍後再試。")
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
    /// The latest round of 意見收斂 per app (active or finished).
    rounds: Mutex<HashMap<String, Round>>,
    /// Per app: (round ID, the files when that round started).
    round_starts: Mutex<HashMap<String, (String, GeneratedFiles)>>,
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
            round_starts: Mutex::new(HashMap::new()),
        })
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
            return Err(M0Error::BadRequest("請輸入需求。"));
        }
        if prompt.chars().count() > max_chars {
            return Err(M0Error::BadRequest("需求太長，請縮短到 4000 字以內。"));
        }
        let requested_name = match body.app_name.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(n) if valid_app_name(n) => Some(n.to_string()),
            Some(_) => return Err(M0Error::BadRequest(
                "應用名稱只能用小寫英文字母、數字和連字號，以英文字母開頭，長度 2 到 31 個字元。",
            )),
        };

        let base_name = match body.base_app.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(n) if valid_app_name(n) => Some(n.to_string()),
            Some(_) => return Err(M0Error::BadRequest("要編輯的應用名稱不正確。")),
        };
        if let (Some(base), Some(req)) = (&base_name, &requested_name) {
            if base != req {
                return Err(M0Error::BadRequest("編輯現有應用時不能改名稱。"));
            }
        }

        let masked = self.orchestrator.mask_for_ai(prompt);
        let mut parts: Vec<Part> = Vec::new();
        let (plan, completion, app, base) = match base_name {
            Some(base_app) => {
                let current = self.load_app(&base_app)?;
                require_user(user)?;
                if current.files.total_bytes() > MAX_EDIT_CODE_BYTES {
                    return Err(M0Error::BadRequest(
                        "這個應用的程式太大，試吃版無法用 AI 編輯。",
                    ));
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
                let plan = parse_app_plan(&completion.content).map_err(M0Error::AiInvalidOutput)?;
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
        });
        store.insert(
            StoredPlan {
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
            },
            Instant::now(),
        );
        Ok(response)
    }

    fn app_url(&self, app: &str) -> String {
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
            return Err(M0Error::BadRequest("應用名稱不正確。"));
        }
        let m = self.load_app(app)?;
        let mut v = self.app_links(app);
        v["appName"] = json!(app);
        v["summary"] = json!(m.plan.summary);
        v["steps"] = json!(m.plan.steps);
        v["architecture"] = architecture_with_layout(&m.plan, app, &m.layout);
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
        let result = if stored.base.is_some() && !from_round {
            self.check_no_round(&stored.app)
                .and_then(|()| self.apply_inner(&stored, user))
        } else {
            self.apply_inner(&stored, user)
        };
        self.lock_store().finish_apply(id, result.is_ok());
        result
    }

    fn apply_inner(&self, stored: &StoredPlan, user: Option<&str>) -> Result<Value, M0Error> {
        if stored.base.is_some() {
            require_user(user)?;
        }
        // A new app must still not exist (another plan may have been
        // applied under the same name since).
        if stored.base.is_none() && self.app_names()?.contains(&stored.app) {
            return Err(M0Error::AppExists(stored.app.clone()));
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
                commit_message(app, &format!("修改：{masked_prompt}")),
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
        });
        let sha = self
            .github
            .commit(&commit_files, &p.message)
            .map_err(M0Error::Github)?;

        let s = &self.settings;
        let rollout: Vec<Value> = rollout_hashes(p.files)
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
        Ok(json!({
            "mode": p.kind,
            "appName": app,
            "commit": sha,
            "commitUrl": self.commit_url(&sha),
            "path": format!("{APPS_DIR}/{app}"),
            "appUrl": self.app_url(app),
            "fleet": {
                "workspace": FLEET_WORKSPACE,
                "gitRepo": FLEET_GITREPO,
                "bundlePrefix": format!("{FLEET_GITREPO}-{APPS_DIR}-{app}"),
            },
            "rollout": rollout,
        }))
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
            return Err(M0Error::BadRequest("應用名稱不正確。"));
        }
        if !valid_commit(commit) {
            return Err(M0Error::BadRequest("版本代號不正確。"));
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

    /// `GET .../apps/<name>/round`: the latest round (or null).
    pub fn round(&self, app: &str, user: Option<&str>) -> Result<Value, M0Error> {
        if !valid_app_name(app) {
            return Err(M0Error::BadRequest("應用名稱不正確。"));
        }
        Ok(json!({ "round": self.round_json(app, user) }))
    }

    /// `POST .../apps/<name>/round`: open a round with the first opinion.
    /// With a deadline of 0 it converges right away (in this request).
    pub fn open_round(
        &self,
        app: &str,
        body: &OpenRoundRequest,
        user: Option<&str>,
    ) -> Result<Value, M0Error> {
        let user = require_user(user)?;
        if !valid_app_name(app) {
            return Err(M0Error::BadRequest("應用名稱不正確。"));
        }
        let scope = rounds::parse_scope(body.scope.as_deref().unwrap_or("all"))
            .ok_or(M0Error::BadRequest("範圍只能是 web、api 或 all。"))?;
        // Only apps laoban can change.
        let start_files = self.load_app(app)?.files;
        let now = SystemTime::now();
        let at = time::rfc3339(now);
        let id = time::plan_id(now).replacen("p-", "r-", 1);
        {
            let mut rs = self.rounds();
            if rs.get(app).is_some_and(|r| r.state.active()) {
                return Err(M0Error::RoundActive);
            }
            let r = Round::open(
                id,
                app,
                user,
                &body.opinion,
                body.deadline_minutes,
                scope,
                unix(now),
                &at,
            )
            .map_err(M0Error::Round)?;
            self.round_starts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(app.to_string(), (r.id.clone(), start_files));
            rs.insert(app.to_string(), r);
        }
        if body.deadline_minutes == 0 {
            self.converge_if_due(app, unix(now));
        }
        self.round(app, Some(user))
    }

    /// `POST .../round/opinions`: anyone adds an opinion while collecting.
    pub fn add_opinion(
        &self,
        app: &str,
        body: &OpinionRequest,
        user: Option<&str>,
    ) -> Result<Value, M0Error> {
        let user = require_user(user)?;
        let at = time::rfc3339(SystemTime::now());
        self.with_round(app, |r| r.add_opinion(user, &body.text, &at))?;
        self.round(app, Some(user))
    }

    /// `POST .../round/confirm`. The last answer starts the AI writing the
    /// code (in this request); the response then has `applied`.
    pub fn confirm_round(&self, app: &str, user: Option<&str>) -> Result<Value, M0Error> {
        let user = require_user(user)?;
        let at = time::rfc3339(SystemTime::now());
        self.with_round(app, |r| r.confirm(user, &at))?;
        self.finish_if_answered(app, user)
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
        self.with_round(app, |r| r.skip(user, body.user.trim(), &at))?;
        self.finish_if_answered(app, user)
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
        self.with_round(app, |r| r.object(user, &body.text, unix(now), &at))?;
        self.round(app, Some(user))
    }

    /// `POST .../round/cancel`: the initiator gives up.
    pub fn cancel_round(&self, app: &str, user: Option<&str>) -> Result<Value, M0Error> {
        let user = require_user(user)?;
        let at = time::rfc3339(SystemTime::now());
        self.with_round(app, |r| r.cancel(user, &at))?;
        self.round(app, Some(user))
    }

    /// `POST .../round/converge`: the initiator converges again after a
    /// failure (in this request).
    pub fn retry_round(&self, app: &str, user: Option<&str>) -> Result<Value, M0Error> {
        let user = require_user(user)?;
        let id = {
            let mut rs = self.rounds();
            let r = rs
                .get_mut(app)
                .ok_or(M0Error::Round(RoundError::WrongState(State::Cancelled)))?;
            r.check_retry(user).map_err(M0Error::Round)?;
            r.start_converging(&time::rfc3339(SystemTime::now()));
            r.id.clone()
        };
        self.converge(app, &id);
        self.round(app, Some(user))
    }

    fn with_round<F>(&self, app: &str, f: F) -> Result<(), M0Error>
    where
        F: FnOnce(&mut Round) -> Result<(), RoundError>,
    {
        let mut rs = self.rounds();
        match rs.get_mut(app) {
            Some(r) if r.state.active() || r.state == State::Done => f(r).map_err(M0Error::Round),
            Some(r) => Err(M0Error::Round(RoundError::WrongState(r.state))),
            None => Err(M0Error::Round(RoundError::WrongState(State::Cancelled))),
        }
    }

    /// Converge every round whose deadline has passed. Called by the
    /// background timer (see [`M0Service::start_round_timer`]) and tests.
    pub fn tick(&self, now_unix: u64) {
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

    /// Check for due rounds every 10 seconds, one at a time.
    pub fn start_round_timer(self: &Arc<Self>) {
        let service = Arc::downgrade(self);
        let _ = std::thread::Builder::new()
            .name("laoban-rounds".into())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(10));
                match service.upgrade() {
                    Some(s) => s.tick(unix(SystemTime::now())),
                    None => return,
                }
            });
    }

    fn converge_if_due(&self, app: &str, now_unix: u64) {
        let id = {
            let mut rs = self.rounds();
            match rs.get_mut(app) {
                Some(r) if r.due(now_unix) => {
                    r.start_converging(&time::rfc3339(SystemTime::now()));
                    r.id.clone()
                }
                _ => return,
            }
        };
        self.converge(app, &id);
    }

    /// The AI turns the round's opinions into one change plan.
    fn converge(&self, app: &str, round_id: &str) {
        let (request, label, initiator) = {
            let rs = self.rounds();
            match rs.get(app) {
                Some(r) if r.id == round_id && r.state == State::Converging => {
                    (r.change_request(), r.opinion_list(), r.initiator.clone())
                }
                _ => return,
            }
        };
        let start = self
            .round_starts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(app)
            .filter(|(id, _)| id == round_id)
            .map(|(_, f)| f.clone());
        let body = PlanRequestBody {
            prompt: request,
            app_name: None,
            base_app: Some(app.to_string()),
        };
        let result = self.plan_with(
            &body,
            Some(&initiator),
            Some(RoundInput { label, start }),
            rounds::MAX_ROUND_CHARS + 2_000,
        );
        let at = time::rfc3339(SystemTime::now());
        let mut rs = self.rounds();
        let Some(r) = rs
            .get_mut(app)
            .filter(|r| r.id == round_id && r.state == State::Converging)
        else {
            return;
        };
        match result {
            Ok(v) => {
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
                r.widen_scope(&parts);
                r.converged(
                    RoundPlan {
                        plan_id: v["planId"].as_str().unwrap_or_default().to_string(),
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
                    },
                    &at,
                );
            }
            Err(e) => r.fail(&e.user_message(), &at),
        }
    }

    /// When everyone answered: the AI writes the code and it is published.
    fn finish_if_answered(&self, app: &str, user: &str) -> Result<Value, M0Error> {
        let (plan_id, initiator, round_id) = {
            let mut rs = self.rounds();
            let Some(r) = rs.get_mut(app) else {
                return self.round(app, Some(user));
            };
            if !r.all_answered() {
                drop(rs);
                return self.round(app, Some(user));
            }
            r.start_applying(&time::rfc3339(SystemTime::now()));
            let plan_id = r
                .plan
                .as_ref()
                .map(|p| p.plan_id.clone())
                .unwrap_or_default();
            (plan_id, r.initiator.clone(), r.id.clone())
        };
        let result = self.apply_plan(&plan_id, Some(&initiator), true);
        let at = time::rfc3339(SystemTime::now());
        {
            let mut rs = self.rounds();
            if let Some(r) = rs.get_mut(app).filter(|r| r.id == round_id) {
                match &result {
                    Ok(v) => r.done(v.clone(), &at),
                    Err(M0Error::PlanNotFound) => r.fail(
                        "修改計畫已超過 1 小時沒有部署而失效，請發起人重新收斂。",
                        &at,
                    ),
                    Err(e) => r.fail(&e.user_message(), &at),
                }
            }
        }
        let mut v = self.round(app, Some(user))?;
        if let Ok(applied) = result {
            v["applied"] = applied;
        }
        Ok(v)
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
            return Err(M0Error::BadRequest("版本代號不正確。"));
        }
        if self.latest_commit(app)? != base_commit.trim() {
            return Err(M0Error::VersionConflict);
        }
        Ok(())
    }

    /// `GET /api/apps/<name>/files`: the current code, for the editor.
    pub fn files(&self, app: &str) -> Result<Value, M0Error> {
        if !valid_app_name(app) {
            return Err(M0Error::BadRequest("應用名稱不正確。"));
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
            return Err(M0Error::BadRequest("應用名稱不正確。"));
        }
        require_user(user)?;
        let files = parse_generated(&body.files.to_string())
            .map_err(|e| M0Error::InvalidFiles(files_problem(&e.0)))?;
        let current = self.load_app(app)?;
        self.check_base(app, &body.base_commit)?;
        if files == current.files {
            return Err(M0Error::BadRequest("程式沒有任何變更。"));
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
            .unwrap_or_else(|| "手動修改程式".into());
        let request = self.orchestrator.mask_for_ai(&note).text;
        let summary = self.orchestrator.mask_for_ai(&current.plan.summary).text;
        let message = commit_message(app, &format!("手動修改：{request}"));
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
            return Err(M0Error::BadRequest("應用名稱不正確。"));
        }
        require_user(user)?;
        if body.positions.is_empty() || body.positions.len() > meta::MAX_LAYOUT_ENTRIES {
            return Err(M0Error::BadRequest("版面資料不正確。"));
        }
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
                return Err(M0Error::BadRequest("版面資料不正確。"));
            }
            layout.insert(id.clone(), Pos { x: pos.x, z: pos.z });
        }
        if layout == current.layout {
            return Err(M0Error::BadRequest("版面沒有任何變更。"));
        }
        let request = "調整 3D 版面".to_string();
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
        let current = self.load_app(app)?;
        require_user(user)?;
        self.check_no_round(app)?;
        if target.files == current.files
            && target.plan == current.plan
            && target.layout == current.layout
        {
            return Err(M0Error::BadRequest("目前已經是這個版本的內容。"));
        }
        let short: String = commit.chars().take(7).collect();
        let request = format!("還原到 {} 的版本（{short}）", target.updated_at());
        let model = target
            .history
            .last()
            .map(|h| h.model.clone())
            .unwrap_or_default();
        let summary = self.orchestrator.mask_for_ai(&target.plan.summary).text;
        let message = commit_message(app, &format!("還原：{request}"));
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
fn require_user(user: Option<&str>) -> Result<&str, M0Error> {
    user.filter(|u| !u.is_empty())
        .ok_or(M0Error::Unauthenticated)
}

fn state_label(s: State) -> &'static str {
    match s {
        State::Collecting => "收集意見中",
        State::Converging => "AI 收斂中",
        State::Confirming => "等待確認",
        State::Applying => "AI 修改程式中",
        State::Done => "已完成",
        State::Failed => "失敗",
        State::Cancelled => "已取消",
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
        "handler.py 必須定義 handle(method, path, body)。".into()
    } else if detail.contains("exceed") {
        "檔案太大（每個檔 64 KiB、全部 192 KiB 以內）。".into()
    } else {
        "只能有 index.html、app.js、style.css、handler.py 四個檔，內容都要是文字。".into()
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
            M0Error::AiInvalidOutput(InvalidOutput("model said x".into())).log_detail(),
            None
        );
        assert_eq!(M0Error::PlanNotFound.log_detail(), None);
    }
}
