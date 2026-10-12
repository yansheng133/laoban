//! Provider error classification.
//!
//! The mapping follows the Anthropic API documentation (see the crate
//! README for the exact pages):
//!
//! * a spend limit the organization or workspace set for itself returns
//!   HTTP 400 `invalid_request_error`, with a message beginning
//!   `You have reached your specified API usage limits` or
//!   `You have reached your specified workspace API usage limits`;
//! * the usage tier's monthly spend cap returns HTTP 429 with error type
//!   `rate_limit_error` (the same type as a rate limit), no `retry-after`
//!   header, and `error.details.error_code` = `enforced_spend_limit_reached`;
//! * an ordinary rate limit returns HTTP 429 `rate_limit_error` with a
//!   `retry-after` header;
//! * overload returns HTTP 529 `overloaded_error`.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Which kind of spend limit was hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpendLimitKind {
    /// A limit the organization or a workspace set for itself (HTTP 400).
    SelfSet,
    /// The usage tier's monthly spend cap (HTTP 429, no `retry-after`).
    TierCap,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProviderError {
    #[error("rate limited by the provider (retry after {retry_after:?})")]
    RateLimited { retry_after: Option<Duration> },
    #[error("provider is overloaded")]
    Overloaded,
    /// Not retryable: it keeps failing until the limit resets or is raised.
    #[error("provider spend limit reached ({kind:?}): {message}")]
    SpendLimitReached {
        kind: SpendLimitKind,
        message: String,
    },
    #[error("invalid request: {message}")]
    InvalidRequest { message: String },
    /// Credentials are missing, invalid, expired, or lack permission.
    #[error("authentication or permission failure: {message}")]
    Authentication { message: String },
    #[error("provider error (status {status:?}, type {error_type}): {message}")]
    Other {
        status: Option<u16>,
        error_type: String,
        message: String,
    },
}

impl ProviderError {
    /// Only transient capacity errors are retried. Spend limits, invalid
    /// requests and credential errors fail the same way every time.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ProviderError::RateLimited { .. } | ProviderError::Overloaded
        )
    }

    /// Server-suggested wait, if any.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            ProviderError::RateLimited { retry_after } => *retry_after,
            _ => None,
        }
    }
}

/// What the HTTP layer extracts from an error response.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApiErrorInfo<'a> {
    pub status: u16,
    /// `error.type`
    pub error_type: &'a str,
    /// `error.message`
    pub message: &'a str,
    /// `error.details.error_code`, when present.
    pub error_code: Option<&'a str>,
    /// Parsed `retry-after` header, when present.
    pub retry_after: Option<Duration>,
}

/// `error.details.error_code` for the tier spend cap.
pub const TIER_CAP_ERROR_CODE: &str = "enforced_spend_limit_reached";

const SELF_SET_PREFIX: &str = "you have reached your specified";
const TIER_CAP_PREFIX: &str = "you have reached your api usage limits";

/// Classify an error response.
pub fn classify(info: &ApiErrorInfo<'_>) -> ProviderError {
    let message = info.message.to_string();
    let lower = info.message.trim_start().to_ascii_lowercase();

    // The tier cap shares status and type with a rate limit, so check the
    // error code (and, as a fallback, the documented message) first.
    if info.error_code == Some(TIER_CAP_ERROR_CODE)
        || (info.status == 429 && info.retry_after.is_none() && lower.starts_with(TIER_CAP_PREFIX))
    {
        return ProviderError::SpendLimitReached {
            kind: SpendLimitKind::TierCap,
            message,
        };
    }
    if info.status == 400
        && info.error_type == "invalid_request_error"
        && lower.starts_with(SELF_SET_PREFIX)
        && lower.contains("usage limits")
    {
        return ProviderError::SpendLimitReached {
            kind: SpendLimitKind::SelfSet,
            message,
        };
    }

    match (info.status, info.error_type) {
        (429, _) | (_, "rate_limit_error") => ProviderError::RateLimited {
            retry_after: info.retry_after,
        },
        (529, _) | (_, "overloaded_error") => ProviderError::Overloaded,
        (401 | 403, _) | (_, "authentication_error" | "permission_error") => {
            ProviderError::Authentication { message }
        }
        (400 | 404 | 413, _)
        | (_, "invalid_request_error" | "not_found_error" | "request_too_large") => {
            ProviderError::InvalidRequest { message }
        }
        (status, error_type) => ProviderError::Other {
            status: Some(status),
            error_type: error_type.to_string(),
            message,
        },
    }
}
