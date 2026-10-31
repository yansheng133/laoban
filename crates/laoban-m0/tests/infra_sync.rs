//! Startup sync of the app Ingress / NetworkPolicy files with the settings.

mod common;

use common::*;
use laoban_ai::MockProvider;
use laoban_m0::config::AppUrl;
use laoban_m0::render::infra_files;
use laoban_m0::service::INFRA_SYNC_MESSAGE;
use laoban_m0::M0Service;
use laoban_mockhttp::MockServer;
use serde_json::Value;

fn app_url(host: &str) -> AppUrl {
    AppUrl {
        scheme: "https".into(),
        host: host.into(),
        port: String::new(),
        ingress_class: None,
        tls_secret: None,
        annotations: Default::default(),
    }
}

fn service(
    url: Option<AppUrl>,
    network_policy: bool,
    files: Vec<(String, String)>,
) -> (M0Service, MockServer) {
    let mut s = settings(1_000_000);
    s.app_url = url;
    s.app_network_policy = network_policy;
    let github = github_mock_files(
        RefUpdate::Ok,
        vec![("todo".into(), Some("{}".into())), ("legacy".into(), None)],
        Vec::new(),
        files,
    );
    let service = M0Service::new(s, Box::new(MockProvider::new()), committer(&github)).unwrap();
    (service, github)
}

/// (path, sha or null) of the committed tree, and the commit message.
fn committed(github: &MockServer) -> (Vec<(String, Option<String>)>, String) {
    let reqs = github.requests();
    let body = |suffix: &str| -> Value {
        let r = reqs
            .iter()
            .rev()
            .find(|r| r.method == "POST" && r.path.ends_with(suffix))
            .unwrap();
        serde_json::from_str(&r.body_str()).unwrap()
    };
    let tree = body("/git/trees")["tree"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["path"].as_str().unwrap().to_string(),
                e["sha"].as_str().map(str::to_string),
            )
        })
        .collect();
    (
        tree,
        body("/git/commits")["message"]
            .as_str()
            .unwrap()
            .to_string(),
    )
}

#[test]
fn writes_missing_files_for_every_app() {
    let (svc, github) = service(Some(app_url("{app}.apps.example.com")), true, Vec::new());
    assert_eq!(svc.sync_infra().unwrap(), 4);
    let (tree, message) = committed(&github);
    let paths: Vec<&str> = tree.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(
        paths,
        [
            "apps/legacy/ingress.yaml",
            "apps/legacy/networkpolicy.yaml",
            "apps/todo/ingress.yaml",
            "apps/todo/networkpolicy.yaml",
        ]
    );
    assert!(tree.iter().all(|(_, sha)| sha.is_some()));
    assert_eq!(message, INFRA_SYNC_MESSAGE);
    let blobs = blob_contents(&github);
    assert!(blobs[0].contains("legacy.apps.example.com"), "{}", blobs[0]);
}

#[test]
fn nothing_to_do_when_files_match() {
    let url = app_url("{app}.apps.example.com");
    let files: Vec<(String, String)> = ["todo", "legacy"]
        .iter()
        .flat_map(|app| {
            infra_files(app, Some(&url), false)
                .into_iter()
                .filter_map(move |(n, c)| c.map(|c| (format!("apps/{app}/{n}"), c)))
        })
        .collect();
    let (svc, github) = service(Some(url), false, files);
    assert_eq!(svc.sync_infra().unwrap(), 0);
    assert_eq!(writes(&github), 0);
}

#[test]
fn rewrites_a_changed_host_and_deletes_unwanted_files() {
    let old = app_url("{app}.old.example.com");
    let mut files: Vec<(String, String)> = Vec::new();
    for (n, c) in infra_files("todo", Some(&old), true) {
        files.push((format!("apps/todo/{n}"), c.unwrap()));
    }
    // apps.url changed, NetworkPolicies turned off.
    let (svc, github) = service(Some(app_url("{app}.new.example.com")), false, files);
    assert_eq!(svc.sync_infra().unwrap(), 3);
    let (tree, _) = committed(&github);
    assert!(
        tree.contains(&("apps/todo/networkpolicy.yaml".into(), None)),
        "{tree:?}"
    );
    assert!(tree
        .iter()
        .any(|(p, s)| p == "apps/todo/ingress.yaml" && s.is_some()));
    assert!(tree
        .iter()
        .any(|(p, s)| p == "apps/legacy/ingress.yaml" && s.is_some()));
    assert!(blob_contents(&github)
        .iter()
        .any(|b| b.contains("todo.new.example.com")));

    // apps.url removed: the Ingress goes too.
    let mut files: Vec<(String, String)> = Vec::new();
    for (n, c) in infra_files("todo", Some(&old), false) {
        if let Some(c) = c {
            files.push((format!("apps/todo/{n}"), c));
        }
    }
    let (svc, github) = service(None, false, files);
    assert_eq!(svc.sync_infra().unwrap(), 1);
    assert_eq!(
        committed(&github).0,
        vec![("apps/todo/ingress.yaml".to_string(), None)]
    );
}
