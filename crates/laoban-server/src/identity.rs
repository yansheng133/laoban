//! Who is calling: the user the Kubernetes API server verified.
//!
//! The backend is a Kubernetes aggregated API (`APIService`
//! [`GROUP`]/[`VERSION`]). The Rancher UI calls it through Rancher's
//! `/k8s/clusters/<id>` proxy, so the request reaches the downstream API
//! server as the signed-in Rancher user. The API server authenticates and
//! authorizes the user (RBAC on this API group, scoped by namespace and
//! therefore by Rancher Project), then forwards the request over mutual TLS
//! with the user in request headers (`X-Remote-User`, `X-Remote-Group`).
//!
//! The headers are trusted only on a TLS connection whose client
//! certificate chains to the request-header CA and, when the API server
//! lists allowed names, has one of them as its common name. Both come from
//! the `kube-system/extension-apiserver-authentication` ConfigMap. Anyone
//! else (another pod reaching the Service directly) cannot present such a
//! certificate, so cannot claim to be someone.
//!
//! Docs: https://kubernetes.io/docs/tasks/extend-kubernetes/configure-aggregation-layer/

use serde_json::Value;

/// API group of the aggregated API. A subdomain of the CRD group
/// (`laoban.yansheng133.github.io`) so the two never collide.
pub const GROUP: &str = "api.laoban.yansheng133.github.io";
pub const VERSION: &str = "v1alpha1";

/// Longest user or group name accepted from the headers.
const MAX_NAME_BYTES: usize = 256;
/// At most this many groups are kept.
const MAX_GROUPS: usize = 64;

/// What the API server told us about the request-header proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthConfig {
    /// PEM of the CA that signs the API server's proxy client certificate.
    pub client_ca_pem: String,
    /// Allowed common names of that certificate; empty means any.
    pub allowed_names: Vec<String>,
    /// Header names carrying the user, lower case, in order of preference.
    pub username_headers: Vec<String>,
    /// Header names carrying groups, lower case.
    pub group_headers: Vec<String>,
}

fn json_list(data: &Value, key: &str) -> Result<Vec<String>, String> {
    match data.get(key).and_then(Value::as_str) {
        None | Some("") => Ok(Vec::new()),
        Some(s) => serde_json::from_str::<Vec<String>>(s)
            .map_err(|_| format!("{key} is not a JSON list of strings")),
    }
}

impl AuthConfig {
    /// From the ConfigMap's `data`.
    pub fn from_configmap_data(data: &Value) -> Result<Self, String> {
        let client_ca_pem = data
            .get("requestheader-client-ca-file")
            .and_then(Value::as_str)
            .filter(|s| s.contains("BEGIN CERTIFICATE"))
            .ok_or("requestheader-client-ca-file is missing")?
            .to_string();
        let lower = |v: Vec<String>| -> Vec<String> {
            v.into_iter()
                .map(|h| h.trim().to_ascii_lowercase())
                .filter(|h| !h.is_empty())
                .collect()
        };
        let username_headers = lower(json_list(data, "requestheader-username-headers")?);
        if username_headers.is_empty() {
            return Err("requestheader-username-headers is empty".into());
        }
        Ok(Self {
            client_ca_pem,
            allowed_names: json_list(data, "requestheader-allowed-names")?,
            username_headers,
            group_headers: lower(json_list(data, "requestheader-group-headers")?),
        })
    }

    /// Is a proxy client certificate with this common name allowed?
    pub fn name_allowed(&self, common_name: Option<&str>) -> bool {
        self.allowed_names.is_empty()
            || common_name.is_some_and(|cn| self.allowed_names.iter().any(|n| n == cn))
    }

    /// The caller from the request headers (`(lower-case name, value)`).
    /// `None` when no user header is present (for example the API
    /// server's own availability check).
    pub fn caller(&self, headers: &[(String, String)]) -> Option<Caller> {
        let user = self.username_headers.iter().find_map(|name| {
            headers
                .iter()
                .find(|(n, v)| n == name && !v.trim().is_empty())
                .map(|(_, v)| v.trim().to_string())
        })?;
        if user.len() > MAX_NAME_BYTES || user.chars().any(char::is_control) {
            return None;
        }
        let groups = headers
            .iter()
            .filter(|(n, _)| self.group_headers.contains(n))
            .map(|(_, v)| v.trim().to_string())
            .filter(|g| {
                !g.is_empty() && g.len() <= MAX_NAME_BYTES && !g.chars().any(char::is_control)
            })
            .take(MAX_GROUPS)
            .collect();
        Some(Caller { user, groups })
    }
}

/// A verified caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// Kubernetes user name; for Rancher users the Rancher user ID
    /// (`u-…`, `user-…`).
    pub user: String,
    pub groups: Vec<String>,
}

/// `GET /apis/<group>/<version>`: discovery, also used by the API server
/// to decide the APIService is available. Resources mirror the paths in
/// docs/m0-design.md, so RBAC rules can name them.
pub fn discovery_json() -> String {
    let r = |name: &str, verbs: &[&str]| {
        serde_json::json!({
            "name": name,
            "singularName": "",
            "namespaced": true,
            "kind": "LaobanApi",
            "verbs": verbs,
        })
    };
    serde_json::json!({
        "kind": "APIResourceList",
        "apiVersion": "v1",
        "groupVersion": format!("{GROUP}/{VERSION}"),
        "resources": [
            r("config", &["list"]),
            r("apps", &["get", "list"]),
            r("apps/versions", &["get"]),
            r("apps/files", &["get", "create"]),
            r("apps/layout", &["create"]),
            r("apps/restore", &["create"]),
            r("apps/round", &["get", "create"]),
            r("apps/rounds", &["get"]),
            r("waitingrounds", &["list"]),
            r("plans", &["get", "create"]),
            r("plans/apply", &["create"]),
        ],
    })
    .to_string()
}

