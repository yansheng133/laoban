//! The Kubernetes API calls the backend makes with its own identity:
//!
//! * reading `kube-system/extension-apiserver-authentication` at startup
//!   (RBAC: the built-in Role `extension-apiserver-authentication-reader`);
//! * reading and writing laoban's own resources (rounds) in its namespace,
//!   [`KubeStore`] (RBAC: the chart's Role, those resources only).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use laoban_m0::state::{Kind, Object, StateError, StateStore, API_GROUP, API_VERSION};
use serde_json::{json, Value};

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, RootCertStore};

use crate::identity::AuthConfig;

const SA_DIR: &str = "/var/run/secrets/kubernetes.io/serviceaccount";
const CONFIGMAP_PATH: &str =
    "/api/v1/namespaces/kube-system/configmaps/extension-apiserver-authentication";

/// `https://<host>:<port>` of the API server and an agent trusting the
/// cluster CA, from inside the pod.
fn in_cluster_agent() -> Result<(String, ureq::Agent), String> {
    let host = std::env::var("KUBERNETES_SERVICE_HOST")
        .map_err(|_| "not running in Kubernetes (KUBERNETES_SERVICE_HOST unset)".to_string())?;
    let port = std::env::var("KUBERNETES_SERVICE_PORT").unwrap_or_else(|_| "443".into());
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    let ca = std::fs::read(format!("{SA_DIR}/ca.crt"))
        .map_err(|_| "no cluster CA in the pod".to_string())?;

    let mut roots = RootCertStore::empty();
    for c in CertificateDer::pem_slice_iter(&ca) {
        roots
            .add(c.map_err(|_| "cluster CA is not valid PEM".to_string())?)
            .map_err(|_| "cluster CA is not a valid certificate".to_string())?;
    }
    let tls =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
    let agent = ureq::AgentBuilder::new()
        .tls_config(Arc::new(tls))
        .timeout(Duration::from_secs(10))
        .build();
    Ok((format!("https://{host}:{port}"), agent))
}

/// Read the request-header settings from the cluster the pod runs in.
pub fn load_auth_config() -> Result<AuthConfig, String> {
    let (base, agent) = in_cluster_agent()?;
    let token = std::fs::read_to_string(format!("{SA_DIR}/token"))
        .map_err(|_| "no ServiceAccount token in the pod".to_string())?;
    let resp = agent
        .get(&format!("{base}{CONFIGMAP_PATH}"))
        .set("Authorization", &format!("Bearer {}", token.trim()))
        .set("Accept", "application/json")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(code, _) => format!("reading the ConfigMap returned HTTP {code}"),
            ureq::Error::Transport(_) => "cannot reach the Kubernetes API".to_string(),
        })?;
    let body = resp
        .into_string()
        .map_err(|_| "cannot read the ConfigMap".to_string())?;
    let v: serde_json::Value =
        serde_json::from_str(&body).map_err(|_| "the ConfigMap is not JSON".to_string())?;
    AuthConfig::from_configmap_data(&v["data"])
}

/// Longest response read from the API server.
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

/// laoban's own resources in one namespace, through the API server with
/// the pod's ServiceAccount. The token is read for every call (it is
/// rotated by the kubelet).
pub struct KubeStore {
    base: String,
    agent: ureq::Agent,
    token_file: PathBuf,
    namespace: String,
}

impl KubeStore {
    /// In the pod: the namespace is `LAOBAN_STATE_NAMESPACE`, else the
    /// pod's own (normally `laoban-system`).
    pub fn in_cluster() -> Result<Self, String> {
        let (base, agent) = in_cluster_agent()?;
        let namespace = match std::env::var("LAOBAN_STATE_NAMESPACE") {
            Ok(ns) if !ns.trim().is_empty() => ns.trim().to_string(),
            _ => std::fs::read_to_string(format!("{SA_DIR}/namespace"))
                .map(|s| s.trim().to_string())
                .map_err(|_| "cannot tell the pod's namespace".to_string())?,
        };
        if !valid_namespace(&namespace) {
            return Err("LAOBAN_STATE_NAMESPACE is not a valid namespace".into());
        }
        Ok(Self {
            base,
            agent,
            token_file: PathBuf::from(format!("{SA_DIR}/token")),
            namespace,
        })
    }

