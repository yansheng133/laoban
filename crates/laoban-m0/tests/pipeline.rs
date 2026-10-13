//! Plan → apply with a mock provider and a mock GitHub server.

mod common;

use common::*;
use laoban_ai::{MockProvider, ProviderError, SpendLimitKind};
use laoban_m0::{ApplyRequest, M0Error, PlanRequestBody};
use serde_json::{json, Value};

fn plan_req(prompt: &str) -> PlanRequestBody {
    PlanRequestBody {
        prompt: prompt.into(),
        app_name: None,
        base_app: None,
    }
}

fn apply_req(id: &str) -> ApplyRequest {
    ApplyRequest { plan_id: id.into() }
}

fn plan_id(v: &Value) -> String {
    v["planId"].as_str().unwrap().to_string()
}

fn call_sequence(h: &Harness) -> Vec<String> {
    h.github
        .requests()
        .iter()
        .map(|r| {
            let path = r
                .path
                .strip_prefix("/repos/example-owner/laoban-apps/git/")
                .or_else(|| r.path.strip_prefix("/repos/example-owner/laoban-apps/"))
                .unwrap_or(&r.path)
                .to_string();
            format!("{} {}", r.method, path)
        })
        .collect()
}

#[test]
fn happy_path_plan_then_apply() {
    let provider = MockProvider::new()
        .then(ok(&plan_json()))
        .then(ok(&files_json()));
    let h = harness(provider, RefUpdate::Ok, 1_000_000);

    let plan = h
        .service
        .plan(&plan_req("做一個早餐店點餐頁面，可以選餐點和送出訂單"))
        .unwrap();
    let id = plan_id(&plan);
    assert!(id.starts_with("p-") && id.len() == 17, "{id}");
    assert_eq!(plan["appName"], "breakfast");
    assert_eq!(plan["summary"], "早餐店點餐頁面，可以選餐點和送出訂單。");
    assert_eq!(plan["steps"], json!(["建立點餐頁面", "建立訂單 API"]));
    assert_eq!(plan["maskedFindings"], 0);
    assert_eq!(
        plan["usage"],
        json!({"inputTokens": 120, "outputTokens": 80})
    );

    // Architecture: fixed shape, positions from the backend only.
    let nodes = plan["architecture"]["nodes"].as_array().unwrap();
    let ids: Vec<&str> = nodes.iter().map(|n| n["id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        [
            "svc-web",
            "svc-api",
            "app-web",
            "app-api",
            "mod-web-1",
            "mod-web-2",
            "mod-api-1",
            "tbl-data"
        ]
    );
    for n in nodes {
        assert!(n["pos"]["x"].is_number() && n["pos"]["z"].is_number());
        assert!(["network", "code", "data"].contains(&n["layer"].as_str().unwrap()));
        let d = n["detail"].as_u64().unwrap();
        assert!((1..=3).contains(&d));
    }
    let api_mod = &nodes[6];
    assert_eq!(api_mod["name"], "orders");
    assert_eq!(api_mod["parent"], "app-api");
    assert_eq!(
        api_mod["pos"],
        json!({"x": 3.0, "z": 2.5}),
        "AI coordinates ignored"
    );
    assert_eq!(nodes[7]["props"]["storage"], "memory");
    assert_eq!(plan["architecture"]["edges"].as_array().unwrap().len(), 4);

    let mut applied = h.service.apply(&apply_req(&id)).unwrap();
    let rollout = applied.as_object_mut().unwrap().remove("rollout").unwrap();
    assert_eq!(
        applied,
        json!({
            "mode": "create",
            "appName": "breakfast",
            "commit": NEW_COMMIT,
            "commitUrl": format!("https://github.com/example-owner/laoban-apps/commit/{NEW_COMMIT}"),
            "path": "apps/breakfast",
            "appUrl": "/k8s/clusters/c-m-test0001/api/v1/namespaces/laoban-dev/services/http:breakfast-web:8080/proxy/",
            "fleet": {"workspace": "fleet-default", "gitRepo": "laoban-apps", "bundlePrefix": "laoban-apps-apps-breakfast"}
        })
    );

    // Git Data API sequence.
    let seq = call_sequence(&h);
    // The name is checked against apps/ when planning and again before
    // committing.
    let mut expected = vec![
        "GET contents/apps?ref=main".to_string(),
        "GET contents/apps?ref=main".to_string(),
        "GET ref/heads/main".to_string(),
        format!("GET commits/{BASE_COMMIT}"),
    ];
    expected.extend(std::iter::repeat("POST blobs".to_string()).take(8));
    expected.extend([
        "POST trees".to_string(),
        "POST commits".to_string(),
        "PATCH refs/heads/main".to_string(),
    ]);
    assert_eq!(seq, expected);

    let reqs = h.github.requests();
    for r in &reqs {
        assert_eq!(
            r.header("authorization"),
            Some(format!("Bearer {GH_TOKEN}").as_str())
        );
        assert_eq!(r.header("accept"), Some("application/vnd.github+json"));
    }
    let body = |i: usize| -> Value { serde_json::from_str(&reqs[i].body_str()).unwrap() };
    let tree = body(12);
    assert_eq!(tree["base_tree"], BASE_TREE);
    let paths: Vec<&str> = tree["tree"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            assert_eq!(e["mode"], "100644");
            assert_eq!(e["type"], "blob");
            e["path"].as_str().unwrap()
        })
        .collect();
    assert_eq!(
        paths,
        [
            "apps/breakfast/fleet.yaml",
            "apps/breakfast/configmap-web.yaml",
            "apps/breakfast/configmap-api.yaml",
            "apps/breakfast/web.yaml",
            "apps/breakfast/api.yaml",
            "apps/breakfast/README.md",
            "apps/breakfast/laoban.json",
            "apps/breakfast/.fleetignore",
        ]
    );
    let commit = body(13);
    assert_eq!(commit["tree"], NEW_TREE);
    assert_eq!(commit["parents"], json!([BASE_COMMIT]));
    assert_eq!(
        commit["message"],
        "laoban: breakfast — 早餐店點餐頁面，可以選餐點和送出訂單。\n\nGenerated-by: laoban\n"
    );
    assert_eq!(body(14), json!({"sha": NEW_COMMIT, "force": false}));

    // Blob contents.
    let blobs = blob_contents(&h.github);
    assert!(blobs[0].contains("defaultNamespace: \"laoban-dev\""));
    assert!(blobs[1].contains("server.py: |") && blobs[1].contains("ThreadingHTTPServer"));
    assert!(blobs[2].contains("handler.py: |") && blobs[2].contains("def handle("));
    assert!(blobs[3].contains("registry.suse.com/bci/python:3.13"));
    assert!(blobs[3].contains("laoban.io/content-hash:"));
    assert!(blobs[3].contains("http://breakfast-api:8080/"));
    assert!(blobs[4].contains("\"/app/main.py\""));
    // The page compares these with the live Deployments' pod templates.
    let rollout = rollout.as_array().unwrap();
    assert_eq!(rollout.len(), 2);
    for (r, (name, blob)) in rollout
        .iter()
        .zip([("breakfast-web", &blobs[3]), ("breakfast-api", &blobs[4])])
    {
        assert_eq!(r["deployment"], name);
        assert_eq!(
            r["path"],
            format!("/k8s/clusters/c-m-test0001/v1/apps.deployments/laoban-dev/{name}")
        );
        let hash = r["contentHash"].as_str().unwrap();
        assert_eq!(hash.len(), 64);
        assert!(
            blob.contains(&format!("laoban.io/content-hash: \"{hash}\"")),
            "{blob}"
        );
    }
    assert!(blobs[5].contains("做一個早餐店點餐頁面"));
    assert!(blobs[5].contains(MODEL));
    // laoban.json reads back with the same validation as an AI response.
    let meta = laoban_m0::meta::parse("breakfast", &blobs[6]).unwrap();
    assert_eq!(
        meta.files,
        laoban_m0::codegen::parse_generated(&files_json()).unwrap()
    );
    assert_eq!(meta.history.len(), 1);
    assert_eq!(meta.history[0].kind, "create");
    assert!(meta.history[0].request.contains("做一個早餐店點餐頁面"));
    assert!(blobs[7].contains("laoban.json"));

    // The code call received the plan as a data block, not the user turn.
    let sent = h.provider.received();
    assert_eq!(sent.len(), 2);
    assert!(sent[1].messages[0].content.contains("kind=approved_plan"));
    assert!(sent[1].system.contains("handler.py"));
    assert_eq!(sent[1].model, MODEL);
}

