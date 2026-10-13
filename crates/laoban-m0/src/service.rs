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

use crate::codegen::{parse_generated, CODEGEN_INSTRUCTIONS};
use crate::config::M0Settings;
use crate::github::{GithubCommitter, GithubError};
use crate::plan::{architecture, parse_app_plan, valid_app_name, PLAN_INSTRUCTIONS};
use crate::render::{render, RenderInput};
use crate::store::{PlanStore, StoreError, StoredPlan};
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
    pub fn plan(&self, body: &PlanRequestBody) -> Result<Value, M0Error> {
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

        let masked = self.orchestrator.mask_for_ai(prompt);
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
                Some(n) if valid_app_name(n) => n.to_string(),
                _ => {
                    return Err(M0Error::AiInvalidOutput(InvalidOutput(
                        "appName missing or invalid".into(),
                    )))
                }
            },
        };
        let arch = architecture(&plan, &app);

        let now = SystemTime::now();
        let mut store = self.lock_store();
        let mut id = time::plan_id(now);
        while store.contains(&id) {
            id = time::plan_id(now);
        }
        let response = json!({
            "planId": id,
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
            },
            Instant::now(),
        );
        Ok(response)
    }

    /// `POST /api/apply`.
    pub fn apply(&self, body: &ApplyRequest) -> Result<Value, M0Error> {
        let id = body.plan_id.trim();
        let stored = self
            .lock_store()
            .begin_apply(id, Instant::now())
            .map_err(|e| match e {
                StoreError::NotFound => M0Error::PlanNotFound,
                StoreError::AlreadyApplied => M0Error::AlreadyApplied,
            })?;
        let result = self.apply_inner(&stored);
        self.lock_store().finish_apply(id, result.is_ok());
        result
    }

    fn apply_inner(&self, stored: &StoredPlan) -> Result<Value, M0Error> {
        let plan_json = serde_json::to_string_pretty(&stored.plan.to_codegen_json(&stored.app))
            .map_err(|e| M0Error::Internal(e.to_string()))?;
        let completion = self.orchestrator.complete(&CompletionRequest {
            principal: PRINCIPAL.into(),
            requested_model: None,
            instructions: CODEGEN_INSTRUCTIONS.into(),
            blocks: vec![UntrustedBlock {
                kind: "approved_plan",
                label: None,
                content: plan_json,
            }],
            max_tokens: Some(CODEGEN_MAX_TOKENS),
        })?;
        let files = parse_generated(&completion.content).map_err(M0Error::AiInvalidOutput)?;

        let app = stored.app.as_str();
        let masked_summary = self.orchestrator.mask_for_ai(&stored.plan.summary).text;
        let masked_prompt = self.orchestrator.mask_for_ai(&stored.masked_prompt).text;
        let generated_at = time::rfc3339(SystemTime::now());
        let commit_files = render(&RenderInput {
            app,
            namespace: &self.settings.target_namespace,
            files: &files,
            masked_prompt: &masked_prompt,
            masked_summary: &masked_summary,
            model: &completion.model,
            generated_at: &generated_at,
        });
        let message = commit_message(app, &masked_summary);
        let sha = self
            .github
            .commit(&commit_files, &message)
            .map_err(M0Error::Github)?;

        let s = &self.settings;
        Ok(json!({
            "commit": sha,
            "commitUrl": format!("https://github.com/{}/{}/commit/{sha}", s.repo_owner, s.repo_name),
            "path": format!("apps/{app}"),
            "appUrl": format!(
                "/k8s/clusters/{}/api/v1/namespaces/{}/services/http:{app}-web:8080/proxy/",
                s.cluster_id, s.target_namespace
            ),
            "fleet": {
                "workspace": FLEET_WORKSPACE,
                "gitRepo": FLEET_GITREPO,
                "bundlePrefix": format!("{FLEET_GITREPO}-apps-{app}"),
            },
        }))
    }
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
