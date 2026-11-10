//! Code written by two subagents at the same time (LAOBAN_AI_SUBAGENTS):
//! one for the web part, one for the API part.

mod common;

use common::*;
use laoban_ai::MockProvider;
use laoban_m0::codegen::parse_generated;
use laoban_m0::meta::{self, HistoryEntry};
use laoban_m0::plan::parse_app_plan;
use laoban_m0::{ApplyRequest, OpenRoundRequest, PlanRequestBody};
use serde_json::{json, Value};

fn on() -> laoban_m0::config::M0Settings {
    let mut s = settings(1_000_000);
    s.ai_subagents = true;
    s
}

fn plan_req() -> PlanRequestBody {
    PlanRequestBody {
        prompt: "做一個早餐店點餐頁面".into(),
        app_name: None,
        base_app: None,
        background: false,
    }
}

fn apply_req(v: &Value) -> ApplyRequest {
    ApplyRequest {
        plan_id: v["planId"].as_str().unwrap().into(),
        background: false,
    }
}

fn web_only() -> String {
    json!({ "web": files_value()["web"] }).to_string()
}

fn api_only() -> String {
    json!({ "api": files_value()["api"] }).to_string()
}

/// The system prompt of each code call, sorted, so the order in which the
/// two threads reached the mock does not matter.
fn code_prompts(h: &Harness) -> Vec<String> {
    let mut p: Vec<String> = h.provider.received()[1..]
        .iter()
        .map(|r| r.system.clone())
        .collect();
    p.sort();
    p
}

#[test]
fn a_new_app_is_written_by_a_web_and_an_api_subagent() {
    // Both answers repeat all four files: each subagent takes its own part.
    let provider = MockProvider::new()
        .then(ok(&plan_json()))
        .then(ok(&files_json()))
        .then(ok(&files_json()));
    let h = harness_settings(provider, RefUpdate::Ok, on(), Vec::new(), Vec::new());
    let plan = h.service.plan(&plan_req(), None).unwrap();
    let applied = h.service.apply(&apply_req(&plan), None).unwrap();

    assert_eq!(h.provider.call_count(), 3, "one plan call, two code calls");
    let prompts = code_prompts(&h);
    assert_eq!(
        prompts
            .iter()
            .filter(|p| p.contains("write the web front end"))
            .count(),
        1
    );
    assert_eq!(
        prompts
            .iter()
            .filter(|p| p.contains("write the API"))
            .count(),
        1
    );
    assert!(prompts
        .iter()
        .all(|p| p.contains("approved_plan.components.api.endpoints")));

    let runs = applied["subagents"].as_array().unwrap();
    assert_eq!(runs.len(), 2);
    for (run, part) in runs.iter().zip(["web", "api"]) {
        assert_eq!(run["part"], part);
        assert_eq!(run["state"], "done");
        assert_eq!(
            (run["inputTokens"].as_u64(), run["outputTokens"].as_u64()),
            (Some(120), Some(80))
        );
        assert_eq!(run["model"], MODEL);
    }
    assert_eq!(
        runs[0]["files"],
        json!(["web/index.html", "web/app.js", "web/style.css"])
    );
    // The fixture's handler.py has no /orders, which the plan lists.
    assert_eq!(
        applied["contractGaps"],
        json!([{ "kind": "apiMissing", "path": "/orders" }])
    );

    // The published files are the two parts put together.
    let blobs = blob_contents(&h.github);
    let m = blobs
        .iter()
        .find_map(|b| meta::parse("breakfast", b).ok())
        .unwrap();
    assert_eq!(m.files, parse_generated(&files_json()).unwrap());
}

#[test]
fn each_subagent_may_answer_with_its_part_only() {
    let provider = MockProvider::new()
        .then(ok(&plan_json()))
        .then(ok(&web_only()))
        .then(ok(&api_only()));
    let h = harness_settings(provider, RefUpdate::Ok, on(), Vec::new(), Vec::new());
    let plan = h.service.plan(&plan_req(), None).unwrap();
    // Which thread gets which answer is up to the scheduler: when the web
    // subagent got the API-only answer, it lacks its key and the apply
    // fails as invalid output; otherwise it succeeds. Either way nothing
    // half-written is published.
    match h.service.apply(&apply_req(&plan), None) {
        Ok(v) => {
            assert_eq!(v["subagents"][0]["state"], "done");
            assert_eq!(v["subagents"][1]["state"], "done");
        }
        Err(e) => {
            assert_eq!(e.code(), "ai_invalid_output");
            assert_eq!(writes(&h.github), 0);
        }
    }
}

