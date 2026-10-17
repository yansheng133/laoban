//! Rounds kept in a store across backend restarts: two services on the
//! same store and the same mock GitHub stand for the backend before and
//! after a restart. Also: writes that collide, a refused write, a missing
//! CRD, archiving finished rounds and the size limits. All data synthetic.

mod common;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use common::*;
use laoban_ai::MockProvider;
use laoban_m0::codegen::parse_generated;
use laoban_m0::meta::{self, HistoryEntry};
use laoban_m0::plan::parse_app_plan;
use laoban_m0::state::ROUNDS;
use laoban_m0::{
    AddOpinionsRequest, M0Service, MemoryStore, OpenRoundRequest, OpinionRequest, RoundLimits,
    StateStore,
};
use laoban_mockhttp::MockServer;
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

fn open(minutes: u64) -> OpenRoundRequest {
    OpenRoundRequest {
        opinion: "清單要能排序".into(),
        items: Vec::new(),
        deadline_minutes: minutes,
        scope: Some("web".into()),
    }
}

fn opinion(text: &str) -> AddOpinionsRequest {
    AddOpinionsRequest {
        text: text.into(),
        ..Default::default()
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

/// A backend on `store` and `github`, with the stored rounds read back.
fn backend(provider: MockProvider, github: &MockServer, store: &Arc<MemoryStore>) -> Backend {
    let provider = Arc::new(provider);
    let mut service = M0Service::new(
        settings(1_000_000),
        Box::new(Arc::clone(&provider)),
        committer(github),
    )
    .unwrap();
    service.use_state_store(Arc::clone(store) as Arc<dyn StateStore>);
    let _ = service.restore_rounds();
    Backend { service, provider }
}

struct Backend {
    service: M0Service,
    provider: Arc<MockProvider>,
}

fn round_of(b: &Backend) -> Value {
    b.service.round("todo", Some("u-a")).unwrap()
}

/// The one stored round's spec.
fn stored(store: &MemoryStore) -> Value {
    let list = store.list(ROUNDS).unwrap();
    assert_eq!(list.len(), 1, "{list:?}");
    list[0].spec.clone()
}

fn set_stored_state(store: &MemoryStore, state: &str) {
    let name = store.list(ROUNDS).unwrap()[0].name.clone();
    store.touch(ROUNDS, &name, |v| {
        v["state"] = json!(state);
        v["round"]["state"] = json!(state);
    });
}

#[test]
fn collecting_and_confirming_rounds_survive_a_restart() {
    let github = github_mock_with(RefUpdate::Ok, todo_app());
    let store = Arc::new(MemoryStore::new());

    // Before the restart: opinions, +1 and a reply.
    let a = backend(MockProvider::new(), &github, &store);
    a.service
        .open_round("todo", &open(30), Some("u-a"))
        .unwrap();
    a.service
        .add_opinion("todo", &opinion("也要能篩選"), Some("u-b"))
        .unwrap();
    a.service
        .support_opinion("todo", "o2", Some("u-a"))
        .unwrap();
    a.service
        .reply_to_opinion(
            "todo",
            "o1",
            &OpinionRequest {
                text: "用到期日".into(),
            },
            Some("u-c"),
        )
        .unwrap();
    let before = round_of(&a);
    let spec = stored(&store);
    assert_eq!(spec["app"], "todo");
    assert_eq!(spec["state"], "collecting");
    assert_eq!(spec["targetNamespace"], "laoban-dev");
    assert_eq!(
        spec["round"]["startCommit"], BASE_COMMIT,
        "a commit, not the files"
    );
    assert!(
        spec.to_string().len() < 8 * 1024,
        "{}",
        spec.to_string().len()
    );
    drop(a);

    // After: the same round, and it goes on.
    let b = backend(
        MockProvider::new()
            .then(ok(&changed_plan()))
            .then(ok(&changed_files())),
        &github,
        &store,
    );
    let after = round_of(&b);
    assert_eq!(after["round"], before["round"]);
    assert_eq!(after["roundsAvailable"], true);
    let v = b
        .service
        .add_opinion("todo", &opinion("字大一點"), Some("u-c"))
        .unwrap();
    assert_eq!(v["round"]["opinions"][2]["id"], "o3", "IDs go on");
    b.service.tick(now() + 31 * 60);
    assert_eq!(state(&round_of(&b)), "confirming");
    let spec = stored(&store);
    assert_eq!(spec["state"], "confirming");
    assert_eq!(spec["round"]["recipe"]["baseCommit"], BASE_COMMIT);
    drop(b);

    // Restart while confirming: the plan store is empty, the round's
    // recipe brings the plan back and it is published once.
    let c = backend(
        MockProvider::new().then(ok(&changed_files())),
        &github,
        &store,
    );
    for u in ["u-a", "u-b"] {
        c.service.confirm_round("todo", Some(u)).unwrap();
    }
    let v = c.service.confirm_round("todo", Some("u-c")).unwrap();
    assert_eq!(state(&v), "done", "{v}");
    assert_eq!(c.provider.call_count(), 1, "only the code, no new plan");
    let sent = c.provider.received();
    assert!(
        sent[0]
            .messages
            .iter()
            .any(|m| m.content.contains("current_code")),
        "the code is written from the app's files at the plan's commit"
    );
    let m = blob_contents(&github)
        .iter()
        .rev()
        .find_map(|b| meta::parse("todo", b).ok())
        .unwrap();
    assert!(m.files.app_js.contains("sortItems"));
    assert!(m
        .history
        .last()
        .unwrap()
        .request
        .contains("(u-b) 也要能篩選"));
    assert_eq!(stored(&store)["state"], "done");
}

#[test]
fn a_restart_while_converging_converges_again() {
    let github = github_mock_with(RefUpdate::Ok, todo_app());
    let store = Arc::new(MemoryStore::new());
    let a = backend(MockProvider::new(), &github, &store);
    a.service
        .open_round("todo", &open(30), Some("u-a"))
        .unwrap();
    drop(a);
    // The backend stopped while the AI was converging.
    set_stored_state(&store, "converging");

    let b = backend(
        MockProvider::new().then(ok(&changed_plan())),
        &github,
        &store,
    );
    let v = round_of(&b);
    assert_eq!(state(&v), "confirming", "{v}");
    assert_eq!(b.provider.call_count(), 1);
    let codes: Vec<&str> = v["round"]["log"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"restarted"), "{codes:?}");
}

#[test]
fn a_restart_while_publishing_never_publishes_twice() {
    // Published before the restart, but "done" was not written yet.
    let github = github_mock_with(RefUpdate::Ok, todo_app());
    let store = Arc::new(MemoryStore::new());
    let a = backend(
        MockProvider::new()
            .then(ok(&changed_plan()))
            .then(ok(&changed_files())),
        &github,
        &store,
    );
    a.service.open_round("todo", &open(0), Some("u-a")).unwrap();
    let applying = stored(&store);
    let v = a.service.confirm_round("todo", Some("u-a")).unwrap();
    assert_eq!(state(&v), "done");
    // Someone arranges the 3D view afterwards: the round's version is no
    // longer the last entry in the history.
    let commit = a.service.files("todo").unwrap()["commit"]
        .as_str()
        .unwrap()
        .to_string();
    a.service
        .save_layout(
            "todo",
            &laoban_m0::SaveLayoutRequest {
                base_commit: commit,
                positions: [(
                    "app-web".to_string(),
                    laoban_m0::meta::Pos { x: 1.0, z: 2.0 },
                )]
                .into_iter()
                .collect(),
            },
            Some("u-b"),
        )
        .unwrap();
    drop(a);
    let name = store.list(ROUNDS).unwrap()[0].name.clone();
    store.touch(ROUNDS, &name, |spec| {
        let mut r = applying["round"].clone();
        r["state"] = json!("applying");
        r["log"].as_array_mut().unwrap().push(
            json!({"at": "2026-01-01T00:00:00Z", "by": "laoban", "event": "x", "code": "applying"}),
        );
        spec["state"] = json!("applying");
        spec["round"] = r;
    });
    let commits = writes(&github);
    let b = backend(MockProvider::new(), &github, &store);
    let v = round_of(&b);
    assert_eq!(state(&v), "done", "{v}");
    assert!(v["round"]["result"]["commit"].is_string());
    // Written to GitHub; not yet running (report 20, N6).
    let done = v["round"]["log"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(done["code"], "done");
    assert!(done["event"]
        .as_str()
        .unwrap()
        .contains("Fleet deploys it next"));
    assert_eq!(writes(&github), commits, "nothing published again");
    assert_eq!(b.provider.call_count(), 0);

    // Not published before the restart: failed, the initiator converges again.
    let github = github_mock_with(RefUpdate::Ok, todo_app());
    let store = Arc::new(MemoryStore::new());
    let a = backend(
        MockProvider::new().then(ok(&changed_plan())),
        &github,
        &store,
    );
    a.service.open_round("todo", &open(0), Some("u-a")).unwrap();
    drop(a);
    set_stored_state(&store, "applying");
    let b = backend(MockProvider::new(), &github, &store);
    let v = round_of(&b);
    assert_eq!(state(&v), "failed", "{v}");
    assert!(v["round"]["message"]
        .as_str()
        .unwrap()
        .contains("converge again"));
    // The page shows it in its own language (report 20, N7).
    assert_eq!(v["round"]["messageCode"], "restartUnpublished");
    let failed = v["round"]["log"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(
        (failed["code"].as_str(), failed["args"]["code"].as_str()),
        (Some("failed"), Some("restartUnpublished"))
    );
    assert_eq!(writes(&github), 0);
}

#[test]
fn colliding_and_refused_writes() {
    let github = github_mock_with(RefUpdate::Ok, todo_app());
    let store = Arc::new(MemoryStore::new());
    let a = backend(MockProvider::new(), &github, &store);
    a.service
        .open_round("todo", &open(30), Some("u-a"))
        .unwrap();

    // Someone else (another replica, kubectl) adds a +1 meanwhile: the
    // next change is made on top of it, nothing is lost.
    let name = store.list(ROUNDS).unwrap()[0].name.clone();
    store.touch(ROUNDS, &name, |v| {
        v["round"]["opinions"][0]["supporters"] = json!(["u-z"]);
    });
    let v = a
        .service
        .add_opinion("todo", &opinion("也要能篩選"), Some("u-b"))
        .unwrap();
    assert_eq!(v["round"]["opinions"][0]["supporters"], json!(["u-z"]));
    assert_eq!(v["round"]["opinions"].as_array().unwrap().len(), 2);
    assert_eq!(
        stored(&store)["round"]["opinions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // A refused write changes nothing, in the store or on the page.
    store.fail_next_writes(1);
    let err = a
        .service
        .add_opinion("todo", &opinion("第三則"), Some("u-c"))
        .unwrap_err();
    assert_eq!((err.code(), err.status()), ("state_failed", 503));
    assert_eq!(
        round_of(&a)["round"]["opinions"].as_array().unwrap().len(),
        2
    );
    // The next try works.
    a.service
        .add_opinion("todo", &opinion("第三則"), Some("u-c"))
        .unwrap();
}

#[test]
fn without_the_crd_rounds_wait_and_the_rest_works() {
    let github = github_mock_with(RefUpdate::Ok, todo_app());
    let store = Arc::new(MemoryStore::new());
    store.set_installed(false);
    let a = backend(MockProvider::new(), &github, &store);
    let v = round_of(&a);
    assert_eq!(v["roundsAvailable"], false);
    assert_eq!(a.service.config_json()["roundsAvailable"], false);
    let err = a
        .service
        .open_round("todo", &open(30), Some("u-a"))
        .unwrap_err();
    assert_eq!((err.code(), err.status()), ("rounds_unavailable", 503));
    assert!(err.user_message().contains("laoban-crd"));
    // The rest of the app works.
    assert!(a.service.get_app("todo", Some("u-a")).is_ok());

    // Installed later: the timer reads the (empty) store and rounds work.
    store.set_installed(true);
    a.service.tick(now());
    assert_eq!(round_of(&a)["roundsAvailable"], true);
    a.service
        .open_round("todo", &open(30), Some("u-a"))
        .unwrap();
}

#[test]
fn finished_rounds_are_archived_masked_then_deleted() {
    let github = github_mock_with(RefUpdate::Ok, todo_app());
    let store = Arc::new(MemoryStore::new());
    let a = backend(MockProvider::new(), &github, &store);
    let id = with_check_digit("A12345678");
    a.service
        .open_round(
            "todo",
            &OpenRoundRequest {
                opinion: format!("聯絡人身分證 {id} 要能查"),
                ..open(30)
            },
            Some("u-a"),
        )
        .unwrap();
    a.service
        .reply_to_opinion(
            "todo",
            "o1",
            &OpinionRequest {
                text: format!("是 {id} 嗎"),
            },
            Some("u-b"),
        )
        .unwrap();
    // Not finished: nothing archived.
    a.service.archive_tick(now());
    assert_eq!(writes(&github), 0);

    a.service.cancel_round("todo", Some("u-a")).unwrap();
    a.service.archive_tick(now());
    let requests = github.requests();
    let created = requests
        .iter()
        .find(|r| r.method == "POST" && r.path.ends_with("/git/refs"))
        .expect("archive branch created");
    assert!(created.body_str().contains("refs/heads/laoban-rounds"));
    let tree = requests
        .iter()
        .find(|r| r.method == "POST" && r.path.ends_with("/git/trees"))
        .unwrap();
    let tree: Value = serde_json::from_str(&tree.body_str()).unwrap();
    assert!(
        tree.get("base_tree").is_none(),
        "no history from main: {tree}"
    );
    let path = tree["tree"][0]["path"].as_str().unwrap();
    assert!(
        path.starts_with("rounds/todo/r-") && path.ends_with(".json"),
        "{path}"
    );
    let commit = requests
        .iter()
        .find(|r| r.method == "POST" && r.path.ends_with("/git/commits"))
        .unwrap();
    let commit: Value = serde_json::from_str(&commit.body_str()).unwrap();
    assert_eq!(commit["parents"], json!([]));
    let doc: Value = blob_contents(&github)
        .iter()
        .find_map(|b| {
            serde_json::from_str::<Value>(b)
                .ok()
                .filter(|v| v["round"].is_object())
        })
        .unwrap();
    assert_eq!(doc["app"], "todo");
    assert_eq!(doc["round"]["state"], "cancelled");
    let text = doc.to_string();
    assert!(!text.contains(&id), "opinions are masked in the archive");
    assert!(text.contains("要能查") && text.contains("u-b"));
    assert!(doc["round"].get("recipe").is_none() && doc["round"].get("startCommit").is_none());
    assert_eq!(stored(&store)["round"]["archived"], true);
    // The main branch was not touched.
    assert!(!requests
        .iter()
        .any(|r| r.path.ends_with("/refs/heads/main") && r.method == "PATCH"));

    // Archived once only.
    let n = writes(&github);
    a.service.archive_tick(now());
    assert_eq!(writes(&github), n);

    // The next round replaces it: the old resource is deleted.
    let old = store.list(ROUNDS).unwrap()[0].name.clone();
    a.service
        .open_round("todo", &open(30), Some("u-b"))
        .unwrap();
    let names: Vec<String> = store
        .list(ROUNDS)
        .unwrap()
        .into_iter()
        .map(|o| o.name)
        .collect();
    assert_eq!(names.len(), 1);
    assert_ne!(names[0], old);

    // The second archive moves the branch instead of creating it.
    a.service.cancel_round("todo", Some("u-b")).unwrap();
    a.service.archive_tick(now());
    assert!(github
        .requests()
        .iter()
        .any(|r| r.method == "PATCH" && r.path.ends_with("/refs/heads/laoban-rounds")));
}

#[test]
fn a_failed_archive_is_retried_later_and_the_round_kept() {
    // GitHub refuses moving any branch.
    let github = github_mock_with(RefUpdate::Status(422), todo_app());
    let store = Arc::new(MemoryStore::new());
    let a = backend(MockProvider::new(), &github, &store);
    a.service
        .open_round("todo", &open(30), Some("u-a"))
        .unwrap();
    a.service.cancel_round("todo", Some("u-a")).unwrap();
    let tries = |g: &MockServer| {
        g.requests()
            .iter()
            .filter(|r| r.method == "POST" && r.path.ends_with("/git/refs"))
            .count()
    };
    let t = now();
    a.service.archive_tick(t);
    assert_eq!(tries(&github), 1);
    assert_eq!(stored(&store)["round"]["archived"], false);
    a.service.archive_tick(t + 5);
    assert_eq!(tries(&github), 1, "not again right away");
    a.service.archive_tick(t + 3600);
    assert_eq!(tries(&github), 2, "retried later");

    // A new round meanwhile: the old one stays stored until archived.
    a.service
        .open_round("todo", &open(30), Some("u-b"))
        .unwrap();
    assert_eq!(store.list(ROUNDS).unwrap().len(), 2);
    drop(a);
    // After a restart too: still queued for the archive.
    let b = backend(MockProvider::new(), &github, &store);
    assert_eq!(
        state(&round_of(&b)),
        "collecting",
        "the newest is the app's round"
    );
    b.service.archive_tick(now() + 7200);
    assert_eq!(tries(&github), 3);
    assert_eq!(store.list(ROUNDS).unwrap().len(), 2);
}

#[test]
fn limits_are_checked_and_shown() {
    let github = github_mock_with(RefUpdate::Ok, todo_app());
    let store = Arc::new(MemoryStore::new());
    let a = backend(MockProvider::new(), &github, &store);
    let v = a
        .service
        .open_round("todo", &open(30), Some("u-a"))
        .unwrap();
    assert_eq!(v["round"]["limits"]["opinionChars"], 1000);
    assert_eq!(v["limits"]["replyChars"], 300);
    assert_eq!(v["round"]["used"]["chars"], 6);

    let err = a
        .service
        .add_opinion("todo", &opinion(&"字".repeat(1001)), Some("u-b"))
        .unwrap_err();
    assert_eq!((err.code(), err.status()), ("too_long", 400));
    assert!(err.user_message().contains("1000"));
    a.service
        .add_opinion("todo", &opinion(&"字".repeat(1000)), Some("u-b"))
        .unwrap();
    let err = a
        .service
        .reply_to_opinion(
            "todo",
            "o1",
            &OpinionRequest {
                text: "回".repeat(301),
            },
            Some("u-b"),
        )
        .unwrap_err();
    assert_eq!(err.code(), "too_long");
    assert!(err.user_message().contains("300"));
    for _ in 0..6 {
        a.service
            .add_opinion("todo", &opinion(&"x".repeat(1000)), Some("u-c"))
            .unwrap();
    }
    let err = a
        .service
        .add_opinion("todo", &opinion(&"x".repeat(1000)), Some("u-c"))
        .unwrap_err();
    assert_eq!(err.code(), "round_full");
    assert!(
        err.user_message().contains("8000"),
        "{}",
        err.user_message()
    );
    // Nothing of a refused opinion was stored.
    assert_eq!(
        stored(&store)["round"]["opinions"]
            .as_array()
            .unwrap()
            .len(),
        8
    );

    // An administrator's lower limits apply to everyone.
    let github_b = github_mock_with(RefUpdate::Ok, todo_app());
    let mut b = M0Service::new(
        settings(1_000_000),
        Box::new(MockProvider::new()),
        committer(&github_b),
    )
    .unwrap();
    b.set_round_limits(RoundLimits {
        opinion_chars: 20,
        ..RoundLimits::default()
    });
    let err = b
        .open_round(
            "todo",
            &OpenRoundRequest {
                opinion: "x".repeat(21),
                ..open(30)
            },
            Some("admin-like-user"),
        )
        .unwrap_err();
    assert_eq!(err.code(), "too_long");
    assert_eq!(b.config_json()["roundLimits"]["opinionChars"], 20);
}

#[test]
fn a_deleted_resource_is_written_again() {
    let github = github_mock_with(RefUpdate::Ok, todo_app());
    let store = Arc::new(MemoryStore::new());
    let a = backend(MockProvider::new(), &github, &store);
    a.service
        .open_round("todo", &open(30), Some("u-a"))
        .unwrap();

    // Deleted by hand: the next change creates it again from memory.
    let name = store.list(ROUNDS).unwrap()[0].name.clone();
    store.remove(ROUNDS, &name);
    a.service
        .add_opinion("todo", &opinion("也要能篩選"), Some("u-b"))
        .unwrap();
    let spec = stored(&store);
    assert_eq!(spec["round"]["opinions"].as_array().unwrap().len(), 2);

    // The CRD gone and reinstalled (every object deleted) while this
    // backend runs: what is in memory stays and is written again; nothing
    // is restarted.
    store.set_installed(false);
    let err = a
        .service
        .add_opinion("todo", &opinion("第三則"), Some("u-c"))
        .unwrap_err();
    assert_eq!(err.code(), "rounds_unavailable");
    store.clear();
    store.set_installed(true);
    a.service.tick(now());
    let spec = stored(&store);
    assert_eq!(spec["round"]["opinions"].as_array().unwrap().len(), 2);
    assert_eq!(round_of(&a)["roundsAvailable"], true);
    a.service
        .add_opinion("todo", &opinion("第三則"), Some("u-c"))
        .unwrap();
    assert_eq!(
        stored(&store)["round"]["opinions"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn a_later_read_does_not_restart_work_in_memory() {
    let github = github_mock_with(RefUpdate::Ok, todo_app());
    let store = Arc::new(MemoryStore::new());
    let a = backend(MockProvider::new(), &github, &store);
    a.service
        .open_round("todo", &open(30), Some("u-a"))
        .unwrap();
    // The stored copy says converging (a stale write); memory says
    // collecting. A later read keeps memory and starts no AI work.
    set_stored_state(&store, "converging");
    store.set_installed(false);
    let _ = a.service.add_opinion("todo", &opinion("x"), Some("u-b"));
    store.set_installed(true);
    a.service.tick(now());
    assert_eq!(state(&round_of(&a)), "collecting");
    assert_eq!(a.provider.call_count(), 0);
}
