//! Shared fixtures. All data is synthetic: example-owner repos, fake
//! tokens, model IDs like model-a-test, generated national IDs.
#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

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
        ai_timeout_secs: 300,
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
    github_mock_with(update, Vec::new())
}

/// Like [`github_mock`], with existing app directories under `apps/`:
/// `(name, Some(laoban.json content))`, or `None` for a directory without
/// one (an app made before laoban.json existed).
pub fn github_mock_with(update: RefUpdate, apps: Vec<(String, Option<String>)>) -> MockServer {
    github_mock_versions(update, apps, Vec::new())
}

/// One past version: (app, commit SHA, laoban.json at that commit, date).
/// Listed newest first, in the order given.
pub type Version = (String, String, String, String);

/// Like [`github_mock_with`], plus the commits API and contents at a SHA.
pub fn github_mock_versions(
    update: RefUpdate,
    apps: Vec<(String, Option<String>)>,
    versions: Vec<Version>,
) -> MockServer {
    let blobs = Arc::new(AtomicUsize::new(0));
    let commits = Arc::new(AtomicUsize::new(0));
    // An app's laoban.json as committed: a blob holding an app's metadata
    // becomes what later reads return once the branch moves.
    let apps = Arc::new(Mutex::new(apps));
    let pending: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    // The main branch's commits, oldest first: (SHA, apps at that commit).
    // Apps given without explicit versions are at BASE_COMMIT.
    type Snapshot = (String, Vec<(String, Option<String>)>);
    let history: Arc<Mutex<Vec<Snapshot>>> = Arc::new(Mutex::new(vec![(
        BASE_COMMIT.to_string(),
        apps.lock().unwrap().clone(),
    )]));
    // The archive branch: its head and the files written to it.
    let archive: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    // Blob contents by SHA, the last tree's entries, and the archive's files.
    let blob_store: Arc<Mutex<std::collections::HashMap<String, String>>> = Arc::default();
    let last_tree: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let archive_files: Arc<Mutex<std::collections::BTreeMap<String, String>>> = Arc::default();
    MockServer::start(move |r: &Recorded| {
        let p = r.path.as_str();
        let repo = "/repos/example-owner/laoban-apps/";
        if let Some(q) = p.strip_prefix(&format!("{repo}commits?path=")) {
            let Some((path, rest)) = q.split_once('&') else {
                return Reply::json(400, r#"{"message":"bad query"}"#);
            };
            if !rest.starts_with("sha=main&per_page=") {
                return Reply::json(400, r#"{"message":"bad query"}"#);
            }
            let mut list: Vec<serde_json::Value> = versions
                .iter()
                .filter(|(app, _, _, _)| path == format!("apps/{app}/laoban.json"))
                .map(|(app, sha, _, date)| {
                    json!({"sha": sha, "commit": {"message": format!("laoban: {app} — x\n\nGenerated-by: laoban\n"), "committer": {"date": date}}})
                })
                .collect();
            // Commits made here that changed this app's laoban.json, newest
            // first, before the given versions (or BASE_COMMIT without any).
            {
                let history = history.lock().unwrap();
                let skip_base = !list.is_empty();
                let mut last: Option<Option<String>> = None;
                let mut found = Vec::new();
                for (sha, snap) in history.iter() {
                    let meta = snap
                        .iter()
                        .find(|(n, _)| path == format!("apps/{n}/laoban.json"))
                        .and_then(|(_, m)| m.clone());
                    let changed = meta.is_some() && last.as_ref() != Some(&meta);
                    if changed && !(skip_base && sha == BASE_COMMIT) {
                        found.push(json!({"sha": sha, "commit": {"message": "laoban: x\n", "committer": {"date": "2026-10-16T00:00:00Z"}}}));
                    }
                    last = Some(meta);
                }
                found.reverse();
                found.extend(list);
                list = found;
            }
            return Reply::json(200, serde_json::Value::Array(list).to_string());
        }
        let contents = "/repos/example-owner/laoban-apps/contents/";
        if let Some(rest) = p.strip_prefix(contents) {
            if r.method != "GET" {
                return Reply::json(405, r#"{"message":"no"}"#);
            }
            if let Some((path, git_ref)) = rest.split_once("?ref=") {
                if git_ref == "laoban-rounds" {
                    let files = archive_files.lock().unwrap();
                    if let Some(content) = files.get(path) {
                        if r.header("accept") != Some("application/vnd.github.raw+json") {
                            return Reply::json(415, r#"{"message":"want raw"}"#);
                        }
                        return Reply::json(200, content.clone());
                    }
                    let prefix = format!("{path}/");
                    let list: Vec<serde_json::Value> = files
                        .keys()
                        .filter_map(|k| k.strip_prefix(&prefix))
                        .filter(|n| !n.contains('/'))
                        .map(|n| json!({"name": n, "path": format!("{prefix}{n}"), "type": "file"}))
                        .collect();
                    if list.is_empty() {
                        return Reply::json(404, r#"{"message":"Not Found"}"#);
                    }
                    return Reply::json(200, serde_json::Value::Array(list).to_string());
                }
                if git_ref != "main" {
                    if r.header("accept") != Some("application/vnd.github.raw+json") {
                        return Reply::json(415, r#"{"message":"want raw"}"#);
                    }
                    if let Some((_, _, meta, _)) = versions.iter().find(|(app, sha, _, _)| {
                        sha == git_ref && path == format!("apps/{app}/laoban.json")
                    }) {
                        return Reply::json(200, meta.clone());
                    }
                    let history = history.lock().unwrap();
                    let meta =
                        history
                            .iter()
                            .find(|(sha, _)| sha == git_ref)
                            .and_then(|(_, snap)| {
                                snap.iter()
                                    .find(|(n, _)| path == format!("apps/{n}/laoban.json"))
                                    .and_then(|(_, m)| m.clone())
                            });
                    return match meta {
                        Some(m) => Reply::json(200, m),
                        None => Reply::json(404, r#"{"message":"Not Found"}"#),
                    };
                }
            }
            let Some(path) = rest.strip_suffix("?ref=main") else {
                return Reply::json(400, r#"{"message":"missing ref"}"#);
            };
            let apps = apps.lock().unwrap();
            if path == "apps" {
                if apps.is_empty() {
                    return Reply::json(404, r#"{"message":"Not Found"}"#);
                }
                let mut list: Vec<serde_json::Value> = apps
                    .iter()
                    .map(|(n, _)| json!({"name": n, "path": format!("apps/{n}"), "type": "dir"}))
                    .collect();
                list.push(json!({"name": ".gitkeep", "path": "apps/.gitkeep", "type": "file"}));
                return Reply::json(200, serde_json::Value::Array(list).to_string());
            }
            for (n, meta) in apps.iter() {
                if path == format!("apps/{n}/laoban.json") {
                    if r.header("accept") != Some("application/vnd.github.raw+json") {
                        return Reply::json(415, r#"{"message":"want raw"}"#);
                    }
                    return match meta {
                        Some(m) => Reply::json(200, m.clone()),
                        None => Reply::json(404, r#"{"message":"Not Found"}"#),
                    };
                }
            }
            return Reply::json(404, r#"{"message":"Not Found"}"#);
        }
        let base = "/repos/example-owner/laoban-apps/git";
        match (r.method.as_str(), p.strip_prefix(base).unwrap_or("")) {
            ("GET", "/ref/heads/main") => {
                let head = history.lock().unwrap().last().unwrap().0.clone();
                Reply::json(
                    200,
                    json!({"ref": "refs/heads/main", "object": {"type": "commit", "sha": head}})
                        .to_string(),
                )
            }
            ("GET", "/ref/heads/laoban-rounds") => match archive.lock().unwrap().clone() {
                Some(head) => Reply::json(
                    200,
                    json!({"ref": "refs/heads/laoban-rounds", "object": {"sha": head}}).to_string(),
                ),
                None => Reply::json(404, r#"{"message":"Not Found"}"#),
            },
            // A GitHub that refuses moving main refuses the archive too.
            ("POST", "/refs") | ("PATCH", "/refs/heads/laoban-rounds")
                if matches!(update, RefUpdate::Status(_)) =>
            {
                pending.lock().unwrap().clear();
                Reply::json(422, r#"{"message":"refused"}"#)
            }
            ("POST", "/refs") => {
                let v: serde_json::Value = serde_json::from_str(&r.body_str()).unwrap();
                assert_eq!(v["ref"], "refs/heads/laoban-rounds");
                *archive.lock().unwrap() = Some(v["sha"].as_str().unwrap().to_string());
                pending.lock().unwrap().clear();
                let blobs = blob_store.lock().unwrap();
                for (path, sha) in last_tree.lock().unwrap().drain(..) {
                    if let Some(c) = blobs.get(&sha) {
                        archive_files.lock().unwrap().insert(path, c.clone());
                    }
                }
                Reply::json(201, json!({"ref": v["ref"]}).to_string())
            }
            ("PATCH", "/refs/heads/laoban-rounds") => {
                let v: serde_json::Value = serde_json::from_str(&r.body_str()).unwrap();
                *archive.lock().unwrap() = Some(v["sha"].as_str().unwrap().to_string());
                pending.lock().unwrap().clear();
                let blobs = blob_store.lock().unwrap();
                for (path, sha) in last_tree.lock().unwrap().drain(..) {
                    if let Some(c) = blobs.get(&sha) {
                        archive_files.lock().unwrap().insert(path, c.clone());
                    }
                }
                Reply::json(200, "{}")
            }
            ("GET", path) if path.starts_with("/commits/") => Reply::json(
                200,
                json!({"sha": path.trim_start_matches("/commits/"), "tree": {"sha": BASE_TREE}})
                    .to_string(),
            ),
            ("POST", "/blobs") => {
                let n = blobs.fetch_add(1, Ordering::SeqCst) + 1;
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&r.body_str()) {
                    let content = v["content"].as_str().unwrap_or_default();
                    blob_store
                        .lock()
                        .unwrap()
                        .insert(format!("{n:040x}"), content.to_string());
                    if let Ok(m) = serde_json::from_str::<serde_json::Value>(content) {
                        if let (Some(_), Some(app)) = (m.get("schema"), m["appName"].as_str()) {
                            pending
                                .lock()
                                .unwrap()
                                .push((app.to_string(), content.to_string()));
                        }
                    }
                }
                Reply::json(201, json!({"sha": format!("{n:040x}")}).to_string())
            }
            ("POST", "/trees") => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&r.body_str()) {
                    let entries: Vec<(String, String)> = v["tree"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|e| {
                                    Some((
                                        e["path"].as_str()?.to_string(),
                                        e["sha"].as_str()?.to_string(),
                                    ))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    *last_tree.lock().unwrap() = entries;
                }
                Reply::json(201, json!({"sha": NEW_TREE}).to_string())
            }
            ("POST", "/commits") => {
                // The first commit is NEW_COMMIT, later ones count up.
                let n = commits.fetch_add(1, Ordering::SeqCst);
                let sha = if n == 0 {
                    NEW_COMMIT.to_string()
                } else {
                    format!("5{:039x}", n)
                };
                Reply::json(201, json!({ "sha": sha }).to_string())
            }
            ("PATCH", "/refs/heads/main") => match update {
                RefUpdate::Ok => {
                    let v: serde_json::Value = serde_json::from_str(&r.body_str()).unwrap();
                    let sha = v["sha"].as_str().unwrap_or(NEW_COMMIT).to_string();
                    let mut apps = apps.lock().unwrap();
                    for (app, meta) in pending.lock().unwrap().drain(..) {
                        match apps.iter_mut().find(|(n, _)| *n == app) {
                            Some(entry) => entry.1 = Some(meta),
                            None => apps.push((app, Some(meta))),
                        }
                    }
                    history.lock().unwrap().push((sha.clone(), apps.clone()));
                    Reply::json(
                        200,
                        json!({"ref": "refs/heads/main", "object": {"sha": sha}}).to_string(),
                    )
                }
                RefUpdate::Status(s) => {
                    pending.lock().unwrap().clear();
                    Reply::json(
                        s,
                        json!({"message": "Update is not a fast forward", "status": s.to_string()})
                            .to_string(),
                    )
                }
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
    harness_with(provider, update, token_cap, Vec::new())
}

pub fn harness_with(
    provider: MockProvider,
    update: RefUpdate,
    token_cap: u64,
    apps: Vec<(String, Option<String>)>,
) -> Harness {
    harness_versions(provider, update, token_cap, apps, Vec::new())
}

pub fn harness_versions(
    provider: MockProvider,
    update: RefUpdate,
    token_cap: u64,
    apps: Vec<(String, Option<String>)>,
    versions: Vec<Version>,
) -> Harness {
    let provider = Arc::new(provider);
    let github = github_mock_versions(update, apps, versions);
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

/// The service with an AI that waits at a gate before each answer, so a
/// test can see the round while the AI is still working. `open()` lets one
/// call through.
pub struct Gate {
    inner: Arc<MockProvider>,
    passes: Arc<(Mutex<usize>, std::sync::Condvar)>,
}

impl Gate {
    pub fn open(&self) {
        let (n, cv) = &*self.passes;
        *n.lock().unwrap() += 1;
        cv.notify_all();
    }
}

struct GatedProvider {
    inner: Arc<MockProvider>,
    passes: Arc<(Mutex<usize>, std::sync::Condvar)>,
}

impl laoban_ai::Provider for GatedProvider {
    fn send(
        &self,
        req: &laoban_ai::ProviderRequest,
    ) -> Result<ProviderResponse, laoban_ai::ProviderError> {
        let (n, cv) = &*self.passes;
        let mut left = n.lock().unwrap();
        while *left == 0 {
            let (l, timeout) = cv
                .wait_timeout(left, std::time::Duration::from_secs(20))
                .unwrap();
            left = l;
            assert!(!timeout.timed_out() || *left > 0, "gate never opened");
        }
        *left -= 1;
        drop(left);
        self.inner.send(req)
    }
}

pub fn gated_service(
    provider: MockProvider,
    apps: Vec<(String, Option<String>)>,
) -> (Arc<M0Service>, Gate, MockServer) {
    let inner = Arc::new(provider);
    let passes = Arc::new((Mutex::new(0), std::sync::Condvar::new()));
    let github = github_mock_versions(RefUpdate::Ok, apps, Vec::new());
    let service = M0Service::new(
        settings(1_000_000),
        Box::new(GatedProvider {
            inner: Arc::clone(&inner),
            passes: Arc::clone(&passes),
        }),
        committer(&github),
    )
    .unwrap();
    let service = Arc::new(service);
    service.attach();
    (service, Gate { inner, passes }, github)
}

impl Gate {
    pub fn calls(&self) -> usize {
        self.inner.call_count()
    }
}

/// Requests that would change the repository.
pub fn writes(github: &MockServer) -> usize {
    github
        .requests()
        .iter()
        .filter(|r| r.method != "GET")
        .count()
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

/// The verified caller used by the tests (the user name the API server
/// would pass). `app` is ignored: kept so call sites read naturally.
#[allow(dead_code)]
pub fn lock(_h: &Harness, _app: &str) -> String {
    "u-tester".to_string()
}
