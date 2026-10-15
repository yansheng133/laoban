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
use laoban_m0::{
    AddOpinionsRequest, OpenRoundRequest, OpinionItem, OpinionRequest, PlanRequestBody, SkipRequest,
};
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
        items: Vec::new(),
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
            &AddOpinionsRequest {
                text: "排序要依到期日".into(),
                ..Default::default()
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
    // Each entry also has a code and its values, for the page's language.
    assert!(log.contains(r#""code":"skipped""#), "{log}");
    assert!(log.contains(r#""user":"u-b""#), "{log}");
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
                background: false,
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
            .add_opinion(
                "todo",
                &AddOpinionsRequest {
                    text: "x".into(),
                    ..Default::default()
                },
                Some("u-a")
            )
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

#[test]
fn opinions_on_several_components_with_support_and_replies() {
    let provider = MockProvider::new().then(ok(&changed_plan()));
    let h = harness_with(provider, RefUpdate::Ok, 1_000_000, todo_app());

    // Open with two opinions on different components at once.
    let v = h
        .service
        .open_round(
            "todo",
            &OpenRoundRequest {
                opinion: String::new(),
                items: vec![
                    OpinionItem {
                        text: "網頁加上搜尋".into(),
                        target: Some("app-web".into()),
                    },
                    OpinionItem {
                        text: "API 回傳總數".into(),
                        target: Some("app-api".into()),
                    },
                ],
                deadline_minutes: 30,
                scope: Some("all".into()),
            },
            Some("u-a"),
        )
        .unwrap();
    let ops = v["round"]["opinions"].as_array().unwrap();
    assert_eq!(ops.len(), 2, "no empty general opinion: {v}");
    assert_eq!(ops[0]["target"]["id"], "app-web");
    assert_eq!(ops[0]["target"]["name"], "todo-web");

    // An unknown component is refused; the whole batch is.
    let err = h
        .service
        .add_opinion(
            "todo",
            &AddOpinionsRequest {
                items: vec![
                    OpinionItem {
                        text: "ok".into(),
                        target: None,
                    },
                    OpinionItem {
                        text: "x".into(),
                        target: Some("app-nope".into()),
                    },
                ],
                ..Default::default()
            },
            Some("u-b"),
        )
        .unwrap_err();
    assert_eq!(err.code(), "bad_request");

    // u-b supports o1 and replies under o2.
    h.service
        .support_opinion("todo", "o1", Some("u-b"))
        .unwrap();
    let v = h
        .service
        .reply_to_opinion(
            "todo",
            "o2",
            &OpinionRequest {
                text: "含已完成的嗎？".into(),
            },
            Some("u-b"),
        )
        .unwrap();
    assert_eq!(v["round"]["opinions"][0]["supporters"], json!(["u-b"]));
    assert_eq!(v["round"]["opinions"][1]["replies"][0]["by"], "u-b");
    assert_eq!(v["round"]["you"]["canDiscuss"], true);
    let err = h
        .service
        .support_opinion("todo", "o9", Some("u-b"))
        .unwrap_err();
    assert_eq!((err.status(), err.code()), (404, "opinion_not_found"));

    // Converge: the AI gets them grouped by component with the support.
    h.service.tick(now() + 31 * 60);
    let sent = &h.provider.received()[0].messages[0].content;
    assert!(sent.contains("【元件：todo-web】"), "{sent}");
    assert!(sent.contains("1 人附議：u-b"), "{sent}");
    assert!(sent.contains("回覆（u-b）含已完成的嗎？"), "{sent}");
    // Only the author confirms (u-b only supported and replied).
    let v = h.service.round("todo", Some("u-b")).unwrap();
    let users: Vec<&str> = v["round"]["confirmers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["user"].as_str().unwrap())
        .collect();
    assert_eq!(users, ["u-a"]);
}

/// Wait (up to 20 s) until the round reaches `want`.
fn wait_for(service: &laoban_m0::M0Service, user: &str, want: &str) -> Value {
    for _ in 0..400 {
        let v = service.round("todo", Some(user)).unwrap();
        if state(&v) == want {
            return v;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("round never reached {want}");
}

#[test]
fn attached_service_answers_at_once_and_works_in_the_background() {
    // Report 18, B8: requests through the Kubernetes API server end after
    // about 60 s; publishing took 114 s. In the server the AI work runs in
    // the background and each request returns right away.
    let provider = MockProvider::new()
        .then(ok(&changed_plan()))
        .then(ok(&changed_files()));
    let (service, gate, github) = gated_service(provider, todo_app());

    // Deadline 0: the response says "converging" while the AI is held.
    let v = service
        .open_round("todo", &open(0, "web"), Some("u-a"))
        .unwrap();
    assert_eq!(state(&v), "converging");
    gate.open();
    let v = wait_for(&service, "u-a", "confirming");
    assert_eq!(v["round"]["plan"]["summary"], "早餐店點餐頁面，可以排序。");

    // The last confirmation: "applying" at once, no `applied` yet.
    let before = writes(&github);
    let v = service.confirm_round("todo", Some("u-a")).unwrap();
    assert_eq!(state(&v), "applying");
    assert!(v.get("applied").is_none());
    assert_eq!(
        writes(&github),
        before,
        "nothing published before the AI answers"
    );

    // A second confirm while it works is refused, not run twice.
    assert!(service.confirm_round("todo", Some("u-a")).is_err());

    gate.open();
    let v = wait_for(&service, "u-a", "done");
    assert!(v["round"]["result"]["commit"].is_string(), "{v}");
    assert!(writes(&github) > before);
    assert_eq!(gate.calls(), 2);
}

#[test]
fn background_failure_is_recorded_on_the_round() {
    let provider = MockProvider::new()
        .then(ok(&changed_plan()))
        .then(ok("not json"));
    let (service, gate, _github) = gated_service(provider, todo_app());

    service
        .open_round("todo", &open(0, "web"), Some("u-a"))
        .unwrap();
    gate.open();
    wait_for(&service, "u-a", "confirming");
    let v = service.confirm_round("todo", Some("u-a")).unwrap();
    assert_eq!(state(&v), "applying");
    // Let any retries through too; the script then runs out.
    for _ in 0..5 {
        gate.open();
    }
    let v = wait_for(&service, "u-a", "failed");
    assert!(v["round"]["message"]
        .as_str()
        .is_some_and(|m| !m.is_empty()));
    assert!(v.get("applied").is_none());
}
