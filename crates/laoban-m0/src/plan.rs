//! Step 1: the plan the AI returns, its validation, and conversion to the
//! viewer's architecture graph (`web/viewer-prototype/graph-diff.mjs`).
//!
//! The AI only chooses names, modules, endpoints and descriptions. The
//! graph's shape (two services, two apps, one in-memory table) and every
//! position are fixed here; any coordinates in the AI response are ignored.

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::InvalidOutput;

/// Response format appended to the system prompt for the plan call.
pub const PLAN_INSTRUCTIONS: &str = r#"Task: plan a small web application from the user requirement in the data block. The application always has exactly two components: a static web front end (index.html, app.js, style.css) and a JSON HTTP API written as one Python module using only the standard library, keeping its data in memory. There is no database, queue or other component.
Respond with exactly one JSON object and nothing else (no Markdown fences), with these fields:
{"appName": string matching ^[a-z][a-z0-9-]{1,30}$, "summary": string, "steps": [string], "components": {"web": {"modules": [{"name": string, "purpose": string}]}, "api": {"modules": [{"name": string, "purpose": string}], "endpoints": [{"method": "GET"|"POST"|"PUT"|"PATCH"|"DELETE", "path": string starting with "/", "purpose": string}]}}, "notes": string}
Write summary, steps, purposes and notes in Traditional Chinese (Taiwan). Use at most 8 modules per component, at most 16 endpoints and at most 20 steps. Do not include positions or coordinates."#;

const MAX_STEPS: usize = 20;
const MAX_MODULES: usize = 8;
const MAX_ENDPOINTS: usize = 16;
const MAX_NAME: usize = 40;
const MAX_TEXT: usize = 500;
const MAX_SUMMARY: usize = 1_000;
const MAX_NOTES: usize = 2_000;

/// `^[a-z][a-z0-9-]{1,30}$`
pub fn valid_app_name(name: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[a-z][a-z0-9-]{1,30}$").unwrap())
        .is_match(name)
}

