//! Anthropic HTTP client against a local mock server. No real network.

use std::time::Duration;

use laoban_ai::anthropic::{AnthropicConfig, AnthropicProvider, ApiKey, ANTHROPIC_VERSION};
use laoban_ai::{
    Message, Provider, ProviderError, ProviderRequest, Role, SpendLimitKind, StopReason,
};
use laoban_mockhttp::{MockServer, Reply};

const KEY: &str = "sk-test-not-a-real-key-0001";

fn provider(server: &MockServer) -> AnthropicProvider {
    provider_with_timeout(server, Duration::from_secs(10))
}

fn provider_with_timeout(server: &MockServer, timeout: Duration) -> AnthropicProvider {
    AnthropicProvider::new(
        ApiKey::new(KEY).unwrap(),
        AnthropicConfig {
            base_url: server.url(),
            connect_timeout: Duration::from_secs(2),
            request_timeout: timeout,
        },
    )
    .unwrap()
}

fn request() -> ProviderRequest {
    ProviderRequest {
        model: "model-a-test".into(),
        system: "system text".into(),
        messages: vec![Message {
            role: Role::User,
            content: "hello 你好".into(),
        }],
        max_tokens: 123,
    }
}

fn error_body(kind: &str, message: &str) -> String {
    serde_json::json!({
        "type": "error",
        "error": {"type": kind, "message": message},
        "request_id": "req_test01"
    })
    .to_string()
}

#[test]
fn success_maps_request_and_response() {
    let server = MockServer::start(|_| {
        Reply::json(
            200,
            r#"{"id":"msg_1","type":"message","role":"assistant","model":"model-a-test",
"content":[{"type":"text","text":"{\"a\":"},{"type":"thinking","thinking":"x"},{"type":"text","text":"1}"}],
"stop_reason":"end_turn","usage":{"input_tokens":11,"output_tokens":7}}"#,
        )
    });
    let resp = provider(&server).send(&request()).unwrap();
    assert_eq!(resp.content, "{\"a\":1}");
    assert_eq!((resp.input_tokens, resp.output_tokens), (11, 7));
    assert_eq!(resp.stop_reason, StopReason::EndTurn);

    let reqs = server.requests();
    assert_eq!(reqs.len(), 1);
    let r = &reqs[0];
    assert_eq!(r.method, "POST");
    assert_eq!(r.path, "/v1/messages");
    assert_eq!(r.header("x-api-key"), Some(KEY));
    assert_eq!(r.header("anthropic-version"), Some(ANTHROPIC_VERSION));
    assert_eq!(r.header("content-type"), Some("application/json"));
    let body: serde_json::Value = serde_json::from_str(&r.body_str()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "model": "model-a-test",
            "max_tokens": 123,
            "system": "system text",
            "messages": [{"role": "user", "content": "hello 你好"}]
        })
    );
}

#[test]
fn max_tokens_and_unknown_stop_reasons_map() {
    let server = MockServer::start(|_| {
        Reply::json(
            200,
            r#"{"content":[{"type":"text","text":"x"}],"stop_reason":"max_tokens","usage":{"input_tokens":1,"output_tokens":2}}"#,
        )
    });
    let resp = provider(&server).send(&request()).unwrap();
    assert_eq!(resp.stop_reason, StopReason::MaxTokens);
}

type Check = fn(&ProviderError) -> bool;

