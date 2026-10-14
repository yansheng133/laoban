//! The M0 service behind `POST /api/plan`, `POST /api/apply` and
//! `GET /api/config`. Transport-independent: the HTTP layer only parses
//! JSON bodies and maps [`M0Error`] to status codes.

use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use laoban_ai::{
    AiError, BudgetError, CompletionRequest, Dependencies, ModelSelection, Orchestrator,
    OrchestratorConfig, PlanError, PlanPolicy, Provider, ProviderError, RandomBoundary,
    RetryPolicy, ThreadSleeper, TokenBudget, UntrustedBlock, UtcMonthClock,
};
use laoban_mask::MaskerConfig;
use serde::Deserialize;
use serde_json::{json, Value};
use thiserror::Error;

use crate::codegen::{parse_generated, CODEGEN_INSTRUCTIONS, EDIT_CODEGEN_INSTRUCTIONS};
use crate::config::M0Settings;
use crate::github::{GithubCommitter, GithubError};
use crate::lock::{LockError, LockTable};
use crate::meta::{self, AppMeta, HistoryEntry, Layout, Pos, META_FILE};
use crate::plan::{
    architecture_with_layout, parse_app_plan, valid_app_name, EDIT_PLAN_INSTRUCTIONS,
    PLAN_INSTRUCTIONS,
};
use crate::render::{render, rollout_hashes, RenderInput};
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

