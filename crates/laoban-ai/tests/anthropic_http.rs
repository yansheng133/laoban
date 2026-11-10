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
        advisor: None,
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

fn with_advisor() -> ProviderRequest {
    let mut r = request();
    r.advisor = Some(laoban_ai::Advisor {
        model: "model-b-advisor".into(),
        max_uses: 1,
        max_tokens: 8000,
    });
    r
}

#[test]
fn advisor_tool_and_beta_header_are_sent_and_its_usage_read() {
    let server = MockServer::start(|_| {
        Reply::json(
            200,
            r#"{"content":[{"type":"server_tool_use","id":"srvtoolu_1","name":"advisor","input":{}},
{"type":"advisor_tool_result","tool_use_id":"srvtoolu_1","content":{"type":"advisor_redacted_result","encrypted_content":"abc","stop_reason":"end_turn"}},
{"type":"text","text":"answer"}],
"stop_reason":"end_turn","usage":{"input_tokens":30,"output_tokens":20,"iterations":[
{"type":"message","input_tokens":10,"output_tokens":5},
{"type":"advisor_message","model":"model-b-advisor","input_tokens":40,"output_tokens":60},
{"type":"message","input_tokens":20,"output_tokens":15}]}}"#,
        )
    });
    let resp = provider(&server).send(&with_advisor()).unwrap();
    // Encrypted advice is not part of the answer.
    assert_eq!(resp.content, "answer");
    assert_eq!((resp.input_tokens, resp.output_tokens), (30, 20));
    let a = resp.advisor.unwrap();
    assert_eq!(
        (
            a.model.as_str(),
            a.consultations,
            a.input_tokens,
            a.output_tokens
        ),
        ("model-b-advisor", 1, 40, 60)
    );

    let r = &server.requests()[0];
    assert_eq!(
        r.header("anthropic-beta"),
        Some(laoban_ai::anthropic::ADVISOR_BETA)
    );
    let body: serde_json::Value = serde_json::from_str(&r.body_str()).unwrap();
    assert_eq!(
        body["tools"],
        serde_json::json!([{"type": "advisor_20260301", "name": "advisor", "model": "model-b-advisor", "max_uses": 1, "max_tokens": 8000}])
    );
}

#[test]
fn without_advisor_no_tool_and_no_beta_header() {
    let server = MockServer::start(|_| {
        Reply::json(
            200,
            r#"{"content":[{"type":"text","text":"x"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#,
        )
    });
    let resp = provider(&server).send(&request()).unwrap();
    assert!(resp.advisor.is_none());
    let r = &server.requests()[0];
    assert_eq!(r.header("anthropic-beta"), None);
    let body: serde_json::Value = serde_json::from_str(&r.body_str()).unwrap();
    assert!(body.get("tools").is_none());
}

#[test]
fn a_turn_paused_for_the_advisor_is_resumed_with_its_content() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    N.store(0, Ordering::SeqCst);
    let server = MockServer::start(|_| {
        if N.fetch_add(1, Ordering::SeqCst) == 0 {
            Reply::json(
                200,
                r#"{"content":[{"type":"text","text":"part one, "},{"type":"server_tool_use","id":"srvtoolu_9","name":"advisor","input":{}}],
"stop_reason":"pause_turn","usage":{"input_tokens":5,"output_tokens":3}}"#,
            )
        } else {
            Reply::json(
                200,
                r#"{"content":[{"type":"advisor_tool_result","tool_use_id":"srvtoolu_9","content":{"type":"advisor_redacted_result","encrypted_content":"z","stop_reason":"end_turn"}},{"type":"text","text":"part two"}],
"stop_reason":"end_turn","usage":{"input_tokens":7,"output_tokens":4,"iterations":[{"type":"advisor_message","model":"model-b-advisor","input_tokens":9,"output_tokens":11}]}}"#,
            )
        }
    });
    let resp = provider(&server).send(&with_advisor()).unwrap();
    assert_eq!(resp.content, "part one, part two");
    assert_eq!((resp.input_tokens, resp.output_tokens), (12, 7));
    assert_eq!(resp.advisor.unwrap().output_tokens, 11);
    assert_eq!(resp.stop_reason, StopReason::EndTurn);

    let reqs = server.requests();
    assert_eq!(reqs.len(), 2);
    let second: serde_json::Value = serde_json::from_str(&reqs[1].body_str()).unwrap();
    let msgs = second["messages"].as_array().unwrap();
    assert_eq!(
        msgs.len(),
        2,
        "the user message, then the paused assistant content"
    );
    assert_eq!(msgs[1]["role"], "assistant");
    assert_eq!(msgs[1]["content"][1]["type"], "server_tool_use");
    assert_eq!(
        second["tools"][0]["type"], "advisor_20260301",
        "the tool stays while a call is pending"
    );
}

const PAUSED: &str = r#"{"content":[{"type":"text","text":"part, "},{"type":"server_tool_use","id":"srvtoolu_9","name":"advisor","input":{}}],
"stop_reason":"pause_turn","usage":{"input_tokens":5,"output_tokens":3,"iterations":[{"type":"advisor_message","model":"model-b-advisor","input_tokens":2,"output_tokens":4}]}}"#;

/// Review H1: a resume that fails still reports what the parts before it used.
#[test]
fn a_failed_resume_reports_the_used_tokens_as_unfinished() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    N.store(0, Ordering::SeqCst);
    let server = MockServer::start(|_| {
        if N.fetch_add(1, Ordering::SeqCst) == 0 {
            Reply::json(200, PAUSED)
        } else {
            Reply::json(529, error_body("overloaded_error", "Overloaded"))
        }
    });
    let resp = provider(&server).send(&with_advisor()).unwrap();
    assert_eq!(resp.stop_reason, StopReason::Incomplete);
    assert_eq!((resp.input_tokens, resp.output_tokens), (5, 3));
    assert_eq!(resp.advisor.unwrap().output_tokens, 4);
}

/// Review H1: still paused after the last resume is unfinished, not success.
#[test]
fn a_turn_still_paused_after_the_last_resume_is_unfinished() {
    let server = MockServer::start(|_| Reply::json(200, PAUSED));
    let resp = provider(&server).send(&with_advisor()).unwrap();
    assert_eq!(resp.stop_reason, StopReason::Incomplete);
    assert_eq!(
        server.requests().len(),
        5,
        "the first request and 4 resumes"
    );
    assert_eq!(resp.output_tokens, 15);
}
