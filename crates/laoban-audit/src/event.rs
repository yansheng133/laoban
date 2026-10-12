//! The audit event as handed to the formatter.

use std::fmt;

use crate::syslog::Severity;
use crate::timestamp::AuditTimestamp;

/// What happened. Rendered as the RFC 5424 `MSGID` and as the `action`
/// SD-PARAM. Every name is printable US-ASCII and at most 32 characters,
/// so it always satisfies the `MSGID` rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Action {
    ChangeSubmitted,
    ChangeApproved,
    ChangeRejected,
    DeployStarted,
    DeploySucceeded,
    DeployFailed,
    AiRequest,
    AiResponse,
    BackupSkippedConfirmed,
    SettingsChanged,
}

impl Action {
    pub fn as_str(&self) -> &'static str {
        match self {
            Action::ChangeSubmitted => "change.submitted",
            Action::ChangeApproved => "change.approved",
            Action::ChangeRejected => "change.rejected",
            Action::DeployStarted => "deploy.started",
            Action::DeploySucceeded => "deploy.succeeded",
            Action::DeployFailed => "deploy.failed",
            Action::AiRequest => "ai.request",
            Action::AiResponse => "ai.response",
            Action::BackupSkippedConfirmed => "backup.skipped_confirmed",
            Action::SettingsChanged => "settings.changed",
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Result of the action. Decides the syslog severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Outcome {
    /// The action completed. Severity: Notice (5).
    Success,
    /// Informational, e.g. a step started or an AI request was sent.
    /// Severity: Informational (6).
    Info,
    /// The action was refused by a person or by policy. Severity: Warning (4).
    Rejected,
    /// The action was attempted and failed. Severity: Error (3).
    Failure,
}

impl Outcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Success => "success",
            Outcome::Info => "info",
            Outcome::Rejected => "rejected",
            Outcome::Failure => "failure",
        }
    }

    pub fn severity(&self) -> Severity {
        match self {
            Outcome::Success => Severity::Notice,
            Outcome::Info => Severity::Informational,
            Outcome::Rejected => Severity::Warning,
            Outcome::Failure => Severity::Error,
        }
    }
}

/// One audit record before masking.
///
/// Free-text values (`project`, `target`, `detail` and field values) may
/// contain sensitive data; they are masked by [`crate::AuditFormatter`]
/// before anything is rendered. `actor` is a Rancher principal ID such as
/// `local://u-abc12` and is kept as is for traceability.
///
/// `Debug` deliberately prints only the non-sensitive parts, so an event
/// that ends up in a debug log does not leak the unmasked text.
#[derive(Clone)]
pub struct AuditEvent {
    pub timestamp: AuditTimestamp,
    /// Rancher principal ID. Never masked.
    pub actor: String,
    pub action: Action,
    pub outcome: Outcome,
    /// Project name. Masked. Omitted from the record when empty.
    pub project: String,
    /// Target, e.g. layer or cluster. Masked. Omitted when empty.
    pub target: String,
    /// Free text, becomes the syslog `MSG`. Masked. Omitted when empty.
    pub detail: String,
    /// Extra key/value pairs, rendered as SD-PARAMs in this order. Keys must
    /// be valid SD-NAMEs and must not reuse a built-in name. Values are
    /// masked.
    pub fields: Vec<(String, String)>,
}

impl AuditEvent {
    pub fn new(
        timestamp: AuditTimestamp,
        actor: impl Into<String>,
        action: Action,
        outcome: Outcome,
    ) -> Self {
        Self {
            timestamp,
            actor: actor.into(),
            action,
            outcome,
            project: String::new(),
            target: String::new(),
            detail: String::new(),
            fields: Vec::new(),
        }
    }

    pub fn project(mut self, project: impl Into<String>) -> Self {
        self.project = project.into();
        self
    }

    pub fn target(mut self, target: impl Into<String>) -> Self {
        self.target = target.into();
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    pub fn field(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.push((key.into(), value.into()));
        self
    }
}

impl fmt::Debug for AuditEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuditEvent")
            .field("timestamp", &self.timestamp)
            .field("actor", &self.actor)
            .field("action", &self.action)
            .field("outcome", &self.outcome)
            .field("project", &"<unmasked>")
            .field("target", &"<unmasked>")
            .field("detail", &"<unmasked>")
            .field(
                "fields",
                &self.fields.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            )
            .finish()
    }
}