/// `POST /api/apps/<name>/lock` body.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LockRequest {
    /// Display name of who is editing (the page's Rancher user).
    pub holder: String,
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
    /// Someone else is editing the app. Holder and since are for display.
    #[error("app is locked")]
    Locked { holder: String, since: String },
    /// No valid edit lock (never taken, or expired).
    #[error("edit lock required")]
    LockRequired,
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
            M0Error::Locked { .. } => "locked",
            M0Error::LockRequired => "lock_required",
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
            M0Error::Locked { .. } | M0Error::LockRequired | M0Error::VersionConflict => 409,
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
            M0Error::Locked { holder, since } => format!(
                "{holder} 正在編輯這個應用（{since} 起）。等對方離開，或 3 分鐘沒有動作後再試。"
            ),
            M0Error::LockRequired => "編輯鎖已失效，請重新整理頁面後再試。".into(),
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
    locks: Mutex<LockTable>,
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
            locks: Mutex::new(LockTable::default()),
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

    /// `POST /api/plan`.
    pub fn plan(&self, body: &PlanRequestBody, lock: Option<&str>) -> Result<Value, M0Error> {
        let prompt = body.prompt.trim();
        if prompt.is_empty() {
            return Err(M0Error::BadRequest("請輸入需求。"));
        }
        if prompt.chars().count() > MAX_PROMPT_CHARS {
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
        let (plan, completion, app, base) = match base_name {
            Some(base_app) => {
                let current = self.load_app(&base_app)?;
                self.require_lock(&base_app, lock)?;
                if current.files.total_bytes() > MAX_EDIT_CODE_BYTES {
                    return Err(M0Error::BadRequest(
                        "這個應用的程式太大，試吃版無法用 AI 編輯。",
                    ));
                }
                let current_plan =
                    serde_json::to_string_pretty(&current.plan.to_codegen_json(&base_app))
                        .map_err(|e| M0Error::Internal(e.to_string()))?;
                let completion = self.orchestrator.complete(&CompletionRequest {
                    principal: PRINCIPAL.into(),
                    requested_model: None,
                    instructions: format!("{PLAN_INSTRUCTIONS}\n{EDIT_PLAN_INSTRUCTIONS}"),
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
                let base = BaseApp {
                    files: current.files,
                    history: current.history,
                    layout: current.layout,
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
    pub fn get_app(&self, app: &str) -> Result<Value, M0Error> {
        if !valid_app_name(app) {
            return Err(M0Error::BadRequest("應用名稱不正確。"));
        }
        let m = self.load_app(app)?;
        let mut v = self.app_links(app);
        v["appName"] = json!(app);
        v["summary"] = json!(m.plan.summary);
        v["steps"] = json!(m.plan.steps);
        v["architecture"] = architecture_with_layout(&m.plan, app, &m.layout);
        v["lock"] = self.lock_json(app);
        v["updatedAt"] = json!(m.updated_at());
        v["history"] = json!(m.history.iter().rev().collect::<Vec<_>>());
        Ok(v)
    }

    /// `POST /api/apply`.
    pub fn apply(&self, body: &ApplyRequest, lock: Option<&str>) -> Result<Value, M0Error> {
        let id = body.plan_id.trim();
        let stored = self
            .lock_store()
            .begin_apply(id, Instant::now())
            .map_err(|e| match e {
                StoreError::NotFound => M0Error::PlanNotFound,
                StoreError::AlreadyApplied => M0Error::AlreadyApplied,
            })?;
        let result = self.apply_inner(&stored, lock);
        self.lock_store().finish_apply(id, result.is_ok());
        result
    }

    fn apply_inner(&self, stored: &StoredPlan, lock: Option<&str>) -> Result<Value, M0Error> {
        if stored.base.is_some() {
            self.require_lock(&stored.app, lock)?;
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
        if let Some(base) = &stored.base {
            blocks.push(UntrustedBlock {
                kind: "current_code",
                label: None,
                content: serde_json::to_string_pretty(&base.files.to_json())
                    .map_err(|e| M0Error::Internal(e.to_string()))?,
            });
            instructions.push('\n');
            instructions.push_str(EDIT_CODEGEN_INSTRUCTIONS);
        }
        let completion = self.orchestrator.complete(&CompletionRequest {
            principal: PRINCIPAL.into(),
            requested_model: None,
            instructions,
            blocks,
            max_tokens: Some(CODEGEN_MAX_TOKENS),
        })?;
        let files = parse_generated(&completion.content).map_err(M0Error::AiInvalidOutput)?;

        let app = stored.app.as_str();
        let masked_summary = self.orchestrator.mask_for_ai(&stored.plan.summary).text;
        let masked_prompt = self.orchestrator.mask_for_ai(&stored.masked_prompt).text;
        let (kind, message) = if stored.base.is_some() {
            (
                "edit",
                commit_message(app, &format!("修改：{masked_prompt}")),
            )
        } else {
            ("create", commit_message(app, &masked_summary))
        };
        let history = stored
            .base
            .as_ref()
            .map(|b| b.history.clone())
            .unwrap_or_default();
        let layout = stored
            .base
            .as_ref()
            .map(|b| b.layout.clone())
            .unwrap_or_default();
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

    fn locks(&self) -> std::sync::MutexGuard<'_, LockTable> {
        self.locks.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn lock_json(&self, app: &str) -> Value {
        match self.locks().status(app, Instant::now()) {
            Some(info) => json!({ "holder": info.holder, "since": time::rfc3339(info.since) }),
            None => Value::Null,
        }
    }

    /// Changes to an existing app need its edit lock.
    fn require_lock(&self, app: &str, token: Option<&str>) -> Result<(), M0Error> {
        self.locks()
            .check(app, token, Instant::now())
            .map_err(lock_error)
    }

    /// `POST /api/apps/<name>/lock`: take the edit lock.
    pub fn lock(&self, app: &str, body: &LockRequest) -> Result<Value, M0Error> {
        if !valid_app_name(app) {
            return Err(M0Error::BadRequest("應用名稱不正確。"));
        }
        if !self.app_names()?.iter().any(|n| n == app) {
            return Err(M0Error::AppNotFound);
        }
        let grant = self
            .locks()
            .acquire(app, &body.holder, Instant::now())
            .map_err(lock_error)?;
        Ok(json!({
            "token": grant.token,
            "holder": grant.holder,
            "ttlSeconds": grant.ttl.as_secs(),
        }))
    }

    /// `POST /api/apps/<name>/lock/renew` (token in the lock header).
    pub fn renew_lock(&self, app: &str, token: Option<&str>) -> Result<Value, M0Error> {
        let token = token.ok_or(M0Error::LockRequired)?;
        self.locks()
            .renew(app, token, Instant::now())
            .map_err(lock_error)?;
        Ok(json!({ "ttlSeconds": crate::lock::LOCK_TTL.as_secs() }))
    }

    /// `POST /api/apps/<name>/unlock` (token in the lock header).
    pub fn unlock(&self, app: &str, token: Option<&str>) -> Result<Value, M0Error> {
        if let Some(t) = token {
            self.locks().release(app, t);
        }
        Ok(json!({ "released": true }))
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
        lock: Option<&str>,
    ) -> Result<Value, M0Error> {
        if !valid_app_name(app) {
            return Err(M0Error::BadRequest("應用名稱不正確。"));
        }
        self.require_lock(app, lock)?;
        let files = parse_generated(&body.files.to_string())
            .map_err(|e| M0Error::InvalidFiles(files_problem(&e.0)))?;
        let current = self.load_app(app)?;
        self.check_base(app, &body.base_commit)?;
        if files == current.files {
            return Err(M0Error::BadRequest("程式沒有任何變更。"));
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
        })
    }

    /// `POST /api/apps/<name>/layout`: dragged 3D positions as a new
    /// version. Code and plan are unchanged; no AI call.
    pub fn save_layout(
        &self,
        app: &str,
        body: &SaveLayoutRequest,
        lock: Option<&str>,
    ) -> Result<Value, M0Error> {
        if !valid_app_name(app) {
            return Err(M0Error::BadRequest("應用名稱不正確。"));
        }
        self.require_lock(app, lock)?;
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
        })
    }

    /// `POST /api/apps/<name>/restore`: publish an earlier version's plan and
    /// code again as a new version. No AI call; history is kept.
    pub fn restore(
        &self,
        app: &str,
        body: &RestoreRequest,
        lock: Option<&str>,
    ) -> Result<Value, M0Error> {
        let commit = body.commit.trim();
        let target = self.load_version(app, commit)?;
        let current = self.load_app(app)?;
        self.require_lock(app, lock)?;
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
    /// `create`, `edit` or `restore`.
    kind: &'static str,
    /// Masked request text recorded in history and README.
    request: String,
    /// Masked plan summary.
    summary: String,
    model: String,
    message: String,
}

fn lock_error(e: LockError) -> M0Error {
    match e {
        LockError::Held(info) => M0Error::Locked {
            holder: info.holder,
            since: time::rfc3339(info.since),
        },
        LockError::NotHeld => M0Error::LockRequired,
    }
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
