//! v0.1.16 round features through the service: editing and withdrawing
//! opinions, moving the deadline, drafts kept in the round, comments on the
//! plan's steps, what waits for whom, and archived rounds. Mock AI and
//! GitHub; synthetic users.

mod common;

use std::time::{SystemTime, UNIX_EPOCH};

use common::*;
use laoban_ai::MockProvider;
use laoban_m0::codegen::parse_generated;
use laoban_m0::meta::{self, HistoryEntry};
use laoban_m0::plan::parse_app_plan;
use laoban_m0::{
    AddOpinionsRequest, DeadlineRequest, DraftsRequest, OpenRoundRequest, OpinionItem,
    OpinionRequest,
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

fn changed_plan() -> String {
    let mut p: Value = serde_json::from_str(&plan_json()).unwrap();
    p["summary"] = json!("早餐店點餐頁面，可以排序。");
    p.to_string()
}

fn open(minutes: u64) -> OpenRoundRequest {
    OpenRoundRequest {
        opinion: "清單要能排序".into(),
        items: Vec::new(),
        deadline_minutes: minutes,
        scope: Some("web".into()),
    }
}

fn text(t: &str) -> OpinionRequest {
    OpinionRequest { text: t.into() }
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
fn edit_withdraw_and_an_empty_round_ends_by_itself() {
    let h = harness_with(MockProvider::new(), RefUpdate::Ok, 1_000_000, todo_app());
    h.service
        .open_round("todo", &open(30), Some("u-a"))
        .unwrap();
    h.service
        .add_opinion(
            "todo",
            &AddOpinionsRequest {
                text: "也要能篩選".into(),
                ..Default::default()
            },
            Some("u-b"),
        )
        .unwrap();
    h.service
        .support_opinion("todo", "o2", Some("u-a"))
        .unwrap();
    let err = h
        .service
        .edit_opinion("todo", "o2", &text("改掉"), Some("u-a"))
        .unwrap_err();
    assert_eq!(err.code(), "bad_request", "only the author");
    let v = h
        .service
        .edit_opinion("todo", "o2", &text("依狀態篩選"), Some("u-b"))
        .unwrap();
    let o = &v["round"]["opinions"][1];
    assert_eq!(o["text"], "依狀態篩選");
    assert_eq!(o["editedAfterSupport"], true);
    let v = h
        .service
        .withdraw_opinion("todo", "o2", Some("u-b"))
        .unwrap();
    let o = &v["round"]["opinions"][1];
    assert_eq!(
        (o["withdrawn"].as_bool(), o["text"].as_str()),
        (Some(true), Some(""))
    );
    // The initiator withdraws too: nothing is left at the deadline.
    h.service
        .withdraw_opinion("todo", "o1", Some("u-a"))
        .unwrap();
    h.service.tick(now() + 31 * 60);
    let v = h.service.round("todo", Some("u-a")).unwrap();
    assert_eq!(state(&v), "cancelled");
    assert_eq!(h.provider.call_count(), 0, "no AI call for nothing");
    assert_eq!(
        v["round"]["log"].as_array().unwrap().last().unwrap()["code"],
        "emptyCancelled"
    );
}

#[test]
fn the_initiator_moves_the_deadline_or_converges_now() {
    let h = harness_with(
        MockProvider::new().then(ok(&changed_plan())),
        RefUpdate::Ok,
        1_000_000,
        todo_app(),
    );
    h.service
        .open_round("todo", &open(30), Some("u-a"))
        .unwrap();
    let err = h
        .service
        .set_round_deadline("todo", &DeadlineRequest { minutes: 60 }, Some("u-b"))
        .unwrap_err();
    assert_eq!(err.code(), "not_initiator");
    let v = h
        .service
        .set_round_deadline("todo", &DeadlineRequest { minutes: 120 }, Some("u-a"))
        .unwrap();
    let deadline = v["round"]["deadline"].as_u64().unwrap();
    assert!(deadline >= now() + 119 * 60, "{deadline}");
    let err = h
        .service
        .set_round_deadline(
            "todo",
            &DeadlineRequest {
                minutes: 24 * 60 + 1,
            },
            Some("u-a"),
        )
        .unwrap_err();
    assert_eq!(err.code(), "bad_request");
    let v = h
        .service
        .set_round_deadline("todo", &DeadlineRequest { minutes: 0 }, Some("u-a"))
        .unwrap();
    assert_eq!(state(&v), "confirming", "converged right away");
}

#[test]
fn drafts_are_kept_in_the_round_and_shared_by_default() {
    let h = harness_with(MockProvider::new(), RefUpdate::Ok, 1_000_000, todo_app());
    h.service
        .open_round("todo", &open(30), Some("u-a"))
        .unwrap();
    let draft = |t: &str, target: Option<&str>| OpinionItem {
        text: t.into(),
        target: target.map(str::to_string),
    };
    h.service
        .save_drafts(
            "todo",
            &DraftsRequest {
                items: vec![
                    draft("網頁加搜尋", Some("app-web")),
                    draft("整體字大一點", None),
                ],
                shared: None,
            },
            Some("u-b"),
        )
        .unwrap();
    h.service
        .save_drafts(
            "todo",
            &DraftsRequest {
                items: vec![draft("只有我看得到", None)],
                shared: Some(false),
            },
            Some("u-c"),
        )
        .unwrap();
    // u-b on another computer gets them back; u-a sees u-b's, not u-c's.
    let v = h.service.round("todo", Some("u-b")).unwrap();
    let mine = &v["round"]["drafts"]["mine"];
    assert_eq!(mine["shared"], true);
    assert_eq!(mine["items"][0]["target"]["name"], "todo-web");
    let v = h.service.round("todo", Some("u-a")).unwrap();
    let shared = v["round"]["drafts"]["shared"].as_array().unwrap();
    assert_eq!(shared.len(), 1);
    assert_eq!(shared[0]["user"], "u-b");
    assert!(!v.to_string().contains("只有我看得到"));
    // A target not in the architecture is refused, as for opinions.
    let err = h
        .service
        .save_drafts(
            "todo",
            &DraftsRequest {
                items: vec![draft("x", Some("no-such-node"))],
                shared: None,
            },
            Some("u-b"),
        )
        .unwrap_err();
    assert_eq!(err.code(), "bad_request");
    // Drafts are not opinions: nothing in the log, nothing counted.
    assert_eq!(v["round"]["used"]["opinions"], 1);
}

#[test]
fn comments_on_the_plan_reach_the_ai_after_an_objection() {
    let h = harness_with(
        MockProvider::new()
            .then(ok(&changed_plan()))
            .then(ok(&changed_plan())),
        RefUpdate::Ok,
        1_000_000,
        todo_app(),
    );
    let v = h.service.open_round("todo", &open(0), Some("u-a")).unwrap();
    assert_eq!(state(&v), "confirming");
    let v = h
        .service
        .comment_on_plan("todo", 1, &text("第一步先只做排序"), Some("u-b"))
        .unwrap();
    assert_eq!(v["round"]["planComments"][0]["id"], "c1");
    assert_eq!(v["round"]["planComments"][0]["step"], 1);
    assert_eq!(v["round"]["you"]["canComment"], true);
    h.service
        .support_plan_comment("todo", "c1", Some("u-a"))
        .unwrap();
    let err = h
        .service
        .comment_on_plan("todo", 99, &text("x"), Some("u-b"))
        .unwrap_err();
    assert_eq!(err.code(), "bad_request");
    // u-b commented but gave no opinion: still not a confirmer.
    let err = h.service.confirm_round("todo", Some("u-b")).unwrap_err();
    assert_eq!(err.code(), "not_confirmer");
    h.service
        .object_in_round("todo", &text("縮小範圍"), Some("u-a"))
        .unwrap();
    h.service.tick(now() + 11 * 60);
    let sent = h.provider.received();
    let req = &sent[1].messages[0].content;
    assert!(
        req.contains("第 1 步") && req.contains("第一步先只做排序"),
        "{req}"
    );
    let v = h.service.round("todo", Some("u-a")).unwrap();
    assert_eq!(state(&v), "confirming");
    assert!(
        v["round"].get("planComments").is_none(),
        "cleared with the new plan"
    );
}

#[test]
fn what_waits_for_whom() {
    let h = harness_with(
        MockProvider::new().then(ok(&changed_plan())),
        RefUpdate::Ok,
        1_000_000,
        todo_app(),
    );
    h.service
        .open_round("todo", &open(30), Some("u-a"))
        .unwrap();
    h.service
        .add_opinion(
            "todo",
            &AddOpinionsRequest {
                text: "也要能篩選".into(),
                ..Default::default()
            },
            Some("u-b"),
        )
        .unwrap();
    let mine = |u: &str| h.service.rounds_waiting(Some(u)).unwrap()["rounds"].clone();
    assert_eq!(mine("u-b"), json!([]), "collecting: nothing to confirm yet");
    h.service.tick(now() + 31 * 60);
    assert_eq!(mine("u-b")[0]["reason"], "confirm");
    assert_eq!(mine("u-b")[0]["app"], "todo");
    assert_eq!(mine("u-a")[0]["reason"], "confirm");
    h.service.confirm_round("todo", Some("u-a")).unwrap();
    assert_eq!(mine("u-a")[0]["reason"], "othersConfirm");
    assert_eq!(mine("u-a")[0]["pending"], 1);
    assert_eq!(mine("u-c"), json!([]));
    assert_eq!(
        h.service.rounds_waiting(None).unwrap_err().code(),
        "unauthenticated"
    );
}

#[test]
fn archived_rounds_can_be_listed_and_read() {
    let h = harness_with(MockProvider::new(), RefUpdate::Ok, 1_000_000, todo_app());
    assert_eq!(h.service.past_rounds("todo").unwrap()["rounds"], json!([]));
    let id_no = with_check_digit("A12345678");
    let v = h
        .service
        .open_round(
            "todo",
            &OpenRoundRequest {
                opinion: format!("聯絡人 {id_no} 要能查"),
                ..open(30)
            },
            Some("u-a"),
        )
        .unwrap();
    let id = v["round"]["id"].as_str().unwrap().to_string();
    h.service.cancel_round("todo", Some("u-a")).unwrap();
    h.service.archive_tick(now());
    let list = h.service.past_rounds("todo").unwrap();
    assert_eq!(list["rounds"][0]["id"], id);
    assert_eq!(list["rounds"][0]["date"].as_str().unwrap().len(), 10);
    let one = h.service.past_round("todo", &id).unwrap();
    assert_eq!(one["round"]["state"], "cancelled");
    assert!(one["openedAt"].is_string());
    assert!(!one.to_string().contains(&id_no), "masked as archived");
    assert_eq!(
        h.service
            .past_round("todo", "r-20260101-000000")
            .unwrap_err()
            .code(),
        "round_not_found"
    );
    assert_eq!(
        h.service.past_round("todo", "../x").unwrap_err().code(),
        "bad_request"
    );
}

#[test]
fn plan_comments_are_masked_in_the_archive() {
    let h = harness_with(
        MockProvider::new().then(ok(&changed_plan())),
        RefUpdate::Ok,
        1_000_000,
        todo_app(),
    );
    let id_no = with_check_digit("A12345678");
    let v = h.service.open_round("todo", &open(0), Some("u-a")).unwrap();
    let id = v["round"]["id"].as_str().unwrap().to_string();
    h.service
        .comment_on_plan(
            "todo",
            1,
            &text(&format!("聯絡人 {id_no} 先處理")),
            Some("u-b"),
        )
        .unwrap();
    h.service.cancel_round("todo", Some("u-a")).unwrap();
    h.service.archive_tick(now());
    let one = h.service.past_round("todo", &id).unwrap();
    let s = one.to_string();
    assert!(s.contains("先處理"), "{s}");
    assert!(!s.contains(&id_no), "plan comments are masked as archived");
}

#[test]
fn a_withdrawn_opinion_takes_its_replies_and_supports_with_it() {
    let h = harness_with(MockProvider::new(), RefUpdate::Ok, 1_000_000, todo_app());
    h.service
        .open_round("todo", &open(30), Some("u-a"))
        .unwrap();
    h.service
        .support_opinion("todo", "o1", Some("u-b"))
        .unwrap();
    h.service
        .reply_to_opinion("todo", "o1", &text("同意這個"), Some("u-b"))
        .unwrap();
    let v = h
        .service
        .withdraw_opinion("todo", "o1", Some("u-a"))
        .unwrap();
    let o = &v["round"]["opinions"][0];
    assert_eq!(o["withdrawn"], true);
    assert_eq!(o["replies"], json!([]));
    assert_eq!(o["supporters"], json!([]));
    assert!(!v.to_string().contains("同意這個"));
}
