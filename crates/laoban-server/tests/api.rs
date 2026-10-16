//! `/api/*` over real sockets, with a mock provider and a mock GitHub.
//! All data is synthetic; no network beyond loopback.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use laoban_ai::{
    MockProvider, Provider, ProviderError, ProviderRequest, ProviderResponse, SpendLimitKind,
    StopReason,
};
use laoban_m0::{GithubCommitter, GithubConfig, GithubToken, M0Service, M0Settings};
use laoban_mockhttp::{MockServer, Recorded, Reply};
use laoban_server::{ApiState, Server, ShutdownHandle, IO_TIMEOUT};
use serde_json::{json, Value};

const COMMIT: &str = "4444444444444444444444444444444444444444";

fn github_mock() -> MockServer {
    let n = Arc::new(AtomicUsize::new(0));
    MockServer::start(move |r: &Recorded| {
        // The repository has no apps/ directory yet.
        if r.path.contains("/contents/") {
            return Reply::json(404, r#"{"message":"Not Found"}"#);
        }
        let i = n.fetch_add(1, Ordering::SeqCst);
        let sha = if r.path.ends_with("/git/commits") {
            COMMIT.to_string()
        } else {
            format!("{:040x}", i + 1)
        };
        let body = match (r.method.as_str(), r.path.as_str()) {
            ("GET", p) if p.ends_with("/git/ref/heads/main") => json!({"object": {"sha": sha}}),
            ("GET", _) => json!({"tree": {"sha": sha}}),
            _ => json!({"sha": sha}),
        };
        Reply::json(200, body.to_string())
    })
}

fn settings(cap: u64) -> M0Settings {
    M0Settings {
        model: "model-a-test".into(),
        token_cap: cap,
        repo_owner: "example-owner".into(),
        repo_name: "laoban-apps".into(),
        target_namespace: "laoban-dev".into(),
        cluster_id: "c-m-test0001".into(),
        ai_timeout_secs: 300,
    }
}

fn ok(content: &str) -> Result<ProviderResponse, ProviderError> {
    Ok(ProviderResponse {
        content: content.into(),
        input_tokens: 10,
        output_tokens: 20,
        stop_reason: StopReason::EndTurn,
    })
}

fn plan_json() -> String {
    json!({
        "appName": "breakfast", "summary": "早餐店點餐", "steps": ["建立頁面"],
        "components": {"web": {"modules": []}, "api": {"modules": [], "endpoints": [{"method": "GET", "path": "/menu", "purpose": "菜單"}]}},
        "notes": ""
    })
    .to_string()
}

fn files_json() -> String {
    json!({
        "web": {"index.html": "<p>hi</p>\n", "app.js": "fetch(\"api/menu\");\n", "style.css": "p{}\n"},
        "api": {"handler.py": "def handle(m, p, b):\n    return 200, {}\n"}
    })
    .to_string()
}

struct Running {
    addr: SocketAddr,
    handle: ShutdownHandle,
    thread: Option<thread::JoinHandle<std::io::Result<()>>>,
    _github: Option<MockServer>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.handle.shutdown();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn start(provider: Box<dyn Provider>, cap: u64) -> Running {
    let github = github_mock();
    let mut cfg = GithubConfig::new("example-owner", "laoban-apps");
    cfg.api_base = github.url();
    let committer = GithubCommitter::new(cfg, GithubToken::new("github_pat_test_0001").unwrap());
    let service = M0Service::new(settings(cap), provider, committer).unwrap();
    start_with(ApiState::Ready(Arc::new(service)), Some(github))
}

fn start_with(api: ApiState, github: Option<MockServer>) -> Running {
    let server = Server::bind_with_api("127.0.0.1:0", api)
        .unwrap()
        .insecure_plain_api_for_tests();
    let addr = server.local_addr();
    let handle = server.shutdown_handle();
    let thread = Some(thread::spawn(move || server.run()));
    Running {
        addr,
        handle,
        thread,
        _github: github,
    }
}

fn raw(addr: SocketAddr, req: &[u8]) -> (u16, String, String) {
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.write_all(req).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let (head, body) = out.split_once("\r\n\r\n").expect("response head");
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, head.to_string(), body.to_string())
}

fn post(addr: SocketAddr, path: &str, body: &str) -> (u16, Value) {
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let (status, head, body) = raw(addr, req.as_bytes());
    assert!(head.contains("Content-Type: application/json"), "{head}");
    (status, serde_json::from_str(&body).unwrap())
}

fn post_as(addr: SocketAddr, path: &str, body: &str, user: &str) -> (u16, Value) {
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: x\r\nX-Remote-User: {user}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let (status, _, body) = raw(addr, req.as_bytes());
    (status, serde_json::from_str(&body).unwrap())
}

fn get(addr: SocketAddr, path: &str) -> (u16, String) {
    let (status, _, body) = raw(
        addr,
        format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes(),
    );
    (status, body)
}

fn assert_error(v: &Value, code: &str) {
    assert_eq!(v["error"]["code"], code, "{v}");
    let msg = v["error"]["message"].as_str().unwrap();
    assert!(!msg.is_empty());
    // Traditional Chinese message: at least one CJK character.
    assert!(
        msg.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
        "{msg}"
    );
    // Also a Kubernetes Status, so kubectl prints the message.
    assert_eq!(v["kind"], "Status", "{v}");
    assert_eq!(v["status"], "Failure");
    assert_eq!(v["message"], msg);
    assert!(v["reason"].as_str().is_some_and(|r| !r.is_empty()));
    assert!(v["code"].as_u64().is_some_and(|c| c >= 400));
}

#[test]
fn plan_apply_and_errors_over_http() {
    let provider = MockProvider::new()
        .then(ok(&plan_json()))
        .then(ok(&files_json()));
    let r = start(Box::new(provider), 1_000_000);

    let (status, body) = get(r.addr, "/api/config");
    assert_eq!(status, 200);
    let cfg: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(cfg["repo"], "example-owner/laoban-apps");
    assert_eq!(cfg["tokenBudget"]["used"], 0);

    let (status, plan) = post(r.addr, "/api/plan", r#"{"prompt":"做一個早餐店點餐頁面"}"#);
    assert_eq!(status, 200, "{plan}");
    let id = plan["planId"].as_str().unwrap().to_string();
    assert_eq!(plan["appName"], "breakfast");

    let apply_body = json!({ "planId": id }).to_string();
    let (status, applied) = post(r.addr, "/api/apply", &apply_body);
    assert_eq!(status, 200, "{applied}");
    assert_eq!(applied["commit"], COMMIT);
    assert_eq!(
        applied["appUrl"],
        "/k8s/clusters/c-m-test0001/api/v1/namespaces/laoban-dev/services/http:breakfast-web:8080/proxy/"
    );

    let (status, err) = post(r.addr, "/api/apply", &apply_body);
    assert_eq!(status, 409);
    assert_error(&err, "already_applied");

    let (status, err) = post(r.addr, "/api/apply", r#"{"planId":"p-20260101-000000"}"#);
    assert_eq!(status, 404);
    assert_error(&err, "plan_not_found");

    // Token usage is visible after the calls.
    let cfg: Value = serde_json::from_str(&get(r.addr, "/api/config").1).unwrap();
    assert_eq!(cfg["tokenBudget"]["used"], 60);

    // Probes still behave as before.
    assert_eq!(get(r.addr, "/healthz"), (200, "ok".to_string()));
}

#[test]
fn apps_endpoints() {
    let r = start(Box::new(MockProvider::new()), 1_000_000);
    let (status, body) = get(r.addr, "/api/apps");
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap(),
        json!({"apps": []})
    );

    let (status, body) = get(r.addr, "/api/apps/breakfast");
    assert_eq!(status, 404);
    assert_error(&serde_json::from_str(&body).unwrap(), "app_not_found");

    let (status, body) = get(r.addr, "/api/apps/Bad_Name");
    assert_eq!(status, 400);
    assert_error(&serde_json::from_str(&body).unwrap(), "bad_request");

    for path in ["/api/apps/a/b", "/api/apps/"] {
        let (status, _) = get(r.addr, path);
        assert_eq!(status, 404, "{path}");
    }
    let (status, _) = post(r.addr, "/api/apps", "{}");
    assert_eq!(status, 405);

    // Versions and restore.
    let (status, body) = get(r.addr, "/api/apps/breakfast/versions");
    assert_eq!(status, 404);
    assert_error(&serde_json::from_str(&body).unwrap(), "app_not_found");
    let (status, body) = get(r.addr, "/api/apps/breakfast/versions/nothex");
    assert_eq!(status, 400);
    assert_error(&serde_json::from_str(&body).unwrap(), "bad_request");
    let (status, _) = get(r.addr, "/api/apps/breakfast/restore");
    assert_eq!(status, 405);
    let (status, v) = post(r.addr, "/api/apps/breakfast/restore", r#"{"sha":"x"}"#);
    assert_eq!(status, 400);
    assert_error(&v, "bad_request");
    let (status, _) = get(r.addr, "/api/apps/breakfast/other");
    assert_eq!(status, 404);

    // Rounds of 意見收斂, code and layout.
    let (status, body) = get(r.addr, "/api/apps/breakfast/round");
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["round"],
        Value::Null
    );
    let (status, head, _) = raw(r.addr, b"PUT /api/apps/breakfast/round HTTP/1.1\r\n\r\n");
    assert_eq!(status, 405);
    assert!(head.contains("Allow: GET, POST"));
    let open = r#"{"opinion":"x","deadlineMinutes":5}"#;
    let (status, v) = post(r.addr, "/api/apps/breakfast/round", open);
    assert_eq!(status, 401, "who opens a round is the verified caller");
    assert_error(&v, "unauthenticated");
    let (status, v) = post_as(r.addr, "/api/apps/breakfast/round", open, "u-test01");
    assert_eq!(status, 404);
    assert_error(&v, "app_not_found");
    let (status, v) = post_as(r.addr, "/api/apps/breakfast/round", "{}", "u-test01");
    assert_eq!(status, 400);
    assert_error(&v, "bad_request");
    let (status, v) = post_as(r.addr, "/api/apps/breakfast/round/confirm", "", "u-test01");
    assert_eq!(status, 409);
    assert_error(&v, "round_state");
    // v0.1.16: author edits, deadline, drafts, plan comments, waiting list,
    // archived rounds. No round here: each reaches the service.
    for (path, body) in [
        ("/api/apps/breakfast/round/opinions/o1/withdraw", ""),
        (
            "/api/apps/breakfast/round/opinions/o1/edit",
            r#"{"text":"x"}"#,
        ),
        ("/api/apps/breakfast/round/deadline", r#"{"minutes":0}"#),
        (
            "/api/apps/breakfast/round/plan/steps/1/comments",
            r#"{"text":"x"}"#,
        ),
        ("/api/apps/breakfast/round/plan/comments/c1/support", ""),
    ] {
        let (status, v) = post_as(r.addr, path, body, "u-test01");
        assert_eq!(status, 409, "{path}");
        assert_error(&v, "round_state");
    }
    for path in [
        "/api/apps/breakfast/round/plan/steps/0/comments",
        "/api/apps/breakfast/round/plan/steps/01/comments",
        "/api/apps/breakfast/round/plan/comments/x1/support",
        "/api/apps/breakfast/round/opinions/c1/edit",
    ] {
        let (status, _) = post_as(r.addr, path, r#"{"text":"x"}"#, "u-test01");
        assert_eq!(status, 404, "{path}");
    }
    let (status, v) = post_as(
        r.addr,
        "/api/apps/breakfast/round/deadline",
        r#"{"minutes":"soon"}"#,
        "u-test01",
    );
    assert_eq!(status, 400);
    assert_error(&v, "bad_request");
    let (status, v) = get_as(r.addr, "/api/rounds/mine", "u-test01");
    assert_eq!(status, 200);
    assert_eq!(v["rounds"], json!([]));
    let (status, _) = get(r.addr, "/api/rounds/mine");
    assert_eq!(status, 401);
    let (status, v) = get_as(r.addr, "/api/apps/breakfast/rounds", "u-test01");
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["rounds"], json!([]));
    let (status, v) = get_as(
        r.addr,
        "/api/apps/breakfast/rounds/r-20260101-000000",
        "u-test01",
    );
    assert_eq!(status, 404);
    assert_error(&v, "round_not_found");
    let (status, body) = get(r.addr, "/api/apps/breakfast/files");
    assert_eq!(status, 404);
    assert_error(&serde_json::from_str(&body).unwrap(), "app_not_found");
    let (status, head, _) = raw(r.addr, b"PUT /api/apps/breakfast/files HTTP/1.1\r\n\r\n");
    assert_eq!(status, 405);
    assert!(head.contains("Allow: GET, POST"));
    let (status, v) = post(r.addr, "/api/apps/breakfast/files", r#"{"files":{}}"#);
    assert_eq!(status, 400);
    assert_error(&v, "bad_request");
    let (status, v) = post(
        r.addr,
        "/api/apps/breakfast/layout",
        r#"{"baseCommit":"x","positions":{"web":{"x":1}}}"#,
    );
    assert_eq!(status, 400);
    assert_error(&v, "bad_request");
    // Saving without a verified caller is refused before anything is read.
    let (status, v) = post(
        r.addr,
        "/api/apps/breakfast/layout",
        r#"{"baseCommit":"x","positions":{"web":{"x":1,"z":2}}}"#,
    );
    assert_eq!(status, 401);
    assert_error(&v, "unauthenticated");
    let (status, v) = post_as(
        r.addr,
        "/api/apps/breakfast/layout",
        r#"{"baseCommit":"x","positions":{"web":{"x":1,"z":2}}}"#,
        "u-test01",
    );
    assert_eq!(status, 404);
    assert_error(&v, "app_not_found");

    // The same API under the aggregated path, only for the served namespace.
    let gv = format!(
        "/apis/{}/{}",
        laoban_server::identity::GROUP,
        laoban_server::identity::VERSION
    );
    let (status, body) = get(r.addr, &gv);
    assert_eq!(status, 200);
    assert!(body.contains("APIResourceList"));
    let (status, body) = get(
        r.addr,
        &format!("{gv}/namespaces/laoban-dev/apps/breakfast"),
    );
    assert_eq!(status, 404);
    assert_error(&serde_json::from_str(&body).unwrap(), "app_not_found");
    let (status, _) = get(r.addr, &format!("{gv}/namespaces/other/apps"));
    assert_eq!(status, 404);
    let (status, v) = post(
        r.addr,
        &format!("{gv}/namespaces/laoban-dev/plans/p-20261013-abcdef/apply"),
        "{}",
    );
    assert_eq!(status, 404);
    assert_error(&v, "plan_not_found");
}

#[test]
fn request_validation() {
    let r = start(Box::new(MockProvider::new()), 1_000_000);
    for body in [
        "not json",
        r#"{"prompt": 1}"#,
        r#"{"prompt":"x","extra":1}"#,
        "",
    ] {
        let (status, err) = post(r.addr, "/api/plan", body);
        assert_eq!(status, 400, "{body}");
        assert_error(&err, "bad_request");
    }
    // Missing JSON content type.
    let (status, _, body) = raw(
        r.addr,
        b"POST /api/plan HTTP/1.1\r\nContent-Type: text/plain\r\nContent-Length: 14\r\n\r\n{\"prompt\":\"x\"}",
    );
    assert_eq!(status, 400);
    assert_error(&serde_json::from_str(&body).unwrap(), "bad_request");
    // Too large.
    let big = format!(r#"{{"prompt":"{}"}}"#, "a".repeat(513 * 1024));
    let (status, err) = post(r.addr, "/api/plan", &big);
    assert_eq!(status, 413);
    assert_error(&err, "bad_request");
    // Chunked bodies are read (the API server may forward that way); an
    // empty one is still a bad plan request.
    let (status, _, body) = raw(
        r.addr,
        b"POST /api/plan HTTP/1.1\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
    );
    assert_eq!(status, 400);
    assert_error(&serde_json::from_str(&body).unwrap(), "bad_request");
    // A chunked request, as kubectl's goes through the API server.
    let (status, _, body) = raw(
        r.addr,
        b"POST /api/apps/breakfast/round/opinions HTTP/1.1\r\nX-Remote-User: u-test01\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n8\r\n{\"text\":\r\n4\r\n\"x\"}\r\n0\r\n\r\n",
    );
    // Read in full: refused only because there is no round.
    assert_eq!(status, 409, "{body}");
    assert_error(&serde_json::from_str(&body).unwrap(), "round_state");
    // Both framings at once: refused.
    let (status, _, _) = raw(
        r.addr,
        b"POST /api/plan HTTP/1.1\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nContent-Length: 5\r\n\r\n0\r\n\r\n",
    );
    assert_eq!(status, 400);
    // Wrong method and unknown path.
    let (status, head, _) = raw(r.addr, b"GET /api/plan HTTP/1.1\r\n\r\n");
    assert_eq!(status, 405);
    assert!(head.contains("Allow: POST"));
    assert_eq!(get(r.addr, "/api/nope").0, 404);
}

#[test]
fn ai_errors_map_to_spec_codes() {
    // Spend limit: 402, provider called exactly once.
    let provider = Arc::new(
        MockProvider::new().then(Err(ProviderError::SpendLimitReached {
            kind: SpendLimitKind::TierCap,
            message: "You have reached your API usage limits".into(),
        })),
    );
    let r = start(Box::new(Arc::clone(&provider)), 1_000_000);
    let (status, err) = post(r.addr, "/api/plan", r#"{"prompt":"x"}"#);
    assert_eq!(status, 402);
    assert_error(&err, "spend_limit");
    assert_eq!(provider.call_count(), 1);
    drop(r);

    // Budget: 429, provider never called.
    let provider = Arc::new(MockProvider::new().then(ok(&plan_json())));
    let r = start(Box::new(Arc::clone(&provider)), 50);
    let (status, err) = post(r.addr, "/api/plan", r#"{"prompt":"x"}"#);
    assert_eq!(status, 429);
    assert_error(&err, "budget_exceeded");
    assert_eq!(provider.call_count(), 0);
    drop(r);

    // Invalid output: 502.
    let r = start(
        Box::new(MockProvider::new().then(ok("no json here"))),
        1_000_000,
    );
    let (status, err) = post(r.addr, "/api/plan", r#"{"prompt":"x"}"#);
    assert_eq!(status, 502);
    assert_error(&err, "ai_invalid_output");
}

/// Provider that takes longer than the socket I/O timeout.
struct SlowProvider(MockProvider, Duration);

impl Provider for SlowProvider {
    fn send(&self, req: &ProviderRequest) -> Result<ProviderResponse, ProviderError> {
        thread::sleep(self.1);
        self.0.send(req)
    }
}

#[test]
fn slow_ai_calls_are_not_cut_by_socket_timeouts() {
    let slow = SlowProvider(
        MockProvider::new().then(ok(&plan_json())),
        IO_TIMEOUT + Duration::from_secs(1),
    );
    let r = start(Box::new(slow), 1_000_000);
    let (status, plan) = post(r.addr, "/api/plan", r#"{"prompt":"x"}"#);
    assert_eq!(status, 200, "{plan}");
}

#[test]
fn unconfigured_api_still_serves_probes() {
    let r = start_with(
        ApiState::Unconfigured("後端設定不完整：缺少環境變數 LAOBAN_MODEL。".into()),
        None,
    );
    assert_eq!(get(r.addr, "/healthz"), (200, "ok".to_string()));
    let (status, body) = get(r.addr, "/api/config");
    assert_eq!(status, 500);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_error(&v, "internal");
    assert!(v["error"]["message"]
        .as_str()
        .unwrap()
        .contains("LAOBAN_MODEL"));
    let (status, _) = post(r.addr, "/api/plan", r#"{"prompt":"x"}"#);
    assert_eq!(status, 500);
}

#[test]
fn binary_without_config_names_missing_variables_and_logs_requests() {
    let secret = "sk-test-should-never-be-logged-0001";
    let mut child = Command::new(env!("CARGO_BIN_EXE_laoban-server"))
        .env("LAOBAN_LISTEN_ADDR", "127.0.0.1:0")
        .env("ANTHROPIC_API_KEY", secret)
        .env_remove("LAOBAN_MODEL")
        .env_remove("LAOBAN_GITHUB_REPO")
        .env_remove("LAOBAN_GITHUB_TOKEN")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    stderr.read_line(&mut line).unwrap();
    let addr: SocketAddr = line.trim().rsplit(' ').next().unwrap().parse().unwrap();

    // Without TLS settings the plain port serves the probes only.
    let (status, body) = get(addr, "/api/config?x=1");
    assert_eq!(status, 404);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert!(v["error"]["message"].as_str().unwrap().contains("健康檢查"));
    assert_eq!(get(addr, "/healthz").0, 200);

    // SAFETY: plain kill(2) on our own child's pid.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    let mut rest = String::new();
    stderr.read_to_string(&mut rest).unwrap();
    child.wait().unwrap();
    assert!(rest.contains("/api disabled"), "{rest}");
    assert!(
        rest.contains("LAOBAN_MODEL") && rest.contains("LAOBAN_GITHUB_REPO"),
        "missing variables are named: {rest}"
    );
    assert!(
        !rest.contains("ANTHROPIC_API_KEY"),
        "set variables are not reported"
    );
    assert!(rest.contains("probes only"), "{rest}");
    assert!(
        rest.contains("laoban-server: GET /api/config 404 "),
        "{rest}"
    );
    assert!(rest.contains("laoban-server: GET /healthz 200 "), "{rest}");
    assert!(!rest.contains(secret) && !line.contains(secret));
    assert!(!rest.contains("x=1"), "query strings are not logged");
}

fn get_as(addr: SocketAddr, path: &str, user: &str) -> (u16, Value) {
    let (status, _, body) = raw(
        addr,
        format!("GET {path} HTTP/1.1\r\nHost: x\r\nX-Remote-User: {user}\r\n\r\n").as_bytes(),
    );
    (status, serde_json::from_str(&body).unwrap())
}

/// Read a job until it is no longer running (up to 20 s).
fn wait_job(addr: SocketAddr, id: &str, user: &str) -> Value {
    for _ in 0..400 {
        let (status, v) = get_as(addr, &format!("/api/plans/{id}"), user);
        assert_eq!(status, 200, "{v}");
        if v["state"] != "running" {
            return v;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("job {id} never finished");
}

#[test]
fn background_plan_and_apply_are_jobs() {
    // Report 18, B8: the API server ends requests after about 60 s, so the
    // page asks for a job and reads it until it is done.
    let provider = MockProvider::new()
        .then(ok(&plan_json()))
        .then(ok(&files_json()))
        .then(Err(ProviderError::Other {
            status: Some(500),
            error_type: "api_error".into(),
            message: "boom".into(),
        }));
    let github = github_mock();
    let mut cfg = GithubConfig::new("example-owner", "laoban-apps");
    cfg.api_base = github.url();
    let committer = GithubCommitter::new(cfg, GithubToken::new("github_pat_test_0001").unwrap());
    let service =
        Arc::new(M0Service::new(settings(1_000_000), Box::new(provider), committer).unwrap());
    service.attach();
    let r = start_with(ApiState::Ready(service), Some(github));

    let (status, job) = post_as(
        r.addr,
        "/api/plan",
        r#"{"prompt":"做一個早餐店點餐頁面","background":true}"#,
        "u-a",
    );
    assert_eq!(status, 200, "{job}");
    let id = job["jobId"].as_str().unwrap().to_string();
    assert!(id.starts_with("j-") && id.len() == 18, "{id}");
    assert_eq!(job["kind"], "plan");

    // Only the user who started it can read it.
    let (status, err) = get_as(r.addr, &format!("/api/plans/{id}"), "u-b");
    assert_eq!(status, 404);
    assert_eq!(err["error"]["code"], "job_not_found");

    let done = wait_job(r.addr, &id, "u-a");
    assert_eq!(done["state"], "done", "{done}");
    let plan_id = done["result"]["planId"].as_str().unwrap().to_string();
    assert_eq!(done["result"]["appName"], "breakfast");

    // Apply through the aggregated-style path, with the optional body.
    let (status, job) = post_as(
        r.addr,
        &format!("/api/plans/{plan_id}/apply"),
        r#"{"background":true}"#,
        "u-a",
    );
    assert_eq!(status, 200, "{job}");
    let done = wait_job(r.addr, job["jobId"].as_str().unwrap(), "u-a");
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(done["result"]["commit"], COMMIT);

    // A failure is kept with the error the request would have returned.
    let (_, job) = post_as(
        r.addr,
        "/api/plan",
        r#"{"prompt":"再一個","background":true}"#,
        "u-a",
    );
    let failed = wait_job(r.addr, job["jobId"].as_str().unwrap(), "u-a");
    assert_eq!(failed["state"], "failed", "{failed}");
    assert!(failed["error"]["code"].is_string());
    assert!(failed["error"]["message"]
        .as_str()
        .is_some_and(|m| !m.is_empty()));

    // Unknown fields are still refused; malformed job IDs are not routes.
    let (status, _) = post_as(
        r.addr,
        &format!("/api/plans/{plan_id}/apply"),
        r#"{"x":1}"#,
        "u-a",
    );
    assert_eq!(status, 400);
    let (status, _) = get_as(r.addr, "/api/plans/j-nothex", "u-a");
    assert_eq!(status, 404);
}
