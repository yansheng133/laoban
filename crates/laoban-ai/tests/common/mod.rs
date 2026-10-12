//! Shared test fixtures. All data is synthetic: 192.0.2.x addresses,
//! example.test domains, local://u-test01 principals, generated IDs.
#![allow(dead_code)]

use std::sync::Arc;

use laoban_ai::{
    Dependencies, FixedBoundary, FixedPeriod, MemoryAuditSink, MockProvider, ModelSelection,
    Orchestrator, OrchestratorConfig, PlanPolicy, PlanRequest, ProviderResponse, RecordingSleeper,
    RetryPolicy, StopReason, TokenBudget,
};
use laoban_mask::{tw_id_checksum_valid, MaskerConfig};

pub const NONCE: &str = "0123456789abcdef0123456789abcdef";
pub const BOUNDARY: &str = "LAOBAN_DATA_0123456789abcdef0123456789abcdef";
pub const LOGIN: &str = "tester01@example.test";
pub const PRINCIPAL: &str = "local://u-test01";
pub const CLUSTER: &str = "c-test01";
pub const MODEL_A: &str = "model-a-test";
pub const MODEL_B: &str = "model-b-test";

/// Append the check digit that makes `prefix` (letter + 8 characters) valid.
pub fn with_check_digit(prefix: &str) -> String {
    for d in 0..10 {
        let candidate = format!("{prefix}{d}");
        if tw_id_checksum_valid(&candidate) {
            return candidate;
        }
    }
    panic!("no check digit for {prefix}");
}

pub fn plan_json(cluster: &str, kind: &str) -> String {
    format!(
        r#"{{"summary":"Deploy demo web app","steps":["create deployment","create service"],
"files_to_change":[{{"path":"apps/demo/deployment.yaml","action":"create"}}],
"resources":[{{"cluster":"{cluster}","namespace":"demo","api_group":"apps","kind":"{kind}","name":"demo","action":"create"}}]}}"#
    )
}

pub fn ok_response(content: &str) -> ProviderResponse {
    ProviderResponse {
        content: content.to_string(),
        input_tokens: 100,
        output_tokens: 50,
        stop_reason: StopReason::EndTurn,
    }
}

pub fn config() -> OrchestratorConfig {
    OrchestratorConfig {
        masker: MaskerConfig {
            login_accounts: vec![LOGIN.into()],
            ..MaskerConfig::default()
        },
        models: ModelSelection::UserChoice {
            allowed: vec![MODEL_A.into(), MODEL_B.into()],
            default: MODEL_A.into(),
        },
        retry: RetryPolicy {
            max_retries: 2,
            base_delay_ms: 100,
            max_delay_ms: 10_000,
        },
        plan_policy: PlanPolicy::default(),
        system_prompt: "You are laoban's planning assistant.".into(),
        max_tokens: 1_000,
    }
}

pub struct Harness {
    pub orchestrator: Orchestrator,
    pub provider: Arc<MockProvider>,
    pub audit: Arc<MemoryAuditSink>,
    pub sleeper: Arc<RecordingSleeper>,
    pub budget: Arc<TokenBudget>,
}

pub fn harness_with(config: OrchestratorConfig, provider: MockProvider, cap: u64) -> Harness {
    let provider = Arc::new(provider);
    let audit = Arc::new(MemoryAuditSink::default());
    let sleeper = Arc::new(RecordingSleeper::default());
    let budget = TokenBudget::new(cap, FixedPeriod(1));
    let orchestrator = Orchestrator::new(
        config,
        Dependencies {
            provider: Box::new(Arc::clone(&provider)),
            budget: Arc::clone(&budget),
            audit: Box::new(Arc::clone(&audit)),
            sleeper: Box::new(Arc::clone(&sleeper)),
            boundary: Box::new(FixedBoundary(NONCE.into())),
        },
    )
    .unwrap();
    Harness {
        orchestrator,
        provider,
        audit,
        sleeper,
        budget,
    }
}

pub fn harness(provider: MockProvider) -> Harness {
    harness_with(config(), provider, 1_000_000)
}

pub fn request(requirement: &str) -> PlanRequest {
    PlanRequest {
        principal: PRINCIPAL.into(),
        requested_model: None,
        target_cluster: CLUSTER.into(),
        requirement: requirement.into(),
        cluster_state: vec![],
        files: vec![],
    }
}