#[test]
fn apply_twice_is_409() {
    let provider = MockProvider::new()
        .then(ok(&plan_json()))
        .then(ok(&files_json()));
    let h = harness(provider, RefUpdate::Ok, 1_000_000);
    let id = plan_id(&h.service.plan(&plan_req("x 需求")).unwrap());
    h.service.apply(&apply_req(&id)).unwrap();
    let err = h.service.apply(&apply_req(&id)).unwrap_err();
    assert!(matches!(err, M0Error::AlreadyApplied), "{err:?}");
    assert_eq!((err.status(), err.code()), (409, "already_applied"));
    assert_eq!(h.provider.call_count(), 2, "no third AI call");
    assert_eq!(
        h.github
            .requests()
            .iter()
            .filter(|r| r.method == "PATCH")
            .count(),
        1
    );
}

#[test]
fn unknown_plan_is_404() {
    let h = harness(MockProvider::new(), RefUpdate::Ok, 1_000_000);
    let err = h
        .service
        .apply(&apply_req("p-20261013-abcdef"))
        .unwrap_err();
    assert_eq!((err.status(), err.code()), (404, "plan_not_found"));
    assert_eq!(h.provider.call_count(), 0);
}

#[test]
fn extra_file_key_is_502_and_nothing_is_committed() {
    let mut extra = files_value();
    extra["web"]["evil.yaml"] = json!("kind: ClusterRoleBinding");
    let provider = MockProvider::new()
        .then(ok(&plan_json()))
        .then(ok(&extra.to_string()))
        .then(ok(&files_json()));
    let h = harness(provider, RefUpdate::Ok, 1_000_000);
    let id = plan_id(&h.service.plan(&plan_req("x 需求")).unwrap());
    let err = h.service.apply(&apply_req(&id)).unwrap_err();
    assert_eq!((err.status(), err.code()), (502, "ai_invalid_output"));
    assert_eq!(writes(&h.github), 0);
    // A failed apply can be retried.
    assert!(h.service.apply(&apply_req(&id)).is_ok());
}

