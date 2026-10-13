//! AI orchestration for laoban.
//!
//! * [`provider`]: provider-neutral [`Provider`] trait and a scripted
//!   [`MockProvider`]. No HTTP client yet.
//! * [`error`]: [`ProviderError`] and the HTTP error [`classify`]er.
//! * [`retry`]: retry policy with a pure [`backoff_delay`] and an injected
//!   [`Sleeper`].
//! * [`budget`]: global [`TokenBudget`] shared across Projects.
//! * [`prompt`]: delimited data blocks with a random per-request boundary.
//! * [`plan`]: the typed [`Plan`] the model must return and its allow-list.
//! * [`model`]: forced model or user choice from an allow-list.
//! * [`audit`]: [`AiAuditSink`] hook.
//! * [`orchestrator`]: ties it together; owns the masker.

pub mod audit;
pub mod budget;
pub mod error;
pub mod model;
pub mod orchestrator;
pub mod plan;
pub mod prompt;
pub mod provider;
pub mod retry;

pub use audit::{AiAuditEvent, AiAuditSink, MemoryAuditSink};
pub use budget::{
    estimate_tokens, BudgetError, FixedPeriod, PeriodClock, Reservation, TokenBudget, UtcMonthClock,
};
pub use error::{classify, ApiErrorInfo, ProviderError, SpendLimitKind};
pub use model::{resolve_model, ModelError, ModelSelection};
pub use orchestrator::{
    AiError, ContextItem, Dependencies, Orchestrator, OrchestratorConfig, PlanOutcome, PlanRequest,
};
pub use plan::{
    parse_plan, validate_plan, FileAction, FileChange, Plan, PlanError, PlanPolicy, ResourceAction,
    ResourceRef,
};
pub use prompt::{BoundaryGenerator, FixedBoundary, PromptError, RandomBoundary};
pub use provider::{
    Message, MockProvider, Provider, ProviderRequest, ProviderResponse, Role, StopReason,
};
pub use retry::{backoff_delay, RecordingSleeper, RetryPolicy, Sleeper, ThreadSleeper};
