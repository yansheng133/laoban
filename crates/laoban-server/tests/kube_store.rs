//! `KubeStore` against a fake Kubernetes API server that keeps objects
//! and checks `resourceVersion` like the real one. Synthetic data only.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use laoban_m0::state::{StateError, StateStore, ROUNDS};
use laoban_mockhttp::{MockServer, Recorded, Reply};
use laoban_server::kube::KubeStore;
use serde_json::{json, Value};

const BASE: &str = "/apis/laoban.yansheng133.github.io/v1alpha1/namespaces/laoban-system/rounds";
const TOKEN: &str = "test-sa-token-0001";

struct Fake {
    server: MockServer,
    installed: Arc<AtomicBool>,
}

fn not_found(name: Option<&str>) -> Reply {
    let details = match name {
        Some(n) => json!({"name": n, "group": "laoban.yansheng133.github.io", "kind": "rounds"}),
        None => json!({}),
    };
    Reply::json(
        404,
        json!({"kind": "Status", "status": "Failure", "reason": "NotFound", "code": 404, "details": details})
            .to_string(),
    )
}

fn fake() -> Fake {
    let objects: Arc<Mutex<BTreeMap<String, Value>>> = Arc::default();
    let counter = Arc::new(AtomicU64::new(100));
    let installed = Arc::new(AtomicBool::new(true));
    let inst = Arc::clone(&installed);
    let server = MockServer::start(move |r: &Recorded| {
        assert_eq!(
            r.header("authorization"),
            Some(format!("Bearer {TOKEN}").as_str())
        );
        if !inst.load(Ordering::SeqCst) {
            return not_found(None);
        }
        let Some(rest) = r.path.strip_prefix(BASE) else {
            return not_found(None);
        };
        let name = rest.strip_prefix('/').filter(|n| !n.is_empty());
        let mut objects = objects.lock().unwrap();
        let next = || (counter.fetch_add(1, Ordering::SeqCst) + 1).to_string();
        match (r.method.as_str(), name) {
            ("GET", None) => Reply::json(
                200,
                json!({"kind": "RoundList", "items": objects.values().cloned().collect::<Vec<_>>()})
                    .to_string(),
            ),
            ("GET", Some(n)) => match objects.get(n) {
                Some(o) => Reply::json(200, o.to_string()),
                None => not_found(Some(n)),
            },
            ("POST", None) => {
                assert_eq!(r.header("content-type"), Some("application/json"));
                let mut o: Value = serde_json::from_str(&r.body_str()).unwrap();
                assert_eq!(o["apiVersion"], "laoban.yansheng133.github.io/v1alpha1");
                assert_eq!(o["kind"], "Round");
                assert_eq!(o["metadata"]["namespace"], "laoban-system");
                let n = o["metadata"]["name"].as_str().unwrap().to_string();
                if objects.contains_key(&n) {
                    return Reply::json(409, json!({"reason": "AlreadyExists", "code": 409}).to_string());
                }
                if o.to_string().len() > 1_500_000 {
                    return Reply::json(413, "{}");
                }
                o["metadata"]["resourceVersion"] = json!(next());
                objects.insert(n, o.clone());
                Reply::json(201, o.to_string())
            }
            ("PUT", Some(n)) => {
                let mut o: Value = serde_json::from_str(&r.body_str()).unwrap();
                let Some(cur) = objects.get(n) else {
                    return not_found(Some(n));
                };
                if cur["metadata"]["resourceVersion"] != o["metadata"]["resourceVersion"] {
                    return Reply::json(409, json!({"reason": "Conflict", "code": 409}).to_string());
                }
                o["metadata"]["resourceVersion"] = json!(next());
                objects.insert(n.to_string(), o.clone());
                Reply::json(200, o.to_string())
            }
            ("DELETE", Some(n)) => match objects.remove(n) {
                Some(_) => Reply::json(200, json!({"status": "Success"}).to_string()),
                None => not_found(Some(n)),
            },
            _ => Reply::json(405, "{}"),
        }
    });
    Fake { server, installed }
}

