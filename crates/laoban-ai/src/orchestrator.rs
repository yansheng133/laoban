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
    boundary_from_nonce, data_block_preamble, render_blocks, security_preamble, BoundaryGenerator,
    DataBlock, PromptError,
};
use crate::provider::{Message, Provider, ProviderRequest, ProviderResponse, Role, StopReason};
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

/// One piece of untrusted content for [`Orchestrator::complete`]. It is
/// masked and wrapped in a data block like every other untrusted input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UntrustedBlock {
    /// Fixed block kind chosen by the calling code, never by user input.
    pub kind: &'static str,
    /// Optional label; untrusted, masked and neutralized.
    pub label: Option<String>,
    pub content: String,
}

/// A request for [`Orchestrator::complete`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionRequest {
    /// Rancher principal ID. Goes to audit only, never to the provider.
    pub principal: String,
    pub requested_model: Option<String>,
    /// Trusted task description and response format, fixed by the calling
    /// code. Appended to the system prompt after the data-block preamble.
    /// Never put user input here.
    pub instructions: String,
    pub blocks: Vec<UntrustedBlock>,
    /// Overrides `OrchestratorConfig::max_tokens` for this call.
    pub max_tokens: Option<u32>,
}

/// Raw result of [`Orchestrator::complete`]. `content` is untrusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub content: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub attempts: u32,
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
        let (response, attempts) = self.call(&req.principal, &model, &request)?;
        let plan = parse_plan(&response.content)?;
        validate_plan(&plan, &self.config.plan_policy, &req.target_cluster)?;
        Ok(PlanOutcome {
            plan,
            model,
            input_tokens: response.input_tokens,
            output_tokens: response.output_tokens,
            attempts,
        })
    }

    /// Mask `text` exactly as the orchestrator masks content for the
    /// provider (`Destination::Ai`). For callers that also write the text
    /// somewhere else (for example a generated README) and want the same
    /// rules applied, and for counting findings.
    pub fn mask_for_ai(&self, text: &str) -> laoban_mask::Masked {
        self.masker.mask(text, Destination::Ai)
    }

    /// Build the masked, delimited request for [`Orchestrator::complete`].
    pub fn build_completion_request(
        &self,
        req: &CompletionRequest,
        model: String,
    ) -> Result<ProviderRequest, AiError> {
        let boundary = boundary_from_nonce(&self.deps.boundary.nonce())?;
        let blocks: Vec<DataBlock> = req
            .blocks
            .iter()
            .map(|b| DataBlock {
                kind: b.kind,
                label: b.label.as_deref().map(|l| self.mask(l)),
                body: self.mask(&b.content),
            })
            .collect();
        let user = render_blocks(&blocks, &boundary);

        let admin = self.mask(&self.config.system_prompt);
        let mut system = String::new();
        if !admin.trim().is_empty() {
            system.push_str(&admin);
            system.push_str("\n\n");
        }
        system.push_str(&data_block_preamble(&boundary));
        system.push_str(&req.instructions);

        Ok(ProviderRequest {
            model,
            system,
            messages: vec![Message {
                role: Role::User,
                content: user,
            }],
            max_tokens: req.max_tokens.unwrap_or(self.config.max_tokens),
        })
    }

    /// General-purpose call with the same guarantees as
    /// [`Orchestrator::propose_plan`] (masking, data blocks, budget, retry
    /// rules, audit, truncation/refusal rejection), for callers that parse
    /// and validate a response schema of their own. The response text is
    /// returned unparsed; treat it as untrusted data.
    pub fn complete(&self, req: &CompletionRequest) -> Result<Completion, AiError> {
        let model = resolve_model(&self.config.models, req.requested_model.as_deref())?;
        let request = self.build_completion_request(req, model.clone())?;
        let (response, attempts) = self.call(&req.principal, &model, &request)?;
        Ok(Completion {
            content: response.content,
            model,
            input_tokens: response.input_tokens,
            output_tokens: response.output_tokens,
            attempts,
        })
    }

    /// Budget, audit, send and retry. Rejects truncated and refused
    /// responses.
    fn call(
        &self,
        principal: &str,
        model: &str,
        request: &ProviderRequest,
    ) -> Result<(ProviderResponse, u32), AiError> {
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
                principal: principal.to_string(),
                model: model.to_string(),
                attempt,
                system: request.system.clone(),
                content: user_content.clone(),
                estimated_input_tokens: estimated_input,
                max_tokens: request.max_tokens,
            });
            match self.deps.provider.send(request) {
                Ok(resp) => {
                    reservation.settle(resp.input_tokens + resp.output_tokens);
                    self.deps.audit.record(AiAuditEvent::Response {
                        principal: principal.to_string(),
                        model: model.to_string(),
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
                        principal: principal.to_string(),
                        model: model.to_string(),
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
        Ok((response, attempt))
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
