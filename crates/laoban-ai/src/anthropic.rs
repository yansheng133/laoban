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
use crate::provider::{AdvisorUsage, Provider, ProviderRequest, ProviderResponse, StopReason};

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
    /// `None` only for a compatible endpoint that does not check keys
    /// (see [`AnthropicProvider::without_key`]); no `x-api-key` is sent.
    key: Option<ApiKey>,
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
        Self::build(Some(key), config)
    }

    /// For an Anthropic Messages API compatible endpoint that does not check
    /// keys, such as a local Ollama server. Refuses the Anthropic API itself,
    /// which always needs a key.
    pub fn without_key(config: AnthropicConfig) -> Result<Self, AnthropicSetupError> {
        if config.base_url.trim_end_matches('/') == DEFAULT_BASE_URL {
            return Err(AnthropicSetupError::MissingKey);
        }
        Self::build(None, config)
    }

    fn build(key: Option<ApiKey>, config: AnthropicConfig) -> Result<Self, AnthropicSetupError> {
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

/// Beta header of the advisor tool.
pub const ADVISOR_BETA: &str = "advisor-tool-2026-03-01";
/// Type of the advisor tool definition.
pub const ADVISOR_TOOL: &str = "advisor_20260301";
/// Times a turn paused for a pending advisor call is resumed.
const MAX_RESUMES: usize = 4;

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
    if let Some(a) = &req.advisor {
        body["tools"] = serde_json::json!([{
            "type": ADVISOR_TOOL,
            "name": "advisor",
            "model": a.model,
            "max_uses": a.max_uses,
            "max_tokens": a.max_tokens,
        }]);
    }
    body
}

#[derive(Deserialize)]
struct ApiResponse {
    #[serde(default)]
    content: Vec<serde_json::Value>,
    usage: Usage,
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
    /// Per model call of the turn; advisor calls are only here, not in the
    /// top-level totals (they are billed at the advisor model's rates).
    #[serde(default)]
    iterations: Vec<Iteration>,
}

#[derive(Deserialize)]
struct Iteration {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

/// One response of the Messages API, before turns are joined.
struct Turn {
    text: String,
    /// The content blocks as sent, to resume a paused turn with them.
    content: Vec<serde_json::Value>,
    input_tokens: u64,
    output_tokens: u64,
    advisor: Option<AdvisorUsage>,
    stop_reason: String,
}

fn parse_turn(body: &str) -> Result<Turn, ProviderError> {
    let parsed: ApiResponse = serde_json::from_str(body).map_err(|e| ProviderError::Other {
        status: Some(200),
        error_type: "invalid_response".into(),
        message: format!("response is not a Messages API object: {e}"),
    })?;
    // Text blocks only: advisor calls and results (encrypted or not) are
    // not part of the answer.
    let text: String = parsed
        .content
        .iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect();
    let mut advisor: Option<AdvisorUsage> = None;
    for it in parsed
        .usage
        .iterations
        .iter()
        .filter(|i| i.kind == "advisor_message")
    {
        let a = advisor.get_or_insert_with(|| AdvisorUsage {
            model: it.model.clone().unwrap_or_default(),
            consultations: 0,
            input_tokens: 0,
            output_tokens: 0,
        });
        a.consultations += 1;
        a.input_tokens += it.input_tokens;
        a.output_tokens += it.output_tokens;
    }
    Ok(Turn {
        text,
        content: parsed.content,
        input_tokens: parsed.usage.input_tokens,
        output_tokens: parsed.usage.output_tokens,
        advisor,
        stop_reason: parsed.stop_reason.unwrap_or_default(),
    })
}

/// Map a successful response body.
pub fn parse_response(body: &str) -> Result<ProviderResponse, ProviderError> {
    let t = parse_turn(body)?;
    Ok(ProviderResponse {
        advisor: t.advisor,
        content: t.text,
        input_tokens: t.input_tokens,
        output_tokens: t.output_tokens,
        stop_reason: StopReason::from_api(&t.stop_reason),
    })
}

/// Add `next` to `total`: a paused turn and its continuations are one answer.
fn join_advisor(total: &mut Option<AdvisorUsage>, next: Option<AdvisorUsage>) {
    match (total.as_mut(), next) {
        (Some(t), Some(n)) => {
            t.consultations += n.consultations;
            t.input_tokens += n.input_tokens;
            t.output_tokens += n.output_tokens;
        }
        (None, Some(n)) => *total = Some(n),
        _ => {}
    }
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

impl AnthropicProvider {
    /// One `POST /v1/messages`; the body text of a 2xx answer.
    fn post(&self, body: &str, advisor: bool) -> Result<String, ProviderError> {
        let mut request = self.agent.post(&self.url);
        if let Some(key) = &self.key {
            request = request.set("x-api-key", key.expose());
        }
        if advisor {
            request = request.set("anthropic-beta", ADVISOR_BETA);
        }
        let result = request
            .set("anthropic-version", ANTHROPIC_VERSION)
            .set("content-type", "application/json")
            .send_string(body);
        match result {
            Ok(resp) => resp.into_string().map_err(|e| ProviderError::Other {
                status: Some(200),
                error_type: "invalid_response".into(),
                message: format!("cannot read response body: {}", e.kind()),
            }),
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

impl Provider for AnthropicProvider {
    /// Send the request. A turn that pauses for a pending advisor call
    /// (`pause_turn`) is resumed with its content as sent, unchanged, and
    /// the same tool; text and usage of all parts add up.
    fn send(&self, req: &ProviderRequest) -> Result<ProviderResponse, ProviderError> {
        let mut body = request_body(req);
        let mut text = String::new();
        let (mut input_tokens, mut output_tokens) = (0, 0);
        let mut advisor = None;
        for resume in 0..=MAX_RESUMES {
            let turn = match self
                .post(&body.to_string(), req.advisor.is_some())
                .and_then(|t| parse_turn(&t))
            {
                Ok(t) => t,
                // The first part failed: nothing was used, a normal error
                // (retried by the orchestrator when it can be).
                Err(e) if resume == 0 => return Err(e),
                // A resume failed: the parts before it were billed, so they
                // are reported (review H1), as an unfinished answer.
                Err(_) => {
                    return Ok(ProviderResponse {
                        advisor,
                        content: text,
                        input_tokens,
                        output_tokens,
                        stop_reason: StopReason::Incomplete,
                    })
                }
            };
            text.push_str(&turn.text);
            input_tokens += turn.input_tokens;
            output_tokens += turn.output_tokens;
            join_advisor(&mut advisor, turn.advisor);
            if turn.stop_reason != "pause_turn" || resume == MAX_RESUMES {
                let stop_reason = if turn.stop_reason == "pause_turn" {
                    StopReason::Incomplete
                } else {
                    StopReason::from_api(&turn.stop_reason)
                };
                return Ok(ProviderResponse {
                    advisor,
                    content: text,
                    input_tokens,
                    output_tokens,
                    stop_reason,
                });
            }
            // The assistant's content goes back as it came; a second pause
            // continues the same assistant message.
            let messages = body["messages"]
                .as_array_mut()
                .expect("messages is an array");
            match messages.last_mut() {
                Some(last) if last["role"] == "assistant" => {
                    if let Some(c) = last["content"].as_array_mut() {
                        c.extend(turn.content);
                    }
                }
                _ => messages
                    .push(serde_json::json!({ "role": "assistant", "content": turn.content })),
            }
        }
        unreachable!("the loop returns on its last round")
    }
}
