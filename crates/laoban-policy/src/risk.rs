//! Risk assessment from rendered Kubernetes manifests.
//!
//! Callers pass the desired-state manifests before and after the change,
//! already parsed into JSON values (laoban renders YAML elsewhere). Objects
//! are matched by kind, namespace and name.
//!
//! "Exposes externally" means any of:
//!
//! * a new `Ingress`, or a new Gateway API route (`HTTPRoute`, `GRPCRoute`,
//!   `TLSRoute`, `TCPRoute`, `UDPRoute`) or `Gateway`;
//! * a `Service` that becomes `LoadBalancer` or `NodePort`;
//! * a `NetworkPolicy` that is removed, or that starts allowing all peers in
//!   a direction it did not allow all before.
//!
//! Database schema changes cannot be seen in manifests; the build step
//! reports them (for example when migration files changed).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "change")]
pub enum ExposureChange {
    NewIngress {
        namespace: String,
        name: String,
    },
    NewGatewayResource {
        kind: String,
        namespace: String,
        name: String,
    },
    ServiceExposed {
        namespace: String,
        name: String,
        service_type: String,
    },
    NetworkPolicyRemoved {
        namespace: String,
        name: String,
    },
    NetworkPolicyAllowsAll {
        namespace: String,
        name: String,
        direction: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Risk {
    pub exposure: Vec<ExposureChange>,
    pub database_schema_change: bool,
    pub permission_change: bool,
}

impl Risk {
    /// A change that touches nothing risky, e.g. text-only edits.
    pub fn none() -> Self {
        Self::default()
    }

    pub fn from_manifests(before: &[Value], after: &[Value], database_schema_change: bool) -> Self {
        let before = index(before);
        let after = index(after);
        Self {
            exposure: exposure_changes(&before, &after),
            database_schema_change,
            permission_change: permission_changed(&before, &after),
        }
    }
}

type Key = (String, String, String);

const GATEWAY_KINDS: &[&str] = &[
    "Gateway",
    "HTTPRoute",
    "GRPCRoute",
    "TLSRoute",
    "TCPRoute",
    "UDPRoute",
];

const PERMISSION_KINDS: &[&str] = &[
    "Role",
    "ClusterRole",
    "RoleBinding",
    "ClusterRoleBinding",
    "ServiceAccount",
];

const CLUSTER_SCOPED: &[&str] = &["ClusterRole", "ClusterRoleBinding", "Namespace"];

fn key(obj: &Value) -> Option<Key> {
    let kind = obj.get("kind")?.as_str()?.to_string();
    let meta = obj.get("metadata")?;
    let name = meta.get("name")?.as_str()?.to_string();
    let namespace = if CLUSTER_SCOPED.contains(&kind.as_str()) {
        String::new()
    } else {
        meta.get("namespace")
            .and_then(Value::as_str)
            .unwrap_or("default")
            .to_string()
    };
    Some((kind, namespace, name))
}

fn index(objs: &[Value]) -> BTreeMap<Key, &Value> {
    objs.iter().filter_map(|o| key(o).map(|k| (k, o))).collect()
}

fn service_type(svc: &Value) -> &str {
    svc.pointer("/spec/type")
        .and_then(Value::as_str)
        .unwrap_or("ClusterIP")
}

fn is_open_cidr(peer: &Value) -> bool {
    let Some(block) = peer.get("ipBlock") else {
        return false;
    };
    let cidr = block.get("cidr").and_then(Value::as_str).unwrap_or("");
    let has_except = block
        .get("except")
        .and_then(Value::as_array)
        .is_some_and(|e| !e.is_empty());
    (cidr == "0.0.0.0/0" || cidr == "::/0") && !has_except
}

/// Whether the policy allows all peers in `direction` ("ingress"/"egress").
/// A rule with no peer list (or an empty one) allows all peers, as does an
/// unrestricted `0.0.0.0/0` or `::/0` block.
fn allows_all(np: &Value, direction: &str) -> bool {
    let peers_field = if direction == "ingress" { "from" } else { "to" };
    let Some(rules) = np
        .pointer(&format!("/spec/{direction}"))
        .and_then(Value::as_array)
    else {
        return false;
    };
    rules.iter().any(|rule| match rule.get(peers_field) {
        None => true,
        Some(Value::Array(peers)) => peers.is_empty() || peers.iter().any(is_open_cidr),
        Some(_) => false,
    })
}

fn exposure_changes(
    before: &BTreeMap<Key, &Value>,
    after: &BTreeMap<Key, &Value>,
) -> Vec<ExposureChange> {
    let mut out = Vec::new();

    for (k, obj) in after {
        let (kind, ns, name) = k;
        let existed = before.get(k);
        match kind.as_str() {
            "Ingress" if existed.is_none() => out.push(ExposureChange::NewIngress {
                namespace: ns.clone(),
                name: name.clone(),
            }),
            k2 if GATEWAY_KINDS.contains(&k2) && existed.is_none() => {
                out.push(ExposureChange::NewGatewayResource {
                    kind: kind.clone(),
                    namespace: ns.clone(),
                    name: name.clone(),
                })
            }
            "Service" => {
                let new_type = service_type(obj);
                let exposed = matches!(new_type, "LoadBalancer" | "NodePort");
                let was = existed.map(|b| service_type(b));
                if exposed && was != Some(new_type) {
                    out.push(ExposureChange::ServiceExposed {
                        namespace: ns.clone(),
                        name: name.clone(),
                        service_type: new_type.to_string(),
                    });
                }
            }
            "NetworkPolicy" => {
                for direction in ["ingress", "egress"] {
                    let now_open = allows_all(obj, direction);
                    let was_open = existed.is_some_and(|b| allows_all(b, direction));
                    if now_open && !was_open {
                        out.push(ExposureChange::NetworkPolicyAllowsAll {
                            namespace: ns.clone(),
                            name: name.clone(),
                            direction: direction.to_string(),
                        });
                    }
                }
            }
            _ => {}
        }
    }

    for (kind, ns, name) in before.keys() {
        if kind == "NetworkPolicy" && !after.contains_key(&(kind.clone(), ns.clone(), name.clone()))
        {
            out.push(ExposureChange::NetworkPolicyRemoved {
                namespace: ns.clone(),
                name: name.clone(),
            });
        }
    }

    out
}

fn permission_changed(before: &BTreeMap<Key, &Value>, after: &BTreeMap<Key, &Value>) -> bool {
    let relevant = |k: &Key| PERMISSION_KINDS.contains(&k.0.as_str());
    let added_or_changed = after
        .iter()
        .filter(|(k, _)| relevant(k))
        .any(|(k, v)| before.get(k) != Some(v));
    let removed = before
        .keys()
        .filter(|k| relevant(k))
        .any(|k| !after.contains_key(k));
    added_or_changed || removed
}
