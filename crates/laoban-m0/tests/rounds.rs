//! 意見收斂 through the service: open, collect, converge (AI plan),
//! confirm, apply (AI code, GitHub commit), and what a running round
//! blocks. Mock AI and GitHub; users are synthetic.

mod common;

use std::time::{SystemTime, UNIX_EPOCH};

use common::*;
use laoban_ai::MockProvider;
use laoban_m0::codegen::parse_generated;
use laoban_m0::meta::{self, HistoryEntry};
use laoban_m0::plan::parse_app_plan;
use laoban_m0::{OpenRoundRequest, OpinionRequest, PlanRequestBody, SkipRequest};
use serde_json::{json, Value};

fn todo_app() -> Vec<(String, Option<String>)> {
    let plan = parse_app_plan(&plan_json()).unwrap();
    let files = parse_generated(&files_json()).unwrap();
    let h = HistoryEntry {
        at: "2026-10-14T01:00:00Z".into(),
        kind: "create".into(),
        request: "做一個早餐店".into(),
        summary: "摘要".into(),
        model: MODEL.into(),
        by: "u-a".into(),
    };
    vec![(
        "todo".into(),
        Some(meta::to_json("todo", &plan, &files, &[h])),
    )]
}

/// The AI's changed plan: one more web module.
fn changed_plan() -> String {
    let mut p: Value = serde_json::from_str(&plan_json()).unwrap();
    p["summary"] = json!("早餐店點餐頁面，可以排序。");
    p["components"]["web"]["modules"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "sorter", "purpose": "排序"}));
    p.to_string()
}

fn changed_files() -> String {
    let mut f = files_value();
    f["web"]["app.js"] = json!("fetch(\"api/menu\").then(r => r.json()).then(sortItems);\n");
    f.to_string()
}