    /// Against `base` (for example a test server), with the token in
    /// `token_file`.
    pub fn with_base(base: &str, token_file: PathBuf, namespace: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(10))
                .build(),
            token_file,
            namespace: namespace.to_string(),
        }
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    fn url(&self, kind: Kind, name: Option<&str>) -> String {
        let mut u = format!(
            "{}/apis/{API_GROUP}/{API_VERSION}/namespaces/{}/{}",
            self.base, self.namespace, kind.plural
        );
        if let Some(n) = name {
            u.push('/');
            u.push_str(n);
        }
        u
    }

    /// One call. `Ok` carries the JSON body of a 2xx answer; `Err` the
    /// status and the body of a refused one.
    fn call(&self, method: &str, url: &str, body: Option<&Value>) -> Result<Value, Refused> {
        let token = std::fs::read_to_string(&self.token_file)
            .map_err(|_| Refused::Other("no ServiceAccount token in the pod".into()))?;
        let req = self
            .agent
            .request(method, url)
            .set("Authorization", &format!("Bearer {}", token.trim()))
            .set("Accept", "application/json");
        let result = match body {
            Some(b) => req
                .set("Content-Type", "application/json")
                .send_string(&b.to_string()),
            None => req.call(),
        };
        let read = |resp: ureq::Response| -> Value {
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(
                &mut std::io::Read::take(resp.into_reader(), MAX_RESPONSE_BYTES),
                &mut text,
            );
            serde_json::from_str(&text).unwrap_or(Value::Null)
        };
        match result {
            Ok(resp) => Ok(read(resp)),
            Err(ureq::Error::Status(code, resp)) => Err(Refused::Status(code, read(resp))),
            Err(ureq::Error::Transport(_)) => {
                Err(Refused::Other("cannot reach the Kubernetes API".into()))
            }
        }
    }
}

enum Refused {
    Status(u16, Value),
    Other(String),
}

/// A 404 about the named object (it does not exist) rather than about the
/// resource type (the CRD is not installed).
fn object_missing(body: &Value, name: &str) -> bool {
    body["details"]["name"].as_str() == Some(name)
}

fn refused(kind: Kind, r: Refused, name: Option<&str>) -> StateError {
    match r {
        Refused::Other(m) => StateError::Other(m),
        Refused::Status(404, body) if name.map_or(true, |n| !object_missing(&body, n)) => {
            StateError::NotInstalled(kind.plural)
        }
        Refused::Status(404, _) => StateError::NotFound,
        Refused::Status(409, body) if body["reason"] == "AlreadyExists" => StateError::Exists,
        Refused::Status(409, _) => StateError::Conflict,
        Refused::Status(413, _) => StateError::TooLarge,
        Refused::Status(422, body)
            if body["message"].as_str().is_some_and(|m| {
                m.to_ascii_lowercase().contains("too long")
                    || m.to_ascii_lowercase().contains("too large")
            }) =>
        {
            StateError::TooLarge
        }
        Refused::Status(403, _) => {
            StateError::Other("forbidden: the backend's Role does not allow this".into())
        }
        Refused::Status(code, _) => StateError::Other(format!("HTTP {code}")),
    }
}

