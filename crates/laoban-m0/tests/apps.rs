//! Listing, reading and changing existing apps (laoban.json).

mod common;

use common::*;
use laoban_ai::MockProvider;
use laoban_m0::codegen::{parse_generated, GeneratedFiles};
use laoban_m0::meta::{self, HistoryEntry};
use laoban_m0::plan::parse_app_plan;
use laoban_m0::service::unused_name;
use laoban_m0::{ApplyRequest, PlanRequestBody};
use serde_json::{json, Value};

fn entry(at: &str, kind: &str, request: &str) -> HistoryEntry {
    HistoryEntry {
        at: at.into(),
        kind: kind.into(),
        request: request.into(),
        summary: "摘要".into(),
        model: MODEL.into(),
    }
}

fn breakfast_meta() -> String {
    let plan = parse_app_plan(&plan_json()).unwrap();
    let files = parse_generated(&files_json()).unwrap();
    meta::to_json(
        "breakfast",
        &plan,
        &files,
        &[entry(
            "2026-10-13T01:00:00Z",
            "create",
            "做一個早餐店點餐頁面",
        )],
    )
}

fn apps() -> Vec<(String, Option<String>)> {
    vec![
        ("breakfast".into(), Some(breakfast_meta())),
        ("legacy-shop".into(), None),
        (
            "todo".into(),
            Some(meta::to_json(
                "todo",
                &parse_app_plan(&plan_json()).unwrap(),
                &parse_generated(&files_json()).unwrap(),
                &[
                    entry("2026-10-12T00:00:00Z", "create", "待辦"),
                    entry("2026-10-13T05:00:00Z", "edit", "加上刪除"),
                ],
            )),
        ),
    ]
}

fn edit_req(prompt: &str, base: &str) -> PlanRequestBody {
    PlanRequestBody {
        prompt: prompt.into(),
        app_name: None,
        base_app: Some(base.into()),
    }
}

fn new_req(prompt: &str, name: Option<&str>) -> PlanRequestBody {
    PlanRequestBody {
        prompt: prompt.into(),
        app_name: name.map(str::to_string),
        base_app: None,
    }
}

#[test]
fn list_newest_first_and_legacy_last() {
    let h = harness_with(MockProvider::new(), RefUpdate::Ok, 1_000_000, apps());
    let v = h.service.list_apps().unwrap();
    let list = v["apps"].as_array().unwrap();
    let names: Vec<&str> = list
        .iter()
        .map(|a| a["appName"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["todo", "breakfast", "legacy-shop"]);
    assert_eq!(list[0]["editable"], true);
    assert_eq!(list[0]["revisions"], 2);
    assert_eq!(list[0]["updatedAt"], "2026-10-13T05:00:00Z");
    assert_eq!(list[0]["fleet"]["bundlePrefix"], "laoban-apps-apps-todo");
    assert_eq!(
        list[0]["appUrl"],
        "/k8s/clusters/c-m-test0001/api/v1/namespaces/laoban-dev/services/http:todo-web:8080/proxy/"
    );
    assert_eq!(list[1]["summary"], "早餐店點餐頁面，可以選餐點和送出訂單。");
    assert_eq!(list[2]["editable"], false);
    // Files are not part of the listing.
    assert!(!v.to_string().contains("def handle("));
    assert_eq!(h.provider.call_count(), 0);
    assert_eq!(writes(&h.github), 0);
}

#[test]
fn list_is_empty_without_apps_dir() {
    let h = harness(MockProvider::new(), RefUpdate::Ok, 1_000_000);
    assert_eq!(h.service.list_apps().unwrap(), json!({"apps": []}));
}

#[test]
fn get_app_and_errors() {
    let h = harness_with(MockProvider::new(), RefUpdate::Ok, 1_000_000, apps());
    let v = h.service.get_app("todo").unwrap();
    assert_eq!(v["appName"], "todo");
    assert_eq!(v["history"][0]["request"], "加上刪除", "newest first");
    assert!(v["architecture"]["nodes"].as_array().unwrap().len() >= 4);
    assert!(v.get("files").is_none());

    let err = h.service.get_app("legacy-shop").unwrap_err();
    assert_eq!((err.status(), err.code()), (409, "not_editable"));
    let err = h.service.get_app("nope").unwrap_err();
    assert_eq!((err.status(), err.code()), (404, "app_not_found"));
    let err = h.service.get_app("../x").unwrap_err();
    assert_eq!(err.code(), "bad_request");
}

#[test]
fn edit_plan_then_apply() {
    // The AI suggests another name; an edit keeps the original one.
    let mut edited: Value = serde_json::from_str(&plan_json()).unwrap();
    edited["appName"] = json!("something-else");
    edited["summary"] = json!("早餐店點餐頁面，加上飲料。");
    let mut new_files: Value = files_value();
    new_files["api"]["handler.py"] = json!("MENU = [\"蛋餅\", \"紅茶\"]\n\ndef handle(method, path, body):\n    return 200, {\"menu\": MENU}\n");
    let provider = MockProvider::new()
        .then(ok(&edited.to_string()))
        .then(ok(&new_files.to_string()));
    let h = harness_with(provider, RefUpdate::Ok, 1_000_000, apps());

    let plan = h.service.plan(&edit_req("加上飲料", "breakfast")).unwrap();
    assert_eq!(plan["mode"], "edit");
    assert_eq!(plan["appName"], "breakfast");
    let sent = h.provider.received();
    let user = &sent[0].messages[0].content;
    assert!(user.contains("kind=current_plan") && user.contains("kind=change_request"));
    assert!(user.contains("加上飲料") && user.contains("早餐店點餐頁面"));
    assert!(sent[0].system.contains("existing application"));

    let id = plan["planId"].as_str().unwrap().to_string();
    let applied = h.service.apply(&ApplyRequest { plan_id: id }).unwrap();
    assert_eq!(applied["mode"], "edit");
    assert_eq!(applied["appName"], "breakfast");

    let sent = h.provider.received();
    let user = &sent[1].messages[0].content;
    assert!(user.contains("kind=approved_plan") && user.contains("kind=current_code"));
    assert!(user.contains("def handle("), "current code sent along");
    assert!(sent[1].system.contains("change to an existing application"));

    let blobs = blob_contents(&h.github);
    let meta = meta::parse("breakfast", &blobs[6]).unwrap();
    assert!(meta.files.handler_py.contains("紅茶"));
    let kinds: Vec<&str> = meta.history.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, ["create", "edit"]);
    assert_eq!(meta.history[1].request, "加上飲料");

    let commit = h
        .github
        .requests()
        .into_iter()
        .find(|r| r.method == "POST" && r.path.ends_with("/git/commits"))
        .unwrap();
    let body: Value = serde_json::from_str(&commit.body_str()).unwrap();
    assert!(body["message"]
        .as_str()
        .unwrap()
        .starts_with("laoban: breakfast — 修改：加上飲料"));
}

