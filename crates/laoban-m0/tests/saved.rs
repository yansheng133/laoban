//! Plans waiting for confirmation and the month's token usage kept across
//! backend restarts (v0.1.19): two services on the same store and the same
//! mock GitHub stand for the backend before and after a restart. All data
//! synthetic.

mod common;

use std::sync::Arc;

use common::*;
use laoban_ai::MockProvider;
use laoban_m0::state::{PLANS, TOKEN_USAGES};
use laoban_m0::{ApplyRequest, M0Service, MemoryStore, PlanRequestBody, StateStore};
use laoban_mockhttp::MockServer;
use serde_json::{json, Value};

fn plan_req(prompt: &str) -> PlanRequestBody {
    PlanRequestBody {
        prompt: prompt.into(),
        app_name: None,
        base_app: None,
        background: false,
    }
}

fn apply_req(id: &str) -> ApplyRequest {
    ApplyRequest {
        plan_id: id.into(),
        background: false,
    }
}

fn backend(provider: MockProvider, github: &MockServer, store: &Arc<MemoryStore>) -> M0Service {
    let mut service =
        M0Service::new(settings(1_000_000), Box::new(provider), committer(github)).unwrap();
    service.use_state_store(Arc::clone(store) as Arc<dyn StateStore>);
    assert!(service.restore_plans());
    service
}

fn used(s: &M0Service) -> u64 {
    s.config_json()["tokenBudget"]["used"].as_u64().unwrap()
}

#[test]
fn a_plan_is_confirmed_after_a_restart() {
    let github = github_mock(RefUpdate::Ok);
    let store = Arc::new(MemoryStore::new());

    let before = backend(MockProvider::new().then(ok(&plan_json())), &github, &store);
    let plan = before
        .plan(&plan_req("做一個早餐店點餐頁面"), None)
        .unwrap();
    let id = plan["planId"].as_str().unwrap().to_string();
    let spent = used(&before);
    assert!(spent > 0);
    let saved = store.list(PLANS).unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].name, format!("plan-{id}"));
    assert_eq!(
        saved[0].labels["laoban.yansheng133.github.io/app"],
        "breakfast"
    );
    let usage = store.list(TOKEN_USAGES).unwrap();
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0].spec["tokens"].as_u64(), Some(spent));
    drop(before);

    // The restarted backend has the plan and the month's usage.
    let after = backend(MockProvider::new().then(ok(&files_json())), &github, &store);
    assert_eq!(used(&after), spent);
    let applied = after.apply(&apply_req(&id), None).unwrap();
    assert_eq!(applied["appName"], "breakfast");
    assert!(
        store.list(PLANS).unwrap().is_empty(),
        "applied plans are not kept"
    );
    assert!(used(&after) > spent);
    assert_eq!(
        store.list(TOKEN_USAGES).unwrap()[0].spec["tokens"].as_u64(),
        Some(used(&after))
    );
}

#[test]
fn a_failed_apply_keeps_the_plan_and_an_applying_one_is_not_kept() {
    let github = github_mock(RefUpdate::Ok);
    let store = Arc::new(MemoryStore::new());
    let mut bad = files_value();
    bad["web"]["evil.yaml"] = json!("x");
    let s = backend(
        MockProvider::new()
            .then(ok(&plan_json()))
            .then(ok(&bad.to_string())),
        &github,
        &store,
    );
    let id = s.plan(&plan_req("x 需求"), None).unwrap()["planId"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(s.apply(&apply_req(&id), None).is_err());
    assert_eq!(
        store.list(PLANS).unwrap().len(),
        1,
        "can be confirmed again"
    );
    // A restart now still has it.
    let again = backend(MockProvider::new().then(ok(&files_json())), &github, &store);
    assert!(again.apply(&apply_req(&id), None).is_ok());
}

#[test]
fn expired_and_unreadable_plans_are_deleted() {
    let github = github_mock(RefUpdate::Ok);
    let store = Arc::new(MemoryStore::new());
    let s = backend(MockProvider::new().then(ok(&plan_json())), &github, &store);
    let id = s.plan(&plan_req("x 需求"), None).unwrap()["planId"]
        .as_str()
        .unwrap()
        .to_string();
    let name = format!("plan-{id}");
    store.touch(PLANS, &name, |v: &mut Value| v["created"] = json!(1_000));
    store
        .create(
            PLANS,
            "plan-broken",
            &Default::default(),
            &json!({"schema": 1, "id": "broken"}),
        )
        .unwrap();
    let after = backend(MockProvider::new(), &github, &store);
    assert!(store.list(PLANS).unwrap().is_empty());
    let err = after.apply(&apply_req(&id), None).unwrap_err();
    assert_eq!(err.code(), "plan_not_found");
}

#[test]
fn without_the_crds_plans_and_usage_stay_in_memory() {
    let github = github_mock(RefUpdate::Ok);
    let store = Arc::new(MemoryStore::new());
    store.set_installed(false);
    let s = backend(
        MockProvider::new()
            .then(ok(&plan_json()))
            .then(ok(&files_json())),
        &github,
        &store,
    );
    let id = s.plan(&plan_req("x 需求"), None).unwrap()["planId"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(used(&s) > 0);
    assert!(s.apply(&apply_req(&id), None).is_ok());
}