fn open(minutes: u64, scope: &str) -> OpenRoundRequest {
    OpenRoundRequest {
        opinion: "清單要能排序".into(),
        deadline_minutes: minutes,
        scope: Some(scope.into()),
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn state(v: &Value) -> &str {
    v["round"]["state"].as_str().unwrap()
}

#[test]
fn immediate_round_converges_confirms_and_publishes() {
    let provider = MockProvider::new()
        .then(ok(&changed_plan()))
        .then(ok(&changed_files()));
    let h = harness_with(provider, RefUpdate::Ok, 1_000_000, todo_app());

    let v = h
        .service
        .open_round("todo", &open(0, "web"), Some("u-a"))
        .unwrap();
    assert_eq!(state(&v), "confirming", "deadline 0 converges right away");
    assert_eq!(v["round"]["plan"]["summary"], "早餐店點餐頁面，可以排序。");
    assert_eq!(v["round"]["confirmers"][0]["user"], "u-a");
    assert_eq!(v["round"]["you"]["mustConfirm"], true);
    let sent = h.provider.received();
    assert!(sent[0].system.contains("opinions from several people"));
    assert!(sent[0].messages[0].content.contains("（u-a）清單要能排序"));

    // Someone who gave no opinion cannot confirm.
    let err = h.service.confirm_round("todo", Some("u-x")).unwrap_err();
    assert_eq!(err.code(), "not_confirmer");

    let v = h.service.confirm_round("todo", Some("u-a")).unwrap();
    assert_eq!(state(&v), "done");
    assert_eq!(v["applied"]["mode"], "edit");
    assert!(v["round"]["result"]["commit"].is_string());
    assert_eq!(h.provider.call_count(), 2);

    let blobs = blob_contents(&h.github);
    let m = meta::parse("todo", &blobs[6]).unwrap();
    assert!(m.files.app_js.contains("sortItems"));
    let last = m.history.last().unwrap();
    assert_eq!((last.kind.as_str(), last.by.as_str()), ("edit", "u-a"));

    // Finished: a new round may start.
    assert!(h
        .service
        .open_round("todo", &open(30, "all"), Some("u-b"))
        .is_ok());
}

#[test]
fn opinions_until_the_deadline_then_everyone_confirms() {
    let provider = MockProvider::new()
        .then(ok(&changed_plan()))
        .then(ok(&changed_plan()))
        .then(ok(&changed_files()));
    let h = harness_with(provider, RefUpdate::Ok, 1_000_000, todo_app());

    let v = h
        .service
        .open_round("todo", &open(30, "web"), Some("u-a"))
        .unwrap();
    assert_eq!(state(&v), "collecting");
    h.service
        .add_opinion(
            "todo",
            &OpinionRequest {
                text: "排序要依到期日".into(),
            },
            Some("u-b"),
        )
        .unwrap();
    h.service.tick(now() + 10 * 60);
    assert_eq!(h.provider.call_count(), 0, "not due yet");
    h.service.tick(now() + 31 * 60);
    let v = h.service.round("todo", Some("u-b")).unwrap();
    assert_eq!(state(&v), "confirming");
    assert!(h.provider.received()[0].messages[0]
        .content
        .contains("（u-b）排序要依到期日"));
    let users: Vec<&str> = v["round"]["confirmers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["user"].as_str().unwrap())
        .collect();
    assert_eq!(users, ["u-a", "u-b"]);
    // The plan adds a web module: the scope stays web.
    assert_eq!(v["round"]["scope"], json!(["web"]));

    // u-b objects: back to collecting with the comment as an opinion.
    let v = h
        .service
        .object_in_round(
            "todo",
            &OpinionRequest {
                text: "改成依優先順序".into(),
            },
            Some("u-b"),
        )
        .unwrap();
    assert_eq!(state(&v), "collecting");
    assert_eq!(v["round"]["opinions"].as_array().unwrap().len(), 3);
    h.service.tick(now() + 11 * 60);
    let v = h.service.round("todo", None).unwrap();
    assert_eq!(state(&v), "confirming");

    // u-a confirms; the initiator skips u-b, which publishes.
    let v = h.service.confirm_round("todo", Some("u-a")).unwrap();
    assert_eq!(state(&v), "confirming");
    let err = h
        .service
        .skip_in_round("todo", &SkipRequest { user: "u-a".into() }, Some("u-b"))
        .unwrap_err();
    assert_eq!((err.status(), err.code()), (403, "not_initiator"));
    let v = h
        .service
        .skip_in_round("todo", &SkipRequest { user: "u-b".into() }, Some("u-a"))
        .unwrap();
    assert_eq!(state(&v), "done");
    let log = v["round"]["log"].to_string();
    assert!(log.contains("略過 u-b 的確認"), "{log}");
}

#[test]
fn a_running_round_blocks_other_changes_in_its_scope() {
    let h = harness_with(MockProvider::new(), RefUpdate::Ok, 1_000_000, todo_app());
    h.service
        .open_round("todo", &open(30, "api"), Some("u-a"))
        .unwrap();

    // Second round, direct plan, restore: refused.
    let err = h
        .service
        .open_round("todo", &open(5, "web"), Some("u-b"))
        .unwrap_err();
    assert_eq!((err.status(), err.code()), (409, "round_active"));
    let err = h
        .service
        .plan(
            &PlanRequestBody {
                prompt: "x".into(),
                app_name: None,
                base_app: Some("todo".into()),
            },
            Some("u-b"),
        )
        .unwrap_err();
    assert_eq!(err.code(), "round_active");

    // Listed with its state.
    let list = h.service.list_apps().unwrap();
    assert_eq!(list["apps"][0]["round"], "collecting");

    // Only the initiator cancels; then nothing is blocked.
    let err = h.service.cancel_round("todo", Some("u-b")).unwrap_err();
    assert_eq!(err.code(), "not_initiator");
    let v = h.service.cancel_round("todo", Some("u-a")).unwrap();
    assert_eq!(state(&v), "cancelled");
    assert!(h
        .service
        .open_round("todo", &open(5, "web"), Some("u-b"))
        .is_ok());
    assert_eq!(h.provider.call_count(), 0);
    assert_eq!(writes(&h.github), 0);
}

#[test]
fn a_failed_convergence_can_be_retried_by_the_initiator() {
    let provider = MockProvider::new()
        .then(ok("not json"))
        .then(ok(&changed_plan()));
    let h = harness_with(provider, RefUpdate::Ok, 1_000_000, todo_app());
    let v = h
        .service
        .open_round("todo", &open(0, "all"), Some("u-a"))
        .unwrap();
    assert_eq!(state(&v), "failed");
    assert!(!v["round"]["message"].as_str().unwrap().is_empty());
    assert_eq!(
        h.service
            .retry_round("todo", Some("u-b"))
            .unwrap_err()
            .code(),
        "not_initiator"
    );
    let v = h.service.retry_round("todo", Some("u-a")).unwrap();
    assert_eq!(state(&v), "confirming");
}

#[test]
fn unauthenticated_and_bad_input() {
    let h = harness_with(MockProvider::new(), RefUpdate::Ok, 1_000_000, todo_app());
    assert_eq!(
        h.service
            .open_round("todo", &open(0, "all"), None)
            .unwrap_err()
            .code(),
        "unauthenticated"
    );
    assert_eq!(
        h.service
            .open_round("todo", &open(0, "db"), Some("u-a"))
            .unwrap_err()
            .code(),
        "bad_request"
    );
    assert_eq!(
        h.service
            .open_round("nope", &open(0, "all"), Some("u-a"))
            .unwrap_err()
            .code(),
        "app_not_found"
    );
    let mut long = open(0, "all");
    long.deadline_minutes = 24 * 60 + 1;
    assert_eq!(
        h.service
            .open_round("todo", &long, Some("u-a"))
            .unwrap_err()
            .code(),
        "bad_request"
    );
    assert_eq!(
        h.service
            .add_opinion("todo", &OpinionRequest { text: "x".into() }, Some("u-a"))
            .unwrap_err()
            .code(),
        "round_state",
        "no round yet"
    );
}

const V1: &str = "1111111111111111111111111111111111111111";

fn with_hand_edit(files: &mut Value) {
    let py = files["api"]["handler.py"].as_str().unwrap().to_string();
    files["api"]["handler.py"] = json!(format!("# kept by hand\n{py}"));
}

#[test]
fn hand_edits_during_a_round_are_kept_or_nothing_is_published() {
    let todo = todo_app().remove(0);
    let versions = vec![(
        "todo".to_string(),
        V1.to_string(),
        todo.1.clone().unwrap(),
        "2026-10-14T01:00:00Z".to_string(),
    )];
    // The AI rewrites handler.py without the hand-added line first, then
    // keeps it after the round is converged again.
    let mut dropped = files_value();
    dropped["api"]["handler.py"] =
        json!("MENU = []\n\n\ndef handle(method, path, body):\n    return 200, {\"count\": 0}\n");
    let mut kept = dropped.clone();
    with_hand_edit(&mut kept);
    let provider = MockProvider::new()
        .then(ok(&changed_plan()))
        .then(ok(&dropped.to_string()))
        .then(ok(&changed_plan()))
        .then(ok(&kept.to_string()));
    let h = harness_versions(provider, RefUpdate::Ok, 1_000_000, vec![todo], versions);

    h.service
        .open_round("todo", &open(30, "web"), Some("u-a"))
        .unwrap();
    // While opinions are collected, u-b edits handler.py (outside the scope).
    let mut files = h.service.files("todo").unwrap()["files"].clone();
    with_hand_edit(&mut files);
    h.service
        .save_files(
            "todo",
            &laoban_m0::SaveFilesRequest {
                base_commit: V1.into(),
                files,
                note: None,
            },
            Some("u-b"),
        )
        .unwrap();
    h.service.tick(now() + 31 * 60);

    // The AI is told about the hand edit; its code drops it: not published.
    let v = h.service.confirm_round("todo", Some("u-a")).unwrap();
    assert_eq!(state(&v), "failed", "{v}");
    assert!(v["round"]["message"]
        .as_str()
        .unwrap()
        .contains("api/handler.py"));
    let sent = h.provider.received();
    assert!(sent[1].system.contains("lines that were added by hand"));
    assert!(sent[1]
        .messages
        .iter()
        .any(|m| m.content.contains("manual_edits") && m.content.contains("# kept by hand")));
    let commits_before = writes(&h.github);

    // Converged again; this time the code keeps it and is published.
    let v = h.service.retry_round("todo", Some("u-a")).unwrap();
    assert_eq!(state(&v), "confirming");
    let v = h.service.confirm_round("todo", Some("u-a")).unwrap();
    assert_eq!(state(&v), "done", "{v}");
    assert!(writes(&h.github) > commits_before);
    let blobs = blob_contents(&h.github);
    let m = blobs
        .iter()
        .rev()
        .find_map(|b| meta::parse("todo", b).ok())
        .unwrap();
    assert!(m.files.handler_py.contains("# kept by hand"));
    // The history shows the opinions, not the request sent to the AI.
    assert_eq!(m.history.last().unwrap().request, "（u-a）清單要能排序");
}