/// `GET /apis/<group>`.
pub fn group_json() -> String {
    let gv =
        serde_json::json!({ "groupVersion": format!("{GROUP}/{VERSION}"), "version": VERSION });
    serde_json::json!({
        "kind": "APIGroup",
        "apiVersion": "v1",
        "name": GROUP,
        "versions": [gv],
        "preferredVersion": gv,
    })
    .to_string()
}

/// Map an aggregated API path to the internal `/api/...` path, given the
/// namespace the backend serves. `None` if it is not under that namespace.
///
/// `/apis/<G>/<V>/namespaces/<ns>/config` -> `/api/config`,
/// `.../apps[/...]` -> `/api/apps[/...]`, `.../plans` -> `/api/plan`,
/// `.../plans/<id>/apply` -> `/api/plans/<id>/apply`.
pub fn internal_path(path: &str, namespace: &str) -> Option<String> {
    let prefix = format!("/apis/{GROUP}/{VERSION}/namespaces/{namespace}/");
    let rest = path.strip_prefix(&prefix)?;
    if rest == "config" {
        return Some("/api/config".into());
    }
    if rest == "plans" {
        return Some("/api/plan".into());
    }
    if rest == "waitingrounds" {
        return Some("/api/rounds/mine".into());
    }
    if rest == "apps" || rest.starts_with("apps/") || rest.starts_with("plans/") {
        return Some(format!("/api/{rest}"));
    }
    None
}

/// A path of this API under a namespace other than the one served.
pub fn other_namespace(path: &str, namespace: &str) -> bool {
    let prefix = format!("/apis/{GROUP}/{VERSION}/namespaces/");
    match path.strip_prefix(&prefix) {
        Some(rest) => {
            let ns = rest.split('/').next().unwrap_or("");
            !ns.is_empty() && ns != namespace
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cm() -> Value {
        json!({
            "requestheader-client-ca-file": "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n",
            "requestheader-allowed-names": "[\"system:auth-proxy\"]",
            "requestheader-username-headers": "[\"X-Remote-User\"]",
            "requestheader-group-headers": "[\"X-Remote-Group\"]",
            "requestheader-extra-headers-prefix": "[\"X-Remote-Extra-\"]",
        })
    }

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn reads_the_configmap() {
        let c = AuthConfig::from_configmap_data(&cm()).unwrap();
        assert_eq!(c.allowed_names, ["system:auth-proxy"]);
        assert_eq!(c.username_headers, ["x-remote-user"]);
        assert_eq!(c.group_headers, ["x-remote-group"]);
        assert!(c.name_allowed(Some("system:auth-proxy")));
        assert!(!c.name_allowed(Some("someone")));
        assert!(!c.name_allowed(None));

        let mut no_ca = cm();
        no_ca["requestheader-client-ca-file"] = json!("");
        assert!(AuthConfig::from_configmap_data(&no_ca).is_err());
        let mut bad = cm();
        bad["requestheader-allowed-names"] = json!("system:auth-proxy");
        assert!(AuthConfig::from_configmap_data(&bad).is_err());
        let mut any = cm();
        any["requestheader-allowed-names"] = json!("");
        assert!(AuthConfig::from_configmap_data(&any)
            .unwrap()
            .name_allowed(Some("x")));
    }

    #[test]
    fn caller_from_headers() {
        let c = AuthConfig::from_configmap_data(&cm()).unwrap();
        let caller = c
            .caller(&h(&[
                ("x-remote-user", "u-abc12"),
                ("x-remote-group", "system:authenticated"),
                ("x-remote-group", "u-abc12-group"),
                ("x-other", "x"),
            ]))
            .unwrap();
        assert_eq!(caller.user, "u-abc12");
        assert_eq!(caller.groups, ["system:authenticated", "u-abc12-group"]);
        assert_eq!(c.caller(&h(&[("x-remote-group", "g")])), None);
        assert_eq!(c.caller(&h(&[("x-remote-user", "  ")])), None);
        assert_eq!(c.caller(&h(&[("x-remote-user", &"a".repeat(300))])), None);
    }

    #[test]
    fn paths() {
        let p = |s: &str| {
            internal_path(
                &format!("/apis/{GROUP}/{VERSION}/namespaces/laoban-dev/{s}"),
                "laoban-dev",
            )
        };
        assert_eq!(p("config").as_deref(), Some("/api/config"));
        assert_eq!(p("apps").as_deref(), Some("/api/apps"));
        assert_eq!(
            p("apps/todo/versions/abc").as_deref(),
            Some("/api/apps/todo/versions/abc")
        );
        assert_eq!(p("plans").as_deref(), Some("/api/plan"));
        assert_eq!(
            p("plans/p-1/apply").as_deref(),
            Some("/api/plans/p-1/apply")
        );
        assert_eq!(p("waitingrounds").as_deref(), Some("/api/rounds/mine"));
        assert_eq!(p("rounds/mine"), None);
        assert_eq!(p("rounds/other"), None);
        assert_eq!(p("other"), None);
        assert_eq!(
            internal_path(
                &format!("/apis/{GROUP}/{VERSION}/namespaces/other/apps"),
                "laoban-dev"
            ),
            None
        );
        let d: Value = serde_json::from_str(&discovery_json()).unwrap();
        assert_eq!(d["groupVersion"], format!("{GROUP}/{VERSION}"));
    }
}
