//! Anthropic Messages API client (feature `http`).
//!
//! Blocking, built on `ureq` with rustls (no OpenSSL). Maps
//! [`ProviderRequest`] / [`ProviderResponse`] as documented in the crate
//! README and classifies every error response with [`classify`], so the
//! retry rules (spend limits are never retried) stay in one place.
//!
//! The API key is read from `ANTHROPIC_API_KEY`, kept in a wrapper whose
//! `Debug` output is redacted, and is never put into an error message.

use std::fmt;
use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;

use crate::error::{classify, ApiErrorInfo, ProviderError};
use crate::provider::{Provider, ProviderRequest, ProviderResponse, StopReason};

/// Environment variable holding the API key.
pub const API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
/// Production endpoint base.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
/// `anthropic-version` header value.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Largest error body read, in bytes. Error bodies are small JSON objects.
const MAX_ERROR_BODY: u64 = 64 * 1024;

/// Connection settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnthropicConfig {
    /// Scheme, host and optional port, without a trailing `/v1/messages`.
    /// Overridden in tests with a local mock server.
    pub base_url: String,
    pub connect_timeout: Duration,
    /// Upper bound for the whole call. Generating code can take minutes,
    /// so this is generous.
    pub request_timeout: Duration,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(300),
        }
    }
}

/// The API key. `Debug` and `Display` never show it.
#[derive(Clone)]
pub struct ApiKey(String);

impl ApiKey {
    /// Reject empty keys and anything that could break or inject a header.
    pub fn new(key: impl Into<String>) -> Result<Self, AnthropicSetupError> {
        let key = key.into();
        let key = key.trim().to_string();
        if key.is_empty() {
            return Err(AnthropicSetupError::MissingKey);
        }
        if !key.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(AnthropicSetupError::MalformedKey);
        }
        Ok(Self(key))
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey([REDACTED])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AnthropicSetupError {
    /// Names the variable, never a value.
    #[error("environment variable {API_KEY_ENV} is not set or empty")]
    MissingKey,
    #[error("API key contains characters that are not allowed in an HTTP header")]
    MalformedKey,
    #[error("base URL must start with http:// or https://")]
    BadBaseUrl,
}

/// [`Provider`] for the Anthropic Messages API.
pub struct AnthropicProvider {
    agent: ureq::Agent,
    url: String,
    key: ApiKey,
}

impl fmt::Debug for AnthropicProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnthropicProvider")
            .field("url", &self.url)
            .field("key", &self.key)
            .finish()
    }
}

impl AnthropicProvider {
    pub fn new(key: ApiKey, config: AnthropicConfig) -> Result<Self, AnthropicSetupError> {
        let base = config.base_url.trim_end_matches('/');
        if !(base.starts_with("https://") || base.starts_with("http://")) {
            return Err(AnthropicSetupError::BadBaseUrl);
        }
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(config.connect_timeout)
            .timeout(config.request_timeout)
            .redirects(0)
            .user_agent(concat!("laoban/", env!("CARGO_PKG_VERSION")))
            .build();
        Ok(Self {
            agent,
            url: format!("{base}/v1/messages"),
            key,
        })
    }

    /// Key from `ANTHROPIC_API_KEY`, default endpoint and timeouts.
    pub fn from_env() -> Result<Self, AnthropicSetupError> {
        let key = std::env::var(API_KEY_ENV).map_err(|_| AnthropicSetupError::MissingKey)?;
        Self::new(ApiKey::new(key)?, AnthropicConfig::default())
    }
}

/// Request body for `POST /v1/messages`.
pub fn request_body(req: &ProviderRequest) -> serde_json::Value {
    let messages: Vec<serde_json::Value> = req
        .messages
        .iter()
        .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
        .collect();
    let mut body = serde_json::json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "messages": messages,
    });
    if !req.system.is_empty() {
        body["system"] = serde_json::Value::String(req.system.clone());
    }
    body
}

#[derive(Deserialize)]
struct ApiResponse {
    #[serde(default)]
    content: Vec<ContentBlock>,
    usage: Usage,
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
}

/// Map a successful response body.
pub fn parse_response(body: &str) -> Result<ProviderResponse, ProviderError> {
    let parsed: ApiResponse = serde_json::from_str(body).map_err(|e| ProviderError::Other {
        status: Some(200),
        error_type: "invalid_response".into(),
        message: format!("response is not a Messages API object: {e}"),
    })?;
    let content: String = parsed
        .content
        .iter()
        .filter(|b| b.kind == "text")
        .filter_map(|b| b.text.as_deref())
        .collect();
    Ok(ProviderResponse {
        content,
        input_tokens: parsed.usage.input_tokens,
        output_tokens: parsed.usage.output_tokens,
        stop_reason: StopReason::from_api(parsed.stop_reason.as_deref().unwrap_or("")),
    })
}

/// Classify an error response from its status, `retry-after` header and
/// body (`{"type":"error","error":{"type","message","details":{"error_code"}}}`).
pub fn classify_response(status: u16, retry_after: Option<&str>, body: &str) -> ProviderError {
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    let err = &v["error"];
    classify(&ApiErrorInfo {
        status,
        error_type: err["type"].as_str().unwrap_or(""),
        message: err["message"].as_str().unwrap_or(""),
        error_code: err["details"]["error_code"].as_str(),
        retry_after: retry_after.and_then(parse_retry_after),
    })
}

/// `retry-after` in seconds. HTTP dates are ignored (the API sends seconds).
fn parse_retry_after(v: &str) -> Option<Duration> {
    let v = v.trim();
    if let Ok(secs) = v.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    v.parse::<f64>()
        .ok()
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(Duration::from_secs_f64)
}

impl Provider for AnthropicProvider {
    fn send(&self, req: &ProviderRequest) -> Result<ProviderResponse, ProviderError> {
        let body = request_body(req).to_string();
        let result = self
            .agent
            .post(&self.url)
            .set("x-api-key", self.key.expose())
            .set("anthropic-version", ANTHROPIC_VERSION)
            .set("content-type", "application/json")
            .send_string(&body);
        match result {
            Ok(resp) => {
                let text = resp.into_string().map_err(|e| ProviderError::Other {
                    status: Some(200),
                    error_type: "invalid_response".into(),
                    message: format!("cannot read response body: {}", e.kind()),
                })?;
                parse_response(&text)
            }
            Err(ureq::Error::Status(status, resp)) => {
                let retry_after = resp.header("retry-after").map(str::to_string);
                let mut text = String::new();
                let _ = std::io::Read::read_to_string(
                    &mut std::io::Read::take(resp.into_reader(), MAX_ERROR_BODY),
                    &mut text,
                );
                Err(classify_response(status, retry_after.as_deref(), &text))
            }
            // Only the error kind: the transport error's own text could
            // grow to include request details in future ureq versions.
            Err(ureq::Error::Transport(t)) => Err(ProviderError::Other {
                status: None,
                error_type: "transport".into(),
                message: format!("{:?}", t.kind()),
            }),
        }
    }
}
