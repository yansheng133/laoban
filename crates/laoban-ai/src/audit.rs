//! Audit hook. The audit crate is built separately; this trait is the seam.
//! Events carry only masked text.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum AiAuditEvent {
    /// Emitted before every provider call (including retries).
    #[serde(rename = "ai.request")]
    Request {
        /// Rancher principal ID, e.g. `local://u-abc12`.
        principal: String,
        model: String,
        attempt: u32,
        /// Masked system prompt as sent.
        system: String,
        /// Masked user content as sent.
        content: String,
        estimated_input_tokens: u64,
        max_tokens: u32,
    },
    /// Emitted after every provider call.
    #[serde(rename = "ai.response")]
    Response {
        principal: String,
        model: String,
        attempt: u32,
        /// `ok` or the error kind. Never contains request content.
        outcome: String,
        /// Masked response text (empty on error).
        content: String,
        input_tokens: u64,
        output_tokens: u64,
    },
}

pub trait AiAuditSink: Send + Sync {
    fn record(&self, event: AiAuditEvent);
}

/// Collects events in memory. For tests.
#[derive(Debug, Default)]
pub struct MemoryAuditSink {
    events: Mutex<Vec<AiAuditEvent>>,
}

impl MemoryAuditSink {
    pub fn events(&self) -> Vec<AiAuditEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl AiAuditSink for MemoryAuditSink {
    fn record(&self, event: AiAuditEvent) {
        self.events.lock().unwrap().push(event);
    }
}

impl<S: AiAuditSink + ?Sized> AiAuditSink for std::sync::Arc<S> {
    fn record(&self, event: AiAuditEvent) {
        (**self).record(event)
    }
}
