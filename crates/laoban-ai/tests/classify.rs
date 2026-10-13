use std::time::Duration;

use laoban_ai::{
    backoff_delay, classify, ApiErrorInfo, ProviderError, RetryPolicy, SpendLimitKind,
};

#[derive(Debug, PartialEq)]
enum Expect {
    RateLimited(Option<u64>),
    Overloaded,
    Spend(SpendLimitKind),
    Invalid,
    Auth,
    Other,
}

fn kind_of(e: &ProviderError) -> Expect {
    match e {
        ProviderError::RateLimited { retry_after } => {
            Expect::RateLimited(retry_after.map(|d| d.as_secs()))
        }
        ProviderError::Overloaded => Expect::Overloaded,
        ProviderError::SpendLimitReached { kind, .. } => Expect::Spend(*kind),
        ProviderError::InvalidRequest { .. } => Expect::Invalid,
        ProviderError::Authentication { .. } => Expect::Auth,
        ProviderError::Other { .. } => Expect::Other,
    }
}

/// (status, error.type, message, error.details.error_code, retry-after secs, expected)
type Row = (
    u16,
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<u64>,
    Expect,
);

#[test]
fn classification_table() {
    // (status, error.type, message, error.details.error_code, retry-after secs, expected)
    let table: &[Row] = &[
        // Ordinary rate limit with retry-after.
        (429, "rate_limit_error", "Number of request tokens has exceeded your per-minute rate limit", None, Some(12), Expect::RateLimited(Some(12))),
        // Acceleration limit, no retry-after, not a spend message.
        (429, "rate_limit_error", "Rate limited", None, None, Expect::RateLimited(None)),
        // Tier spend cap: same status and type, identified by error_code.
        (429, "rate_limit_error", "You have reached your API usage limits: your organization has crossed its monthly API usage threshold, set based on your organization's API tier. You will regain access on 2026-11-01 at 00:00 UTC.", Some("enforced_spend_limit_reached"), None, Expect::Spend(SpendLimitKind::TierCap)),
        // Tier spend cap without details: fall back to the documented message.
        (429, "rate_limit_error", "You have reached your API usage limits: your organization has crossed its monthly API usage threshold.", None, None, Expect::Spend(SpendLimitKind::TierCap)),
        // Self-set organization spend limit: HTTP 400 invalid_request_error.
        (400, "invalid_request_error", "You have reached your specified API usage limits. You will regain access on 2026-11-01 at 00:00 UTC.", None, None, Expect::Spend(SpendLimitKind::SelfSet)),
        // Self-set workspace spend limit.
        (400, "invalid_request_error", "You have reached your specified workspace API usage limits. You will regain access on 2026-11-01 at 00:00 UTC.", None, None, Expect::Spend(SpendLimitKind::SelfSet)),
        // Ordinary 400.
        (400, "invalid_request_error", "messages: field required", None, None, Expect::Invalid),
        (404, "not_found_error", "model: not-a-model", None, None, Expect::Invalid),
        (413, "request_too_large", "Request exceeds the maximum allowed number of bytes", None, None, Expect::Invalid),
        (401, "authentication_error", "invalid x-api-key", None, None, Expect::Auth),
        (403, "permission_error", "Your API key does not have permission", None, None, Expect::Auth),
        (529, "overloaded_error", "Overloaded", None, None, Expect::Overloaded),
        (402, "billing_error", "Payment required", None, None, Expect::Other),
        (500, "api_error", "Internal server error", None, None, Expect::Other),
        (504, "timeout_error", "Request timed out", None, None, Expect::Other),
    ];
    for (status, error_type, message, error_code, retry_after, expected) in table {
        let info = ApiErrorInfo {
            status: *status,
            error_type,
            message,
            error_code: *error_code,
            retry_after: retry_after.map(Duration::from_secs),
        };
        let got = classify(&info);
        assert_eq!(
            &kind_of(&got),
            expected,
            "status {status} type {error_type}: {message}"
        );
    }
}

#[test]
fn spend_limit_message_is_kept_for_the_operator() {
    let msg = "You have reached your specified API usage limits. You will regain access on 2026-11-01 at 00:00 UTC.";
    let err = classify(&ApiErrorInfo {
        status: 400,
        error_type: "invalid_request_error",
        message: msg,
        ..ApiErrorInfo::default()
    });
    assert!(err.to_string().contains("2026-11-01"));
}

#[test]
fn only_capacity_errors_are_retryable() {
    let retryable = [
        ProviderError::RateLimited { retry_after: None },
        ProviderError::Overloaded,
    ];
    let not_retryable = [
        ProviderError::SpendLimitReached {
            kind: SpendLimitKind::SelfSet,
            message: String::new(),
        },
        ProviderError::SpendLimitReached {
            kind: SpendLimitKind::TierCap,
            message: String::new(),
        },
        ProviderError::InvalidRequest {
            message: String::new(),
        },
        ProviderError::Authentication {
            message: String::new(),
        },
        ProviderError::Other {
            status: Some(500),
            error_type: "api_error".into(),
            message: String::new(),
        },
    ];
    assert!(retryable.iter().all(ProviderError::is_retryable));
    assert!(not_retryable.iter().all(|e| !e.is_retryable()));
}

#[test]
fn backoff_is_exponential_and_capped() {
    let p = RetryPolicy {
        max_retries: 10,
        base_delay_ms: 500,
        max_delay_ms: 4_000,
    };
    let ms: Vec<u128> = (0..6)
        .map(|i| backoff_delay(&p, i, None).as_millis())
        .collect();
    assert_eq!(ms, vec![500, 1_000, 2_000, 4_000, 4_000, 4_000]);
    // Huge retry index does not overflow.
    assert_eq!(backoff_delay(&p, u32::MAX, None).as_millis(), 4_000);
}

#[test]
fn retry_after_wins_over_backoff() {
    let p = RetryPolicy::default();
    assert_eq!(
        backoff_delay(&p, 0, Some(Duration::from_secs(7))),
        Duration::from_secs(7)
    );
}