#[test]
fn one_failing_subagent_publishes_nothing() {
    let provider = MockProvider::new()
        .then(ok(&plan_json()))
        .then(ok("not json"))
        .then(ok("not json"));
    let h = harness_settings(provider, RefUpdate::Ok, on(), Vec::new(), Vec::new());
    let plan = h.service.plan(&plan_req(), None).unwrap();
    let err = h.service.apply(&apply_req(&plan), None).unwrap_err();
    assert_eq!(err.code(), "ai_invalid_output");
    assert_eq!(writes(&h.github), 0);
}

#[test]
fn turned_off_one_call_writes_all_four_files() {
    let provider = MockProvider::new()
        .then(ok(&plan_json()))
        .then(ok(&files_json()));
    let h = harness(provider, RefUpdate::Ok, 1_000_000);
    let plan = h.service.plan(&plan_req(), None).unwrap();
    let applied = h.service.apply(&apply_req(&plan), None).unwrap();
    assert_eq!(h.provider.call_count(), 2);
    assert!(applied.get("subagents").is_none());
    assert_eq!(h.service.config_json()["subagents"], false);
}

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

/// The round's plan adds a web module only: the scope stays web.
fn web_plan() -> String {
    let mut p: Value = serde_json::from_str(&plan_json()).unwrap();
    p["components"]["web"]["modules"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "sorter", "purpose": "排序"}));
    p.to_string()
}

fn open(scope: &str) -> OpenRoundRequest {
    OpenRoundRequest {
        opinion: "清單要能排序".into(),
        items: Vec::new(),
        deadline_minutes: 0,
        scope: Some(scope.into()),
    }
}

#[test]
fn a_web_only_round_runs_the_web_subagent_and_keeps_the_api() {
    let mut web = files_value()["web"].clone();
    web["app.js"] = json!("fetch(\"api/menu\").then(r => r.json()).then(sortItems);\n");
    let provider = MockProvider::new()
        .then(ok(&web_plan()))
        .then(ok(&json!({ "web": web }).to_string()));
    let h = harness_settings(provider, RefUpdate::Ok, on(), todo_app(), Vec::new());
    h.service
        .open_round("todo", &open("web"), Some("u-a"))
        .unwrap();
    let v = h.service.confirm_round("todo", Some("u-a")).unwrap();
    assert_eq!(v["round"]["state"], "done", "{}", v["round"]["message"]);
    assert_eq!(
        h.provider.call_count(),
        2,
        "plan, then the web subagent only"
    );
    let sent = h.provider.received();
    assert!(sent[1].system.contains("write the web front end"));
    // The web subagent got the web files only as current code.
    let msg = &sent[1].messages[0].content;
    assert!(
        msg.contains("index.html") && !msg.contains("def handle("),
        "{msg}"
    );

    let runs = &v["applied"]["subagents"];
    assert_eq!(runs[0]["state"], "done");
    assert_eq!(runs[1]["state"], "skipped");
    let blobs = blob_contents(&h.github);
    let m = blobs
        .iter()
        .find_map(|b| meta::parse("todo", b).ok())
        .unwrap();
    assert!(m.files.app_js.contains("sortItems"));
    assert_eq!(
        m.files.handler_py,
        parse_generated(&files_json()).unwrap().handler_py,
        "API part kept as it was"
    );
}

#[test]
fn a_round_that_touches_the_api_rewrites_both_parts() {
    let provider = MockProvider::new()
        .then(ok(&plan_json()))
        .then(ok(&files_json()))
        .then(ok(&files_json()));
    let h = harness_settings(provider, RefUpdate::Ok, on(), todo_app(), Vec::new());
    h.service
        .open_round("todo", &open("api"), Some("u-a"))
        .unwrap();
    let v = h.service.confirm_round("todo", Some("u-a")).unwrap();
    assert_eq!(v["round"]["state"], "done", "{v}");
    assert_eq!(h.provider.call_count(), 3);
    assert_eq!(v["applied"]["subagents"][0]["state"], "done");
    assert_eq!(v["applied"]["subagents"][1]["state"], "done");
}
