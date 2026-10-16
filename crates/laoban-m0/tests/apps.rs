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
        by: String::new(),
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
        background: false,
    }
}

fn new_req(prompt: &str, name: Option<&str>) -> PlanRequestBody {
    PlanRequestBody {
        prompt: prompt.into(),
        app_name: name.map(str::to_string),
        base_app: None,
        background: false,
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
    let v = h.service.get_app("todo", None).unwrap();
    assert_eq!(v["appName"], "todo");
    assert_eq!(v["history"][0]["request"], "加上刪除", "newest first");
    assert!(v["architecture"]["nodes"].as_array().unwrap().len() >= 4);
    assert!(v.get("files").is_none());

    let err = h.service.get_app("legacy-shop", None).unwrap_err();
    assert_eq!((err.status(), err.code()), (409, "not_editable"));
    let err = h.service.get_app("nope", None).unwrap_err();
    assert_eq!((err.status(), err.code()), (404, "app_not_found"));
    let err = h.service.get_app("../x", None).unwrap_err();
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

    // Without a verified caller nothing happens.
    let err = h
        .service
        .plan(&edit_req("加上飲料", "breakfast"), None)
        .unwrap_err();
    assert_eq!((err.status(), err.code()), (401, "unauthenticated"));
    assert_eq!(h.provider.call_count(), 0);
    let token = lock(&h, "breakfast");
    let plan = h
        .service
        .plan(&edit_req("加上飲料", "breakfast"), Some(&token))
        .unwrap();
    assert_eq!(plan["mode"], "edit");
    assert_eq!(plan["appName"], "breakfast");
    let sent = h.provider.received();
    let user = &sent[0].messages[0].content;
    assert!(user.contains("kind=current_plan") && user.contains("kind=change_request"));
    assert!(user.contains("加上飲料") && user.contains("早餐店點餐頁面"));
    assert!(sent[0].system.contains("existing application"));
    // The last change's notes are not given back to the AI.
    assert!(!user.contains("資料只放記憶體"), "{user}");

    let id = plan["planId"].as_str().unwrap().to_string();
    let applied = h
        .service
        .apply(
            &ApplyRequest {
                plan_id: id,
                background: false,
            },
            Some(&token),
        )
        .unwrap();
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
        .starts_with("laoban: breakfast — Change: 加上飲料"));
}

#[test]
fn edit_errors() {
    let h = harness_with(MockProvider::new(), RefUpdate::Ok, 1_000_000, apps());
    let err = h
        .service
        .plan(&edit_req("x", "legacy-shop"), None)
        .unwrap_err();
    assert_eq!(err.code(), "not_editable");
    let err = h.service.plan(&edit_req("x", "nope"), None).unwrap_err();
    assert_eq!(err.code(), "app_not_found");
    let err = h
        .service
        .plan(
            &PlanRequestBody {
                prompt: "x".into(),
                app_name: Some("other".into()),
                base_app: Some("breakfast".into()),
                background: false,
            },
            None,
        )
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
    let token = lock(&h, "big");
    let err = h
        .service
        .plan(&edit_req("x", "big"), Some(&token))
        .unwrap_err();
    assert_eq!(err.code(), "bad_request");
    assert_eq!(h.provider.call_count(), 0);
}

