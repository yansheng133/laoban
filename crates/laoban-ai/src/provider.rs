//! Provider-neutral request/response types and the [`Provider`] trait.
//!
//! Only Anthropic is targeted for now, but nothing here is Anthropic
//! specific. The HTTP client is the `anthropic` module (feature `http`);
//! the README documents the Messages API mapping.

use std::collections::VecDeque;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::error::ProviderError;

/// Conversation role. The Messages API has no `system` role inside
/// `messages`; the system prompt is a separate field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

/// One call to the model. Everything in here has already been masked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderRequest {
    /// Model ID taken from configuration. Never hard-coded.
    pub model: String,
    pub system: String,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
}

/// Why the model stopped. Unknown values are kept as text so a new API
/// value does not break parsing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    Refusal,
    Other(String),
}

impl StopReason {
    /// Map the provider's string value.
    pub fn from_api(value: &str) -> Self {
        match value {
            "end_turn" => StopReason::EndTurn,
            "max_tokens" => StopReason::MaxTokens,
            "stop_sequence" => StopReason::StopSequence,
            "refusal" => StopReason::Refusal,
            other => StopReason::Other(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderResponse {
    /// Concatenated text content.
    pub content: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub stop_reason: StopReason,
}

/// A model provider. Synchronous on purpose: the orchestrator is called
/// from a request handler that can run it on a blocking thread, and this
/// keeps the dependency set small.
pub trait Provider: Send + Sync {
    fn send(&self, req: &ProviderRequest) -> Result<ProviderResponse, ProviderError>;
}

/// Scripted provider for tests. Returns the queued results in order and
/// records every request it receives. Never touches the network.
#[derive(Debug, Default)]
pub struct MockProvider {
    script: Mutex<VecDeque<Result<ProviderResponse, ProviderError>>>,
    received: Mutex<Vec<ProviderRequest>>,
}

impl MockProvider {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a result. Builder style.
    pub fn then(self, result: Result<ProviderResponse, ProviderError>) -> Self {
        self.push(result);
        self
    }

    pub fn push(&self, result: Result<ProviderResponse, ProviderError>) {
        self.script.lock().unwrap().push_back(result);
    }

    /// Requests received so far, in order.
    pub fn received(&self) -> Vec<ProviderRequest> {
        self.received.lock().unwrap().clone()
    }

    pub fn call_count(&self) -> usize {
        self.received.lock().unwrap().len()
    }
}

impl Provider for MockProvider {
    fn send(&self, req: &ProviderRequest) -> Result<ProviderResponse, ProviderError> {
        self.received.lock().unwrap().push(req.clone());
        self.script.lock().unwrap().pop_front().unwrap_or_else(|| {
            Err(ProviderError::Other {
                status: None,
                error_type: "mock_exhausted".into(),
                message: "MockProvider has no scripted result left".into(),
            })
        })
    }
}

impl<P: Provider + ?Sized> Provider for std::sync::Arc<P> {
    fn send(&self, req: &ProviderRequest) -> Result<ProviderResponse, ProviderError> {
        (**self).send(req)
    }
}
