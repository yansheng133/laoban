//! The orchestrator: the only path from laoban to a model provider.
//!
//! It owns the [`Masker`], so masking cannot be skipped by a caller; it
//! reserves budget before every call; it wraps untrusted text in data
//! blocks; and it parses the response into a validated [`Plan`]. It never
//! executes anything.

use std::sync::Arc;

use laoban_mask::{Destination, MaskError, Masker, MaskerConfig};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::audit::{AiAuditEvent, AiAuditSink};
use crate::budget::{estimate_tokens, BudgetError, TokenBudget};
use crate::error::ProviderError;
use crate::model::{resolve_model, ModelError, ModelSelection};
use crate::plan::{parse_plan, validate_plan, Plan, PlanError, PlanPolicy};
use crate::prompt::{
    boundary_from_nonce, render_blocks, security_preamble, BoundaryGenerator, DataBlock,
    PromptError,
};
use crate::provider::{Message, Provider, ProviderRequest, Role, StopReason};
use crate::retry::{backoff_delay, RetryPolicy, Sleeper};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestratorConfig {
    pub masker: MaskerConfig,
    pub models: ModelSelection,
    #[serde(default)]
    pub retry: RetryPolicy,
    #[serde(default)]
    pub plan_policy: PlanPolicy,
    /// Administrator's system prompt. The security preamble is appended.
    #[serde(default)]
    pub system_prompt: String,
    pub max_tokens: u32,
}

/// Collaborators, injected so tests can replace every side effect.
pub struct Dependencies {
    pub provider: Box<dyn Provider>,
    pub budget: Arc<TokenBudget>,
    pub audit: Box<dyn AiAuditSink>,
    pub sleeper: Box<dyn Sleeper>,
    pub boundary: Box<dyn BoundaryGenerator>,
}