#[test]
fn new_app_with_taken_name() {
    // Chosen by the user: refused before calling the AI.
    let h = harness_with(MockProvider::new(), RefUpdate::Ok, 1_000_000, apps());
    let err = h
        .service
        .plan(&new_req("x", Some("breakfast")), None)
        .unwrap_err();
    assert_eq!((err.status(), err.code()), (409, "app_exists"));
    assert!(err.user_message().contains("breakfast"));
    assert!(!err.to_string().contains("breakfast"));
    assert_eq!(h.provider.call_count(), 0);

    // Chosen by the AI: a free variant is used.
    let provider = MockProvider::new().then(ok(&plan_json()));
    let h = harness_with(provider, RefUpdate::Ok, 1_000_000, apps());
    let plan = h.service.plan(&new_req("x", None), None).unwrap();
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

const V1: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const V2: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// todo with two versions: v1 (create) and v2 (edit, the current one, with
/// an extra module and endpoint).
fn todo_versions() -> (Vec<(String, Option<String>)>, Vec<common::Version>) {
    let plan1 = parse_app_plan(&plan_json()).unwrap();
    let mut v2: Value = serde_json::from_str(&plan_json()).unwrap();
    v2["summary"] = json!("早餐店點餐頁面，加上飲料。");
    v2["components"]["web"]["modules"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "drinks", "purpose": "飲料"}));
    v2["components"]["api"]["endpoints"]
        .as_array_mut()
        .unwrap()
        .push(json!({"method": "GET", "path": "/drinks", "purpose": "飲料"}));
    let plan2 = parse_app_plan(&v2.to_string()).unwrap();
    let files1 = parse_generated(&files_json()).unwrap();
    let mut files2 = files1.clone();
    files2.handler_py.push_str("\nDRINKS = [\"紅茶\"]\n");
    let h1 = entry("2026-10-13T01:00:00Z", "create", "做一個早餐店");
    let h2 = entry("2026-10-13T02:00:00Z", "edit", "加上飲料");
    let m1 = meta::to_json("todo", &plan1, &files1, std::slice::from_ref(&h1));
    let m2 = meta::to_json("todo", &plan2, &files2, &[h1, h2]);
    (
        vec![("todo".into(), Some(m2.clone()))],
        vec![
            ("todo".into(), V2.into(), m2, "2026-10-13T02:00:05Z".into()),
            ("todo".into(), V1.into(), m1, "2026-10-13T01:00:05Z".into()),
        ],
    )
}

#[test]
fn versions_newest_first_with_history() {
    let (apps, versions) = todo_versions();
    let h = harness_versions(
        MockProvider::new(),
        RefUpdate::Ok,
        1_000_000,
        apps,
        versions,
    );
    let v = h.service.versions("todo").unwrap();
    let list = v["versions"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["version"], 2);
    assert_eq!(list[0]["commit"], V2);
    assert_eq!(list[0]["kind"], "edit");
    assert_eq!(list[0]["request"], "加上飲料");
    assert_eq!(list[0]["current"], true);
    assert_eq!(list[1]["version"], 1);
    assert_eq!(list[1]["kind"], "create");
    assert_eq!(list[1]["current"], false);
    assert_eq!(
        list[1]["commitUrl"],
        format!("https://github.com/example-owner/laoban-apps/commit/{V1}")
    );
    assert_eq!(v["truncated"], false);
    assert_eq!(writes(&h.github), 0);
}

#[test]
fn one_version_has_its_own_architecture() {
    let (apps, versions) = todo_versions();
    let h = harness_versions(
        MockProvider::new(),
        RefUpdate::Ok,
        1_000_000,
        apps,
        versions,
    );
    let old = h.service.version("todo", V1).unwrap();
    let new = h.service.version("todo", V2).unwrap();
    let ids = |v: &Value| -> Vec<String> {
        v["architecture"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap().to_string())
            .collect()
    };
    assert!(!ids(&old).contains(&"mod-web-drinks".to_string()));
    assert!(ids(&new).contains(&"mod-web-drinks".to_string()));
    // Module ids follow names, so unchanged modules keep their ids.
    assert!(ids(&old).contains(&"mod-web-menu-view".to_string()));
    assert!(ids(&new).contains(&"mod-web-menu-view".to_string()));
    let routes = |v: &Value| v["architecture"]["nodes"][3]["props"]["routes"].clone();
    assert_eq!(routes(&old), "GET /menu, POST /orders");
    assert_eq!(routes(&new), "GET /drinks, GET /menu, POST /orders");
    assert!(old.get("files").is_none());

    for (bad, code) in [
        ("zzzz", "bad_request"),
        (
            "cccccccccccccccccccccccccccccccccccccccc",
            "version_not_found",
        ),
    ] {
        assert_eq!(
            h.service.version("todo", bad).unwrap_err().code(),
            code,
            "{bad}"
        );
    }
}