#[test]
fn edit_errors() {
    let h = harness_with(MockProvider::new(), RefUpdate::Ok, 1_000_000, apps());
    let err = h.service.plan(&edit_req("x", "legacy-shop")).unwrap_err();
    assert_eq!(err.code(), "not_editable");
    let err = h.service.plan(&edit_req("x", "nope")).unwrap_err();
    assert_eq!(err.code(), "app_not_found");
    let err = h
        .service
        .plan(&PlanRequestBody {
            prompt: "x".into(),
            app_name: Some("other".into()),
            base_app: Some("breakfast".into()),
        })
        .unwrap_err();
    assert_eq!(err.code(), "bad_request");
    assert_eq!(h.provider.call_count(), 0);
}

#[test]
fn edit_refused_when_code_is_too_large() {
    let big = GeneratedFiles {
        index_html: "x".repeat(30 * 1024),
        app_js: "y".repeat(15 * 1024),
        style_css: String::new(),
        handler_py: "def handle(m, p, b):\n    return 200, {}\n".into(),
    };
    let meta = meta::to_json(
        "big",
        &parse_app_plan(&plan_json()).unwrap(),
        &big,
        &[entry("2026-10-13T00:00:00Z", "create", "x")],
    );
    let h = harness_with(
        MockProvider::new(),
        RefUpdate::Ok,
        1_000_000,
        vec![("big".into(), Some(meta))],
    );
    let err = h.service.plan(&edit_req("x", "big")).unwrap_err();
    assert_eq!(err.code(), "bad_request");
    assert_eq!(h.provider.call_count(), 0);
}

#[test]
fn new_app_with_taken_name() {
    // Chosen by the user: refused before calling the AI.
    let h = harness_with(MockProvider::new(), RefUpdate::Ok, 1_000_000, apps());
    let err = h
        .service
        .plan(&new_req("x", Some("breakfast")))
        .unwrap_err();
    assert_eq!((err.status(), err.code()), (409, "app_exists"));
    assert!(err.user_message().contains("breakfast"));
    assert!(!err.to_string().contains("breakfast"));
    assert_eq!(h.provider.call_count(), 0);

    // Chosen by the AI: a free variant is used.
    let provider = MockProvider::new().then(ok(&plan_json()));
    let h = harness_with(provider, RefUpdate::Ok, 1_000_000, apps());
    let plan = h.service.plan(&new_req("x", None)).unwrap();
    assert_eq!(plan["mode"], "create");
    assert_eq!(plan["appName"], "breakfast-2");
}

#[test]
fn unused_names() {
    let taken = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(unused_name("shop", &taken(&[])).as_deref(), Some("shop"));
    assert_eq!(
        unused_name("shop", &taken(&["shop", "shop-2"])).as_deref(),
        Some("shop-3")
    );
    let long = "a".repeat(31);
    let n = unused_name(&long, &taken(&[&long])).unwrap();
    assert_eq!(n.len(), 31);
    assert!(n.ends_with("-2"));
    let all: Vec<String> = std::iter::once("x1".to_string())
        .chain((2..=9).map(|i| format!("x1-{i}")))
        .collect();
    assert_eq!(unused_name("x1", &all), None);
}