/// A labelled piece of untrusted context (cluster state, file contents).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextItem {
    pub label: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanRequest {
    /// Rancher principal ID of the requester. Goes to audit only, never to
    /// the provider.
    pub principal: String,
    pub requested_model: Option<String>,
    /// The only cluster the plan may touch.
    pub target_cluster: String,
    /// The user's requirement in natural language.
    pub requirement: String,
    #[serde(default)]
    pub cluster_state: Vec<ContextItem>,
    #[serde(default)]
    pub files: Vec<ContextItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanOutcome {
    pub plan: Plan,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub attempts: u32,
}

#[derive(Debug, Error)]
pub enum AiError {
    #[error(transparent)]
    Mask(#[from] MaskError),
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error(transparent)]
    Budget(#[from] BudgetError),
    #[error(transparent)]
    Prompt(#[from] PromptError),
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error(transparent)]
    Plan(#[from] PlanError),
}

pub struct Orchestrator {
    config: OrchestratorConfig,
    masker: Masker,
    deps: Dependencies,
}

impl Orchestrator {
    pub fn new(config: OrchestratorConfig, deps: Dependencies) -> Result<Self, AiError> {
        config.models.validate()?;
        let masker = Masker::new(config.masker.clone())?;
        Ok(Self {
            config,
            masker,
            deps,
        })
    }

    fn mask(&self, text: &str) -> String {
        self.masker.mask(text, Destination::Ai).text
    }

    /// Build the masked, delimited request. Public so the UI can preview
    /// exactly what would be sent.
    pub fn build_request(
        &self,
        req: &PlanRequest,
        model: String,
    ) -> Result<ProviderRequest, AiError> {
        let boundary = boundary_from_nonce(&self.deps.boundary.nonce())?;

        let mut blocks = vec![
            DataBlock {
                kind: "target_cluster",
                label: None,
                body: self.mask(&req.target_cluster),
            },
            DataBlock {
                kind: "user_requirement",
                label: None,
                body: self.mask(&req.requirement),
            },
        ];
        for item in &req.cluster_state {
            blocks.push(DataBlock {
                kind: "cluster_state",
                label: Some(self.mask(&item.label)),
                body: self.mask(&item.content),
            });
        }
        for item in &req.files {
            blocks.push(DataBlock {
                kind: "file",
                label: Some(self.mask(&item.label)),
                body: self.mask(&item.content),
            });
        }
        let user = render_blocks(&blocks, &boundary);

        let admin = self.mask(&self.config.system_prompt);
        let preamble = security_preamble(&boundary);
        let system = if admin.trim().is_empty() {
            preamble
        } else {
            format!("{admin}\n\n{preamble}")
        };

        Ok(ProviderRequest {
            model,
            system,
            messages: vec![Message {
                role: Role::User,
                content: user,
            }],
            max_tokens: self.config.max_tokens,
        })
    }

    /// Ask the model for a plan. Returns a validated plan or a typed error.
    pub fn propose_plan(&self, req: &PlanRequest) -> Result<PlanOutcome, AiError> {
        let model = resolve_model(&self.config.models, req.requested_model.as_deref())?;
        let request = self.build_request(req, model.clone())?;
        let user_content: String = request
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect();
        let estimated_input = estimate_tokens(&request.system) + estimate_tokens(&user_content);
        let reserve_amount = estimated_input + u64::from(request.max_tokens);

        let policy = &self.config.retry;
        let mut attempt: u32 = 0;
        let response = loop {
            attempt += 1;
            // Refused here, before the provider is called.
            let reservation = self.deps.budget.reserve(reserve_amount)?;
            self.deps.audit.record(AiAuditEvent::Request {
                principal: req.principal.clone(),
                model: model.clone(),
                attempt,
                system: request.system.clone(),
                content: user_content.clone(),
                estimated_input_tokens: estimated_input,
                max_tokens: request.max_tokens,
            });
            match self.deps.provider.send(&request) {
                Ok(resp) => {
                    reservation.settle(resp.input_tokens + resp.output_tokens);
                    self.deps.audit.record(AiAuditEvent::Response {
                        principal: req.principal.clone(),
                        model: model.clone(),
                        attempt,
                        outcome: "ok".into(),
                        content: self.masker.mask(&resp.content, Destination::Audit).text,
                        input_tokens: resp.input_tokens,
                        output_tokens: resp.output_tokens,
                    });
                    break resp;
                }
                Err(err) => {
                    drop(reservation); // refund
                    self.deps.audit.record(AiAuditEvent::Response {
                        principal: req.principal.clone(),
                        model: model.clone(),
                        attempt,
                        outcome: error_kind(&err).into(),
                        content: String::new(),
                        input_tokens: 0,
                        output_tokens: 0,
                    });
                    let retry_index = attempt - 1;
                    if !err.is_retryable() || retry_index >= policy.max_retries {
                        return Err(err.into());
                    }
                    let delay = backoff_delay(policy, retry_index, err.retry_after());
                    if delay.as_millis() > u128::from(policy.max_delay_ms) {
                        // The server asks for a longer wait than we allow.
                        return Err(err.into());
                    }
                    self.deps.sleeper.sleep(delay);
                }
            }
        };

        match response.stop_reason {
            StopReason::MaxTokens => return Err(PlanError::Truncated.into()),
            StopReason::Refusal => return Err(PlanError::Refused.into()),
            _ => {}
        }
        let plan = parse_plan(&response.content)?;
        validate_plan(&plan, &self.config.plan_policy, &req.target_cluster)?;
        Ok(PlanOutcome {
            plan,
            model,
            input_tokens: response.input_tokens,
            output_tokens: response.output_tokens,
            attempts: attempt,
        })
    }
}

fn error_kind(err: &ProviderError) -> &'static str {
    match err {
        ProviderError::RateLimited { .. } => "rate_limited",
        ProviderError::Overloaded => "overloaded",
        ProviderError::SpendLimitReached { .. } => "spend_limit_reached",
        ProviderError::InvalidRequest { .. } => "invalid_request",
        ProviderError::Authentication { .. } => "authentication",
        ProviderError::Other { .. } => "other",
    }
}