#[test]
fn restore_publishes_an_old_version_without_ai() {
    let (apps, versions) = todo_versions();
    let h = harness_versions(
        MockProvider::new(),
        RefUpdate::Ok,
        1_000_000,
        apps,
        versions,
    );
    let token = lock(&h, "todo");
    let out = h
        .service
        .restore(
            "todo",
            &laoban_m0::RestoreRequest { commit: V1.into() },
            Some(&token),
        )
        .unwrap();
    assert_eq!(out["mode"], "restore");
    assert_eq!(out["appName"], "todo");
    assert_eq!(out["rollout"].as_array().unwrap().len(), 2);
    assert_eq!(h.provider.call_count(), 0, "no AI call");

    let blobs = blob_contents(&h.github);
    let m = meta::parse("todo", &blobs[6]).unwrap();
    assert!(!m.files.handler_py.contains("DRINKS"), "old code is back");
    assert!(!m.plan.web_modules.iter().any(|x| x.name == "drinks"));
    let kinds: Vec<&str> = m.history.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(
        kinds,
        ["create", "edit", "restore"],
        "history kept, restore added"
    );
    assert!(m.history[2].request.contains("aaaaaaa"));

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
        .starts_with("laoban: todo — Restore: "));
}

#[test]
fn restore_refuses_the_current_content_and_bad_input() {
    let (apps, versions) = todo_versions();
    let h = harness_versions(
        MockProvider::new(),
        RefUpdate::Ok,
        1_000_000,
        apps,
        versions,
    );
    let err = h
        .service
        .restore(
            "todo",
            &laoban_m0::RestoreRequest { commit: V1.into() },
            None,
        )
        .unwrap_err();
    assert_eq!(err.code(), "unauthenticated");
    let token = lock(&h, "todo");
    let err = h
        .service
        .restore(
            "todo",
            &laoban_m0::RestoreRequest { commit: V2.into() },
            Some(&token),
        )
        .unwrap_err();
    assert_eq!(err.code(), "bad_request");
    for (app, commit, code) in [
        ("todo", "short", "bad_request"),
        ("Bad", V1, "bad_request"),
        (
            "todo",
            "dddddddddddddddddddddddddddddddddddddddd",
            "version_not_found",
        ),
    ] {
        let err = h
            .service
            .restore(
                app,
                &laoban_m0::RestoreRequest {
                    commit: commit.into(),
                },
                Some(&token),
            )
            .unwrap_err();
        assert_eq!(err.code(), code, "{app} {commit}");
    }
    assert_eq!(writes(&h.github), 0);
}

fn todo_harness() -> Harness {
    let (apps, versions) = todo_versions();
    harness_versions(
        MockProvider::new(),
        RefUpdate::Ok,
        1_000_000,
        apps,
        versions,
    )
}

fn save_files(
    h: &Harness,
    files: Value,
    base: &str,
    token: Option<&str>,
) -> Result<Value, laoban_m0::M0Error> {
    h.service.save_files(
        "todo",
        &laoban_m0::SaveFilesRequest {
            base_commit: base.into(),
            files,
            note: Some("改標題".into()),
        },
        token,
    )
}

#[test]
fn files_read_and_manual_save_without_ai() {
    let h = todo_harness();
    let v = h.service.files("todo").unwrap();
    assert_eq!(v["commit"], V2);
    assert!(v["files"]["api"]["handler.py"]
        .as_str()
        .unwrap()
        .contains("DRINKS"));

    let token = lock(&h, "todo");
    let mut files = v["files"].clone();
    files["web"]["index.html"] = json!("<!doctype html><title>新標題</title>\n");
    let out = save_files(&h, files, V2, Some(&token)).unwrap();
    assert_eq!(out["appName"], "todo");
    assert_eq!(h.provider.call_count(), 0, "no AI call");
    let blobs = blob_contents(&h.github);
    let m = meta::parse("todo", &blobs[6]).unwrap();
    assert!(m.files.index_html.contains("新標題"));
    assert!(
        m.plan.web_modules.iter().any(|x| x.name == "drinks"),
        "plan unchanged"
    );
    let last = m.history.last().unwrap();
    assert_eq!(
        (last.kind.as_str(), last.request.as_str()),
        ("manual", "改標題")
    );
    assert_eq!(last.model, "");
}

