//! Shared fixtures. All data is synthetic: example-owner repos, fake
//! tokens, model IDs like model-a-test, generated national IDs.
#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use laoban_ai::{MockProvider, ProviderResponse, StopReason};
use laoban_m0::{GithubCommitter, GithubConfig, GithubToken, M0Service, M0Settings};
use laoban_mask::tw_id_checksum_valid;
use laoban_mockhttp::{MockServer, Recorded, Reply};
use serde_json::json;

pub const GH_TOKEN: &str = "github_pat_test_token_0001";
pub const BASE_COMMIT: &str = "1111111111111111111111111111111111111111";
pub const BASE_TREE: &str = "2222222222222222222222222222222222222222";
pub const NEW_TREE: &str = "3333333333333333333333333333333333333333";
pub const NEW_COMMIT: &str = "4444444444444444444444444444444444444444";
pub const MODEL: &str = "model-a-test";

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

pub fn settings(token_cap: u64) -> M0Settings {
    M0Settings {
        model: MODEL.into(),
        token_cap,
        repo_owner: "example-owner".into(),
        repo_name: "laoban-apps".into(),
        target_namespace: "laoban-dev".into(),
        cluster_id: "c-m-test0001".into(),
    }
}

/// How the mock GitHub answers the final ref update.
#[derive(Clone, Copy)]
pub enum RefUpdate {
    Ok,
    Status(u16),
}

/// Mock GitHub implementing the Git Data API calls the committer makes.
pub fn github_mock(update: RefUpdate) -> MockServer {
    let blobs = Arc::new(AtomicUsize::new(0));
    MockServer::start(move |r: &Recorded| {
        let p = r.path.as_str();
        let base = "/repos/example-owner/laoban-apps/git";
        match (r.method.as_str(), p.strip_prefix(base).unwrap_or("")) {
            ("GET", "/ref/heads/main") => Reply::json(
                200,
                json!({"ref": "refs/heads/main", "object": {"type": "commit", "sha": BASE_COMMIT}})
                    .to_string(),
            ),
            ("GET", path) if path == format!("/commits/{BASE_COMMIT}") => Reply::json(
                200,
                json!({"sha": BASE_COMMIT, "tree": {"sha": BASE_TREE}}).to_string(),
            ),
            ("POST", "/blobs") => {
                let n = blobs.fetch_add(1, Ordering::SeqCst) + 1;
                Reply::json(201, json!({"sha": format!("{n:040x}")}).to_string())
            }
            ("POST", "/trees") => Reply::json(201, json!({"sha": NEW_TREE}).to_string()),
            ("POST", "/commits") => Reply::json(201, json!({"sha": NEW_COMMIT}).to_string()),
            ("PATCH", "/refs/heads/main") => match update {
                RefUpdate::Ok => Reply::json(
                    200,
                    json!({"ref": "refs/heads/main", "object": {"sha": NEW_COMMIT}}).to_string(),
                ),
                RefUpdate::Status(s) => Reply::json(
                    s,
                    json!({"message": "Update is not a fast forward", "status": s.to_string()})
                        .to_string(),
                ),
            },
            _ => Reply::json(404, r#"{"message":"Not Found"}"#),
        }
    })
}

pub fn committer(server: &MockServer) -> GithubCommitter {
    let mut cfg = GithubConfig::new("example-owner", "laoban-apps");
    cfg.api_base = server.url();
    GithubCommitter::new(cfg, GithubToken::new(GH_TOKEN).unwrap())
}

pub fn ok(content: &str) -> Result<ProviderResponse, laoban_ai::ProviderError> {
    Ok(ProviderResponse {
        content: content.to_string(),
        input_tokens: 120,
        output_tokens: 80,
        stop_reason: StopReason::EndTurn,
    })
}

pub fn plan_json() -> String {
    json!({
        "appName": "breakfast",
        "summary": "早餐店點餐頁面，可以選餐點和送出訂單。",
        "steps": ["建立點餐頁面", "建立訂單 API"],
        "components": {
            "web": {"modules": [{"name": "menu-view", "purpose": "顯示菜單"}, "order-form"]},
            "api": {
                "modules": [{"name": "orders", "purpose": "訂單", "pos": {"x": 99, "z": 99}}],
                "endpoints": [
                    {"method": "GET", "path": "/menu", "purpose": "取得菜單"},
                    {"method": "post", "path": "/orders", "purpose": "送出訂單"}
                ]
            }
        },
        "notes": "資料只放記憶體。"
    })
    .to_string()
}

pub fn files_value() -> serde_json::Value {
    json!({
        "web": {
            "index.html": "<!doctype html>\n<html><head><link rel=\"stylesheet\" href=\"style.css\"></head>\n<body><script src=\"app.js\"></script></body></html>\n",
            "app.js": "fetch(\"api/menu\").then(r => r.json());\n",
            "style.css": "body { color: #333; }\n"
        },
        "api": {
            "handler.py": "MENU = [\"蛋餅\"]\n\n\ndef handle(method, path, body):\n    if path == \"/menu\":\n        return 200, {\"menu\": MENU}\n    return 404, {\"error\": \"not found\"}\n"
        }
    })
}

pub fn files_json() -> String {
    files_value().to_string()
}

pub struct Harness {
    pub service: M0Service,
    pub provider: Arc<MockProvider>,
    pub github: MockServer,
}

pub fn harness(provider: MockProvider, update: RefUpdate, token_cap: u64) -> Harness {
    let provider = Arc::new(provider);
    let github = github_mock(update);
    let service = M0Service::new(
        settings(token_cap),
        Box::new(Arc::clone(&provider)),
        committer(&github),
    )
    .unwrap();
    Harness {
        service,
        provider,
        github,
    }
}

/// Decoded blob contents by request order (blobs are created in file order).
pub fn blob_contents(github: &MockServer) -> Vec<String> {
    github
        .requests()
        .iter()
        .filter(|r| r.method == "POST" && r.path.ends_with("/git/blobs"))
        .map(|r| {
            let v: serde_json::Value = serde_json::from_str(&r.body_str()).unwrap();
            assert_eq!(v["encoding"], "utf-8");
            v["content"].as_str().unwrap().to_string()
        })
        .collect()
}
