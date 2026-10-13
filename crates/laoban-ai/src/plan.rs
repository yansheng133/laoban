//! The typed plan the AI must return, and the allow-list it is checked
//! against. The response is data: it is parsed and validated, never
//! executed here.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub summary: String,
    pub steps: Vec<String>,
    #[serde(default)]
    pub files_to_change: Vec<FileChange>,
    #[serde(default)]
    pub resources: Vec<ResourceRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAction {
    Create,
    Modify,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileChange {
    pub path: String,
    pub action: FileAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceAction {
    Read,
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceRef {
    pub cluster: String,
    #[serde(default)]
    pub namespace: Option<String>,
    /// Empty string for the core API group.
    #[serde(default)]
    pub api_group: String,
    pub kind: String,
    pub name: String,
    pub action: ResourceAction,
}

/// What a plan may touch. Defaults: no Secrets (any action), no objects in
/// the RBAC API group (any action, which covers escalation and binding),
/// no system namespaces, only the target cluster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlanPolicy {
    /// Kinds that may not appear at all, matched case-insensitively
    /// regardless of API group.
    pub denied_kinds: Vec<String>,
    /// API groups that may not appear at all.
    pub denied_api_groups: Vec<String>,
    /// Namespaces that may not be touched.
    pub denied_namespaces: Vec<String>,
    pub max_steps: usize,
    pub max_files: usize,
    pub max_resources: usize,
}

impl Default for PlanPolicy {
    fn default() -> Self {
        Self {
            denied_kinds: vec![
                "Secret".into(),
                "Role".into(),
                "ClusterRole".into(),
                "RoleBinding".into(),
                "ClusterRoleBinding".into(),
            ],
            denied_api_groups: vec!["rbac.authorization.k8s.io".into()],
            denied_namespaces: vec![
                "kube-system".into(),
                "cattle-system".into(),
                "cattle-fleet-system".into(),
            ],
            max_steps: 50,
            max_files: 200,
            max_resources: 200,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PlanError {
    #[error("response is not a valid plan: {0}")]
    Schema(String),
    #[error("response was cut off at max_tokens")]
    Truncated,
    #[error("model refused the request")]
    Refused,
    #[error("plan touches a different cluster `{found}` (target is `{target}`)")]
    WrongCluster { target: String, found: String },
    #[error("plan touches a denied kind `{kind}`")]
    DeniedKind { kind: String },
    #[error("plan touches a denied API group `{group}`")]
    DeniedApiGroup { group: String },
    #[error("plan touches a denied namespace `{namespace}`")]
    DeniedNamespace { namespace: String },
    #[error("plan has an unsafe file path `{path}`")]
    UnsafePath { path: String },
}

/// Parse the model's text. Accepts a bare JSON object, optionally inside a
/// single ```json fence. Anything else is a schema error.
pub fn parse_plan(text: &str) -> Result<Plan, PlanError> {
    let t = text.trim();
    let t = strip_fence(t).unwrap_or(t);
    let plan: Plan = serde_json::from_str(t).map_err(|e| PlanError::Schema(e.to_string()))?;
    if plan.summary.trim().is_empty() {
        return Err(PlanError::Schema("summary is empty".into()));
    }
    if plan.steps.is_empty() {
        return Err(PlanError::Schema("steps is empty".into()));
    }
    Ok(plan)
}

fn strip_fence(t: &str) -> Option<&str> {
    let rest = t.strip_prefix("```")?;
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    let rest = rest.strip_suffix("```")?;
    Some(rest.trim())
}

/// Check a parsed plan against the policy and the target cluster.
pub fn validate_plan(
    plan: &Plan,
    policy: &PlanPolicy,
    target_cluster: &str,
) -> Result<(), PlanError> {
    if plan.steps.len() > policy.max_steps
        || plan.files_to_change.len() > policy.max_files
        || plan.resources.len() > policy.max_resources
    {
        return Err(PlanError::Schema("plan exceeds size limits".into()));
    }
    for f in &plan.files_to_change {
        if !is_safe_relative_path(&f.path) {
            return Err(PlanError::UnsafePath {
                path: f.path.clone(),
            });
        }
    }
    for r in &plan.resources {
        if r.cluster != target_cluster {
            return Err(PlanError::WrongCluster {
                target: target_cluster.to_string(),
                found: r.cluster.clone(),
            });
        }
        let kind = r.kind.trim();
        let singular = kind
            .strip_suffix('s')
            .or_else(|| kind.strip_suffix('S'))
            .unwrap_or(kind);
        if policy
            .denied_kinds
            .iter()
            .any(|k| k.eq_ignore_ascii_case(kind) || k.eq_ignore_ascii_case(singular))
        {
            return Err(PlanError::DeniedKind {
                kind: r.kind.clone(),
            });
        }
        let group = r.api_group.trim();
        if policy
            .denied_api_groups
            .iter()
            .any(|g| g.eq_ignore_ascii_case(group))
        {
            return Err(PlanError::DeniedApiGroup {
                group: r.api_group.clone(),
            });
        }
        if let Some(ns) = &r.namespace {
            if policy
                .denied_namespaces
                .iter()
                .any(|d| d.eq_ignore_ascii_case(ns.trim()))
            {
                return Err(PlanError::DeniedNamespace {
                    namespace: ns.clone(),
                });
            }
        }
    }
    Ok(())
}

fn is_safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.starts_with('\\')
        && !path.contains(':')
        && !path.contains('\0')
        && path
            .split(['/', '\\'])
            .all(|seg| seg != ".." && !seg.is_empty())
}