#[test]
fn manual_save_checks() {
    let h = todo_harness();
    let token = lock(&h, "todo");
    let current = h.service.files("todo").unwrap()["files"].clone();
    // Same as now.
    let err = save_files(&h, current.clone(), V2, Some(&token)).unwrap_err();
    assert_eq!(err.code(), "bad_request");
    // Someone saved in between.
    let mut changed = current.clone();
    changed["web"]["style.css"] = json!("body { color: red; }\n");
    let err = save_files(&h, changed, V1, Some(&token)).unwrap_err();
    assert_eq!((err.status(), err.code()), (409, "version_conflict"));
    // Same checks as AI output.
    let mut bad = current.clone();
    bad["api"]["handler.py"] = json!("print('hi')\n");
    let err = save_files(&h, bad, V2, Some(&token)).unwrap_err();
    assert_eq!(err.code(), "invalid_files");
    assert!(err.user_message().contains("handle("));
    let mut big = current.clone();
    big["web"]["app.js"] = json!("a".repeat(70 * 1024));
    let err = save_files(&h, big, V2, Some(&token)).unwrap_err();
    assert!(err.user_message().contains("too large"));
    let mut extra = current;
    extra["web"]["x.js"] = json!("");
    assert_eq!(
        save_files(&h, extra, V2, Some(&token)).unwrap_err().code(),
        "invalid_files"
    );
    assert_eq!(writes(&h.github), 0);
}

#[test]
fn layout_save_is_a_new_version_and_shows_in_the_architecture() {
    let h = todo_harness();
    let token = lock(&h, "todo");
    let positions: meta::Layout =
        serde_json::from_value(json!({"mod-web-drinks": {"x": 7.5, "z": -3}})).unwrap();
    let req = |positions: meta::Layout, base: &str| laoban_m0::SaveLayoutRequest {
        base_commit: base.into(),
        positions,
    };
    let err = h
        .service
        .save_layout("todo", &req(positions.clone(), V2), None)
        .unwrap_err();
    assert_eq!(err.code(), "unauthenticated", "no verified caller");
    let bad: meta::Layout =
        serde_json::from_value(json!({"no-such-node": {"x": 1, "z": 1}})).unwrap();
    assert_eq!(
        h.service
            .save_layout("todo", &req(bad, V2), Some(&token))
            .unwrap_err()
            .code(),
        "bad_request"
    );
    assert_eq!(
        h.service
            .save_layout("todo", &req(positions.clone(), V1), Some(&token))
            .unwrap_err()
            .code(),
        "version_conflict"
    );

    h.service
        .save_layout("todo", &req(positions, V2), Some(&token))
        .unwrap();
    assert_eq!(h.provider.call_count(), 0);
    let blobs = blob_contents(&h.github);
    let m = meta::parse("todo", &blobs[6]).unwrap();
    let p = m.layout["mod-web-drinks"];
    assert_eq!((p.x, p.z), (7.5, -3.0));
    assert_eq!(m.history.last().unwrap().kind, "layout");

    let arch = laoban_m0::plan::architecture_with_layout(&m.plan, "todo", &m.layout);
    let node = arch["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "mod-web-drinks")
        .unwrap()
        .clone();
    assert_eq!(node["pos"], json!({"x": 7.5, "z": -3.0}));
}

#[test]
fn two_saves_at_once_give_one_version_and_a_version_conflict() {
    // Report 20, B11: the second save failed as "GitHub failed, check the
    // token". Saves of one app now wait for each other, so the second one
    // is checked against the version the first one wrote.
    let h = todo_harness();
    let v = h.service.files("todo").unwrap();
    let token = lock(&h, "todo");
    let mut a = v["files"].clone();
    a["web"]["style.css"] = json!("body { color: #111; } /* a */\n");
    let mut b = v["files"].clone();
    b["web"]["style.css"] = json!("body { color: #222; } /* b */\n");
    let results: Vec<Result<Value, laoban_m0::M0Error>> = std::thread::scope(|s| {
        let ta = s.spawn(|| save_files(&h, a.clone(), V2, Some(&token)));
        let tb = s.spawn(|| save_files(&h, b.clone(), V2, Some(&token)));
        vec![ta.join().unwrap(), tb.join().unwrap()]
    });
    let ok = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(ok, 1, "{results:?}");
    let err = results.into_iter().find_map(Result::err).unwrap();
    assert_eq!((err.status(), err.code()), (409, "version_conflict"));
    assert!(!err.user_message().contains("token"));
    let patches = h
        .github
        .requests()
        .iter()
        .filter(|r| r.method == "PATCH")
        .count();
    assert_eq!(patches, 1, "only one version written");
}