fn store(f: &Fake) -> (KubeStore, tempdir::Dir) {
    let dir = tempdir::Dir::new();
    let token = dir.path.join("token");
    std::fs::write(&token, format!("{TOKEN}\n")).unwrap();
    (
        KubeStore::with_base(&f.server.url(), token, "laoban-system"),
        dir,
    )
}

/// A throwaway directory for the token file.
mod tempdir {
    pub struct Dir {
        pub path: std::path::PathBuf,
    }
    impl Dir {
        pub fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "laoban-kube-store-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

#[test]
fn create_read_update_delete_with_versions() {
    let f = fake();
    let (s, _dir) = store(&f);
    let labels = BTreeMap::from([(
        "laoban.yansheng133.github.io/app".to_string(),
        "todo".to_string(),
    )]);
    let v1 = s
        .create(ROUNDS, "todo-r-1", &labels, &json!({"state": "collecting"}))
        .unwrap();
    assert_eq!(
        s.create(ROUNDS, "todo-r-1", &labels, &json!({})),
        Err(StateError::Exists)
    );
    let o = s.get(ROUNDS, "todo-r-1").unwrap().unwrap();
    assert_eq!(
        (o.version.as_str(), o.spec["state"].as_str()),
        (v1.as_str(), Some("collecting"))
    );
    assert_eq!(o.labels["laoban.yansheng133.github.io/app"], "todo");

    let v2 = s
        .update(ROUNDS, "todo-r-1", &v1, &json!({"state": "converging"}))
        .unwrap();
    assert_ne!(v1, v2);
    assert_eq!(
        s.update(ROUNDS, "todo-r-1", &v1, &json!({"state": "x"})),
        Err(StateError::Conflict),
        "a stale version never overwrites"
    );
    let o = s.get(ROUNDS, "todo-r-1").unwrap().unwrap();
    assert_eq!(o.spec["state"], "converging");
    assert_eq!(o.labels.len(), 1, "labels kept on update");

    assert_eq!(s.list(ROUNDS).unwrap().len(), 1);
    s.delete(ROUNDS, "todo-r-1").unwrap();
    s.delete(ROUNDS, "todo-r-1").unwrap();
    assert_eq!(s.get(ROUNDS, "todo-r-1").unwrap(), None);
    assert_eq!(
        s.update(ROUNDS, "todo-r-1", &v2, &json!({})),
        Err(StateError::Conflict),
        "deleted meanwhile"
    );
    // The token is sent on every call (checked by the fake) and nowhere else.
    for r in f.server.requests() {
        assert!(!r.path.contains(TOKEN) && !r.body_str().contains(TOKEN));
    }
}

#[test]
fn missing_crd_and_too_large() {
    let f = fake();
    let (s, _dir) = store(&f);
    f.installed.store(false, Ordering::SeqCst);
    assert_eq!(s.list(ROUNDS), Err(StateError::NotInstalled("rounds")));
    assert_eq!(s.get(ROUNDS, "x"), Err(StateError::NotInstalled("rounds")));
    f.installed.store(true, Ordering::SeqCst);
    let big = json!({"x": "a".repeat(600 * 1024)});
    assert_eq!(
        s.create(ROUNDS, "big", &BTreeMap::new(), &big),
        Err(StateError::TooLarge),
        "refused before it is sent"
    );
    assert!(f.server.requests().iter().all(|r| r.method != "POST"));
}

#[test]
fn unreachable_api_server() {
    let (s, _dir) = {
        let f = fake();
        let url = f.server.url();
        drop(f);
        let dir = tempdir::Dir::new();
        let token = dir.path.join("token");
        std::fs::write(&token, TOKEN).unwrap();
        (KubeStore::with_base(&url, token, "laoban-system"), dir)
    };
    assert!(matches!(s.list(ROUNDS), Err(StateError::Other(_))));
}