#[test]
fn extra_top_level_key_is_502() {
    let mut extra = files_value();
    extra["k8s"] = json!({"rbac.yaml": "x"});
    let provider = MockProvider::new()
        .then(ok(&plan_json()))
        .then(ok(&extra.to_string()));
    let h = harness(provider, RefUpdate::Ok, 1_000_000);
    let id = plan_id(&h.service.plan(&plan_req("x 需求")).unwrap());
    let err = h.service.apply(&apply_req(&id)).unwrap_err();
    assert_eq!(err.code(), "ai_invalid_output");
    assert_eq!(writes(&h.github), 0);
}

#[test]
fn oversize_file_is_502() {
    let mut big = files_value();
    big["web"]["app.js"] = json!("x".repeat(64 * 1024 + 1));
    let provider = MockProvider::new()
        .then(ok(&plan_json()))
        .then(ok(&big.to_string()));
    let h = harness(provider, RefUpdate::Ok, 1_000_000);
    let id = plan_id(&h.service.plan(&plan_req("x 需求")).unwrap());
    let err = h.service.apply(&apply_req(&id)).unwrap_err();
    assert_eq!((err.status(), err.code()), (502, "ai_invalid_output"));
    assert_eq!(writes(&h.github), 0);
}

#[test]
fn spend_limit_is_402_and_not_retried() {
    for kind in [SpendLimitKind::SelfSet, SpendLimitKind::TierCap] {
        let provider = MockProvider::new().then(Err(ProviderError::SpendLimitReached {
            kind,
            message: "You have reached your specified API usage limits".into(),
        }));
        let h = harness(provider, RefUpdate::Ok, 1_000_000);
        let err = h.service.plan(&plan_req("x 需求")).unwrap_err();
        assert_eq!((err.status(), err.code()), (402, "spend_limit"));
        assert_eq!(h.provider.call_count(), 1, "provider called exactly once");
    }
}

#[test]
fn budget_exceeded_is_429_before_provider_call() {
    let h = harness(
        MockProvider::new().then(ok(&plan_json())),
        RefUpdate::Ok,
        100,
    );
    let err = h.service.plan(&plan_req("x 需求")).unwrap_err();
    assert_eq!((err.status(), err.code()), (429, "budget_exceeded"));
    assert_eq!(h.provider.call_count(), 0);
}