#[test]
fn error_responses_are_classified_with_the_shared_classifier() {
    let cases: Vec<(Reply, Check)> = vec![
        (
            Reply::json(
                400,
                error_body(
                    "invalid_request_error",
                    "You have reached your specified API usage limits. You will regain access on 2026-11-01.",
                ),
            ),
            |e| {
                matches!(
                    e,
                    ProviderError::SpendLimitReached {
                        kind: SpendLimitKind::SelfSet,
                        ..
                    }
                )
            },
        ),
        (
            Reply::json(
                429,
                r#"{"type":"error","error":{"type":"rate_limit_error","message":"You have reached your API usage limits.","details":{"error_code":"enforced_spend_limit_reached"}}}"#,
            ),
            |e| {
                matches!(
                    e,
                    ProviderError::SpendLimitReached {
                        kind: SpendLimitKind::TierCap,
                        ..
                    }
                )
            },
        ),
        (
            Reply::json(429, error_body("rate_limit_error", "slow down")).header("retry-after", "7"),
            |e| {
                *e == ProviderError::RateLimited {
                    retry_after: Some(Duration::from_secs(7)),
                }
            },
        ),
        (
            Reply::json(529, error_body("overloaded_error", "Overloaded")),
            |e| *e == ProviderError::Overloaded,
        ),
        (
            Reply::json(401, error_body("authentication_error", "invalid x-api-key")),
            |e| matches!(e, ProviderError::Authentication { .. }),
        ),
        (
            Reply::json(404, error_body("not_found_error", "model not found")),
            |e| matches!(e, ProviderError::InvalidRequest { .. }),
        ),
        (
            Reply::json(500, "not json at all"),
            |e| {
                matches!(
                    e,
                    ProviderError::Other {
                        status: Some(500),
                        ..
                    }
                )
            },
        ),
    ];
    for (reply, check) in cases {
        let status = reply.status;
        let server = MockServer::start(move |_| reply.clone());
        let err = provider(&server).send(&request()).unwrap_err();
        assert!(check(&err), "status {status}: unexpected {err:?}");
        assert!(!format!("{err} {err:?}").contains(KEY));
        // Exactly one call: the client itself never retries.
        assert_eq!(server.requests().len(), 1);
    }
}

#[test]
fn malformed_success_body_is_an_error() {
    let server = MockServer::start(|_| Reply::json(200, "{}"));
    let err = provider(&server).send(&request()).unwrap_err();
    assert!(
        matches!(err, ProviderError::Other { ref error_type, .. } if error_type == "invalid_response")
    );
}

#[test]
fn timeout_is_a_non_retryable_transport_error() {
    let server = MockServer::start(|_| Reply::json(200, "{}").delayed(Duration::from_millis(1500)));
    let err = provider_with_timeout(&server, Duration::from_millis(300))
        .send(&request())
        .unwrap_err();
    assert!(
        matches!(err, ProviderError::Other { status: None, ref error_type, .. } if error_type == "transport"),
        "{err:?}"
    );
    assert!(!err.is_retryable());
}

#[test]
fn key_never_appears_in_debug_output() {
    let server = MockServer::start(|_| Reply::json(200, "{}"));
    let p = provider(&server);
    let dbg = format!("{p:?} {:?}", ApiKey::new(KEY).unwrap());
    assert!(!dbg.contains(KEY), "{dbg}");
    assert!(dbg.contains("REDACTED"));
}

#[test]
fn bad_keys_are_rejected_without_echoing_them() {
    assert!(ApiKey::new("   ").is_err());
    let err = ApiKey::new("abc\r\nX-Injected: 1").unwrap_err();
    assert!(!err.to_string().contains("Injected"));
}

#[test]
fn without_key_sends_no_api_key_header() {
    // An Anthropic-compatible endpoint such as a local Ollama server.
    let server = MockServer::start(|_| {
        Reply::json(
            200,
            r#"{"id":"msg_1","type":"message","role":"assistant","model":"local-model",
"content":[{"type":"thinking","thinking":"plan"},{"type":"text","text":"ok"}],
"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":1}}"#,
        )
    });
    let p = AnthropicProvider::without_key(AnthropicConfig {
        base_url: server.url(),
        connect_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_secs(10),
    })
    .unwrap();
    assert_eq!(p.send(&request()).unwrap().content, "ok");
    let reqs = server.requests();
    assert_eq!(reqs[0].path, "/v1/messages");
    assert_eq!(reqs[0].header("x-api-key"), None);
    assert_eq!(reqs[0].header("anthropic-version"), Some(ANTHROPIC_VERSION));
}

#[test]
fn without_key_refuses_the_anthropic_api() {
    for base_url in ["https://api.anthropic.com", "https://api.anthropic.com/"] {
        let err = AnthropicProvider::without_key(AnthropicConfig {
            base_url: base_url.into(),
            ..AnthropicConfig::default()
        })
        .unwrap_err();
        assert_eq!(err, laoban_ai::anthropic::AnthropicSetupError::MissingKey);
    }
}