/// A module may be given as a bare name or as `{name, purpose}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
enum RawModule {
    Name(String),
    Full {
        name: String,
        #[serde(default)]
        purpose: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPlan {
    #[serde(default)]
    app_name: Option<String>,
    summary: String,
    steps: Vec<String>,
    components: RawComponents,
    #[serde(default)]
    notes: Value,
}

#[derive(Debug, Clone, Deserialize)]
struct RawComponents {
    web: RawWeb,
    api: RawApi,
}

#[derive(Debug, Clone, Deserialize)]
struct RawWeb {
    #[serde(default)]
    modules: Vec<RawModule>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawApi {
    #[serde(default)]
    modules: Vec<RawModule>,
    #[serde(default)]
    endpoints: Vec<Endpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Module {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub purpose: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub purpose: String,
}

/// A validated plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppPlan {
    /// As suggested by the AI; may be replaced by the user's choice.
    pub app_name: Option<String>,
    pub summary: String,
    pub steps: Vec<String>,
    pub web_modules: Vec<Module>,
    pub api_modules: Vec<Module>,
    pub endpoints: Vec<Endpoint>,
    pub notes: String,
}

impl AppPlan {
    /// The plan as given to the code generation call.
    pub fn to_codegen_json(&self, app_name: &str) -> Value {
        json!({
            "appName": app_name,
            "summary": self.summary,
            "steps": self.steps,
            "components": {
                "web": { "modules": self.web_modules },
                "api": { "modules": self.api_modules, "endpoints": self.endpoints },
            },
            "notes": self.notes,
        })
    }
}

/// Strip one surrounding Markdown code fence, if present.
pub(crate) fn strip_fence(text: &str) -> &str {
    let t = text.trim();
    let Some(rest) = t.strip_prefix("```") else {
        return t;
    };
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    match rest.trim_end().strip_suffix("```") {
        Some(inner) => inner.trim(),
        None => t,
    }
}

fn check_text(
    field: &str,
    s: &str,
    max: usize,
    allow_newlines: bool,
) -> Result<String, InvalidOutput> {
    let s = s.trim();
    if s.chars().count() > max {
        return Err(InvalidOutput(format!("{field} is too long")));
    }
    if s.chars()
        .any(|c| c.is_control() && !(allow_newlines && (c == '\n' || c == '\t')))
    {
        return Err(InvalidOutput(format!(
            "{field} contains control characters"
        )));
    }
    Ok(s.to_string())
}

fn check_modules(field: &str, raw: Vec<RawModule>) -> Result<Vec<Module>, InvalidOutput> {
    if raw.len() > MAX_MODULES {
        return Err(InvalidOutput(format!("too many {field}")));
    }
    raw.into_iter()
        .map(|m| {
            let (name, purpose) = match m {
                RawModule::Name(n) => (n, None),
                RawModule::Full { name, purpose } => (name, purpose),
            };
            let name = check_text(field, &name, MAX_NAME, false)?;
            if name.is_empty() {
                return Err(InvalidOutput(format!("{field} has an empty name")));
            }
            let purpose = check_text(field, purpose.as_deref().unwrap_or(""), MAX_TEXT, false)?;
            Ok(Module { name, purpose })
        })
        .collect()
}

fn check_endpoint(e: Endpoint) -> Result<Endpoint, InvalidOutput> {
    static PATH: OnceLock<Regex> = OnceLock::new();
    let path_re = PATH.get_or_init(|| Regex::new(r"^/[A-Za-z0-9/_.{}:-]{0,99}$").unwrap());
    let method = e.method.trim().to_ascii_uppercase();
    if !matches!(method.as_str(), "GET" | "POST" | "PUT" | "PATCH" | "DELETE") {
        return Err(InvalidOutput("endpoint method not allowed".into()));
    }
    if !path_re.is_match(e.path.trim()) || e.path.contains("..") {
        return Err(InvalidOutput("endpoint path is not allowed".into()));
    }
    Ok(Endpoint {
        method,
        path: e.path.trim().to_string(),
        purpose: check_text("endpoint purpose", &e.purpose, MAX_TEXT, false)?,
    })
}

/// Parse and validate the AI's plan response.
pub fn parse_app_plan(text: &str) -> Result<AppPlan, InvalidOutput> {
    let raw: RawPlan = serde_json::from_str(strip_fence(text))
        .map_err(|e| InvalidOutput(format!("plan is not the expected JSON: {e}")))?;

    let summary = check_text("summary", &raw.summary, MAX_SUMMARY, true)?;
    if summary.is_empty() {
        return Err(InvalidOutput("summary is empty".into()));
    }
    if raw.steps.is_empty() || raw.steps.len() > MAX_STEPS {
        return Err(InvalidOutput("steps count out of range".into()));
    }
    let steps = raw
        .steps
        .iter()
        .map(|s| check_text("step", s, MAX_TEXT, false))
        .collect::<Result<Vec<_>, _>>()?;
    if steps.iter().any(String::is_empty) {
        return Err(InvalidOutput("empty step".into()));
    }
    let web_modules = check_modules("web modules", raw.components.web.modules)?;
    let api_modules = check_modules("api modules", raw.components.api.modules)?;
    if raw.components.api.endpoints.len() > MAX_ENDPOINTS {
        return Err(InvalidOutput("too many endpoints".into()));
    }
    let endpoints = raw
        .components
        .api
        .endpoints
        .into_iter()
        .map(check_endpoint)
        .collect::<Result<Vec<_>, _>>()?;
    let notes = match raw.notes {
        Value::Null => String::new(),
        Value::String(s) => s,
        Value::Array(items) => items
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| InvalidOutput("notes must be text".into()))?
            .join("\n"),
        _ => return Err(InvalidOutput("notes must be text".into())),
    };
    let notes = check_text("notes", &notes, MAX_NOTES, true)?;

    Ok(AppPlan {
        app_name: raw.app_name.map(|s| s.trim().to_string()),
        summary,
        steps,
        web_modules,
        api_modules,
        endpoints,
        notes,
    })
}

const WEB_X: f64 = -3.0;
const API_X: f64 = 3.0;
const MODULES_PER_ROW: usize = 4;
const MODULE_SPACING: f64 = 1.5;

/// Module positions: rows of four under the parent app, centred on it.
fn module_pos(parent_x: f64, index: usize, total: usize) -> (f64, f64) {
    let row = index / MODULES_PER_ROW;
    let col = index % MODULES_PER_ROW;
    let in_row = (total - row * MODULES_PER_ROW).min(MODULES_PER_ROW);
    let x = parent_x + (col as f64 - (in_row as f64 - 1.0) / 2.0) * MODULE_SPACING;
    let z = 2.5 + row as f64 * MODULE_SPACING;
    (x, z)
}

fn module_nodes(prefix: &str, parent: &str, parent_x: f64, modules: &[Module]) -> Vec<Value> {
    modules
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let (x, z) = module_pos(parent_x, i, modules.len());
            let mut node = json!({
                "id": format!("{prefix}-{}", i + 1),
                "layer": "code",
                "kind": "module",
                "name": m.name,
                "detail": 3,
                "parent": parent,
                "pos": { "x": x, "z": z },
            });
            if !m.purpose.is_empty() {
                node["props"] = json!({ "purpose": m.purpose });
            }
            node
        })
        .collect()
}

