//! The one Kubernetes API call the backend makes with its own identity:
//! reading `kube-system/extension-apiserver-authentication` at startup
//! (RBAC: the built-in Role `extension-apiserver-authentication-reader`).

use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, RootCertStore};

use crate::identity::AuthConfig;

const SA_DIR: &str = "/var/run/secrets/kubernetes.io/serviceaccount";
const CONFIGMAP_PATH: &str =
    "/api/v1/namespaces/kube-system/configmaps/extension-apiserver-authentication";

/// Read the request-header settings from the cluster the pod runs in.
pub fn load_auth_config() -> Result<AuthConfig, String> {
    let host = std::env::var("KUBERNETES_SERVICE_HOST")
        .map_err(|_| "not running in Kubernetes (KUBERNETES_SERVICE_HOST unset)".to_string())?;
    let port = std::env::var("KUBERNETES_SERVICE_PORT").unwrap_or_else(|_| "443".into());
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    let token = std::fs::read_to_string(format!("{SA_DIR}/token"))
        .map_err(|_| "no ServiceAccount token in the pod".to_string())?;
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
    let resp = agent
        .get(&format!("https://{host}:{port}{CONFIGMAP_PATH}"))
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