#[test]
fn national_id_never_reaches_provider_or_readme() {
    let id_number = with_check_digit("F20481234");
    let provider = MockProvider::new()
        .then(ok(&plan_json()))
        .then(ok(&files_json()));
    let h = harness(provider, RefUpdate::Ok, 1_000_000);
    let prompt = format!("做一個會員查詢頁，測試帳號的身分證字號是 {id_number}。");
    let plan = h.service.plan(&plan_req(&prompt)).unwrap();
    assert_eq!(plan["maskedFindings"], 1);
    h.service.apply(&apply_req(&plan_id(&plan))).unwrap();

    for req in h.provider.received() {
        let all = format!("{}{}", req.system, req.messages[0].content);
        assert!(!all.contains(&id_number));
    }
    assert!(h.provider.received()[0].messages[0]
        .content
        .contains("[MASKED:TW_ID]"));
    let blobs = blob_contents(&h.github);
    assert!(blobs.iter().all(|b| !b.contains(&id_number)));
    let readme = &blobs[5];
    assert!(readme.contains("[MASKED:TW_ID]"), "{readme}");
}

#[test]
fn stale_ref_is_github_failed_without_retry() {
    for status in [422, 409] {
        let provider = MockProvider::new()
            .then(ok(&plan_json()))
            .then(ok(&files_json()));
        let h = harness(provider, RefUpdate::Status(status), 1_000_000);
        let id = plan_id(&h.service.plan(&plan_req("x 需求")).unwrap());
        let err = h.service.apply(&apply_req(&id)).unwrap_err();
        assert_eq!((err.status(), err.code()), (502, "github_failed"));
        assert!(!format!("{err} {err:?}").contains(GH_TOKEN));
        let patches = h
            .github
            .requests()
            .iter()
            .filter(|r| r.method == "PATCH")
            .count();
        assert_eq!(patches, 1, "no retry loop");
    }
}

#[test]
fn bad_requests() {
    let h = harness(MockProvider::new(), RefUpdate::Ok, 1_000_000);
    for body in [
        plan_req("   "),
        plan_req(&"字".repeat(4_001)),
        PlanRequestBody {
            prompt: "ok".into(),
            app_name: Some("Bad_Name".into()),
            base_app: None,
        },
    ] {
        let err = h.service.plan(&body).unwrap_err();
        assert_eq!((err.status(), err.code()), (400, "bad_request"));
    }
    assert_eq!(h.provider.call_count(), 0);
}

#[test]
fn user_app_name_wins_and_invalid_ai_name_is_502() {
    let provider = MockProvider::new().then(ok(&plan_json()));
    let h = harness(provider, RefUpdate::Ok, 1_000_000);
    let plan = h
        .service
        .plan(&PlanRequestBody {
            prompt: "x 需求".into(),
            app_name: Some("my-shop".into()),
            base_app: None,
        })
        .unwrap();
    assert_eq!(plan["appName"], "my-shop");
    assert_eq!(plan["architecture"]["nodes"][0]["name"], "my-shop-web");

    let mut bad = serde_json::from_str::<Value>(&plan_json()).unwrap();
    bad["appName"] = json!("Not Valid");
    let h = harness(
        MockProvider::new().then(ok(&bad.to_string())),
        RefUpdate::Ok,
        1_000_000,
    );
    let err = h.service.plan(&plan_req("x 需求")).unwrap_err();
    assert_eq!(err.code(), "ai_invalid_output");
}

#[test]
fn malformed_plan_is_502() {
    for content in [
        "not json".to_string(),
        r#"{"summary":"x","steps":[]}"#.to_string(),
        json!({"appName":"ab","summary":"s","steps":["a"],"components":{"web":{},"api":{"endpoints":[{"method":"TRACE","path":"/x"}]}}}).to_string(),
    ] {
        let h = harness(MockProvider::new().then(ok(&content)), RefUpdate::Ok, 1_000_000);
        let err = h.service.plan(&plan_req("x 需求")).unwrap_err();
        assert_eq!(err.code(), "ai_invalid_output", "{content}");
    }
}

#[test]
fn config_json_has_no_secrets() {
    let h = harness(MockProvider::new(), RefUpdate::Ok, 1_000_000);
    let c = h.service.config_json();
    assert_eq!(
        c,
        json!({"model": MODEL, "targetNamespace": "laoban-dev", "repo": "example-owner/laoban-apps",
               "tokenBudget": {"cap": 1_000_000, "used": 0}, "aiTimeoutSeconds": 300})
    );
    assert!(!c.to_string().contains(GH_TOKEN));
}