fn to_object(v: &Value) -> Option<Object> {
    let m = &v["metadata"];
    Some(Object {
        name: m["name"].as_str()?.to_string(),
        version: m["resourceVersion"].as_str()?.to_string(),
        labels: m["labels"]
            .as_object()
            .map(|l| {
                l.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default(),
        spec: v["spec"].clone(),
    })
}

fn version_of(v: &Value) -> Result<String, StateError> {
    v["metadata"]["resourceVersion"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| StateError::Other("no resourceVersion in the answer".into()))
}

impl StateStore for KubeStore {
    fn list(&self, kind: Kind) -> Result<Vec<Object>, StateError> {
        let v = self
            .call("GET", &self.url(kind, None), None)
            .map_err(|r| refused(kind, r, None))?;
        Ok(v["items"]
            .as_array()
            .map(|items| items.iter().filter_map(to_object).collect())
            .unwrap_or_default())
    }

    fn get(&self, kind: Kind, name: &str) -> Result<Option<Object>, StateError> {
        match self.call("GET", &self.url(kind, Some(name)), None) {
            Ok(v) => Ok(to_object(&v)),
            Err(r) => match refused(kind, r, Some(name)) {
                StateError::NotFound => Ok(None),
                e => Err(e),
            },
        }
    }

    fn create(
        &self,
        kind: Kind,
        name: &str,
        labels: &BTreeMap<String, String>,
        spec: &Value,
    ) -> Result<String, StateError> {
        laoban_m0::state::check_size(spec)?;
        let body = json!({
            "apiVersion": format!("{API_GROUP}/{API_VERSION}"),
            "kind": kind.kind,
            "metadata": { "name": name, "namespace": self.namespace, "labels": labels },
            "spec": spec,
        });
        let v = self
            .call("POST", &self.url(kind, None), Some(&body))
            .map_err(|r| refused(kind, r, Some(name)))?;
        version_of(&v)
    }

    fn update(
        &self,
        kind: Kind,
        name: &str,
        version: &str,
        spec: &Value,
    ) -> Result<String, StateError> {
        laoban_m0::state::check_size(spec)?;
        let url = self.url(kind, Some(name));
        // Keep the labels; replace the spec only if still at `version`.
        let current = match self.call("GET", &url, None) {
            Ok(v) => v,
            Err(r) => {
                return Err(match refused(kind, r, Some(name)) {
                    StateError::NotFound => StateError::Conflict,
                    e => e,
                })
            }
        };
        let mut body = current;
        body["metadata"]["resourceVersion"] = json!(version);
        body["spec"] = spec.clone();
        if let Some(m) = body["metadata"].as_object_mut() {
            m.remove("managedFields");
        }
        let v = self.call("PUT", &url, Some(&body)).map_err(|r| {
            match refused(kind, r, Some(name)) {
                StateError::NotFound => StateError::Conflict,
                e => e,
            }
        })?;
        version_of(&v)
    }

    fn delete(&self, kind: Kind, name: &str) -> Result<(), StateError> {
        match self.call("DELETE", &self.url(kind, Some(name)), None) {
            Ok(_) => Ok(()),
            Err(r) => match refused(kind, r, Some(name)) {
                StateError::NotFound => Ok(()),
                e => Err(e),
            },
        }
    }
}

/// Used when the pod cannot reach its own resources at all: rounds then
/// stay unavailable (never kept in memory only, where a restart loses them).
pub struct UnavailableStore(pub String);

impl StateStore for UnavailableStore {
    fn list(&self, _: Kind) -> Result<Vec<Object>, StateError> {
        Err(StateError::Other(self.0.clone()))
    }
    fn get(&self, _: Kind, _: &str) -> Result<Option<Object>, StateError> {
        Err(StateError::Other(self.0.clone()))
    }
    fn create(
        &self,
        _: Kind,
        _: &str,
        _: &BTreeMap<String, String>,
        _: &Value,
    ) -> Result<String, StateError> {
        Err(StateError::Other(self.0.clone()))
    }
    fn update(&self, _: Kind, _: &str, _: &str, _: &Value) -> Result<String, StateError> {
        Err(StateError::Other(self.0.clone()))
    }
    fn delete(&self, _: Kind, _: &str) -> Result<(), StateError> {
        Err(StateError::Other(self.0.clone()))
    }
}

fn valid_namespace(ns: &str) -> bool {
    !ns.is_empty()
        && ns.len() <= 63
        && ns
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !ns.starts_with('-')
        && !ns.ends_with('-')
}