/// The architecture graph in the viewer model. Deterministic: same plan
/// and app name, same graph.
pub fn architecture(plan: &AppPlan, app: &str) -> Value {
    let mut nodes = vec![
        json!({"id": "svc-web", "layer": "network", "kind": "service", "name": format!("{app}-web"),
               "detail": 2, "parent": "app-web", "props": {"type": "ClusterIP", "port": 8080},
               "pos": {"x": WEB_X, "z": -2.0}}),
        json!({"id": "svc-api", "layer": "network", "kind": "service", "name": format!("{app}-api"),
               "detail": 2, "parent": "app-api", "props": {"type": "ClusterIP", "port": 8080},
               "pos": {"x": API_X, "z": -2.0}}),
        json!({"id": "app-web", "layer": "code", "kind": "app", "name": format!("{app}-web"),
               "detail": 1, "props": {"lang": "HTML/JavaScript", "replicas": 1},
               "pos": {"x": WEB_X, "z": 0.0}}),
        json!({"id": "app-api", "layer": "code", "kind": "app", "name": format!("{app}-api"),
               "detail": 1, "props": {"lang": "Python", "replicas": 1, "endpoints": plan.endpoints.len()},
               "pos": {"x": API_X, "z": 0.0}}),
    ];
    nodes.extend(module_nodes("mod-web", "app-web", WEB_X, &plan.web_modules));
    nodes.extend(module_nodes("mod-api", "app-api", API_X, &plan.api_modules));
    nodes.push(
        json!({"id": "tbl-data", "layer": "data", "kind": "table", "name": format!("{app}-data"),
                      "detail": 2, "parent": "app-api", "props": {"storage": "memory"},
                      "pos": {"x": API_X, "z": 0.0}}),
    );

    let edges = json!([
        {"from": "svc-web", "to": "app-web", "kind": "routes"},
        {"from": "app-web", "to": "svc-api", "kind": "calls", "label": "HTTP api/*"},
        {"from": "svc-api", "to": "app-api", "kind": "routes"},
        {"from": "app-api", "to": "tbl-data", "kind": "writes", "label": "memory"},
    ]);
    json!({ "nodes": nodes, "edges": edges })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_names() {
        for ok in ["ab", "breakfast", "a-1", "a234567890123456789012345678901"] {
            assert!(valid_app_name(ok), "{ok}");
        }
        for bad in [
            "a",
            "1ab",
            "Ab",
            "a_b",
            "a2345678901234567890123456789012",
            "",
            "ab\n",
        ] {
            assert!(!valid_app_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn fences_are_stripped() {
        assert_eq!(strip_fence("```json\n{}\n```"), "{}");
        assert_eq!(strip_fence(" {} "), "{}");
        assert_eq!(strip_fence("```\n{}"), "```\n{}");
    }

    #[test]
    fn module_layout_rows() {
        assert_eq!(module_pos(0.0, 0, 1), (0.0, 2.5));
        assert_eq!(module_pos(0.0, 0, 2), (-0.75, 2.5));
        assert_eq!(module_pos(0.0, 4, 5), (0.0, 4.0));
    }
}
