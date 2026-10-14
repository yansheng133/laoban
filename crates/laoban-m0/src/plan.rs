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
{"appName": string matching ^[a-z][a-z0-9-]{1,30}$, "summary": string, "steps": [string], "components": {"web": {"modules": [{"name": string, "purpose": string}]}, "api": {"modules": [{"name": string, "purpose": string}], "endpoints": [{"method": "GET"|"POST"|"PUT"|"PATCH"|"DELETE", "path": string starting with "/" as the API module sees it, without any /api prefix (for example "/orders"), "purpose": string}]}}, "notes": string}
Write summary, steps, purposes and notes in Traditional Chinese (Taiwan). Use at most 8 modules per component, at most 16 endpoints and at most 20 steps. Do not include positions or coordinates."#;

/// Extra instructions for the plan call when an existing app is changed.
pub const EDIT_PLAN_INSTRUCTIONS: &str = r#"This is a change to an existing application. The data block current_plan holds its current plan in the response format, and the data block change_request holds what the user wants changed. Return the complete updated plan (not only the differences), keep appName unchanged, and keep the modules and endpoints the change does not affect. In summary, describe the application as a whole after the change; make the steps the steps of this change."#;

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

/// Strip one surrounding Markdown code fence, if present. If what is left
/// still does not start with `{` (smaller local models often add a sentence
/// before or after the JSON), fall back to the span from the first `{` to the
/// last `}`. The result is parsed and validated as strictly as before.
pub(crate) fn strip_fence(text: &str) -> &str {
    let t = text.trim();
    let unfenced = match t.strip_prefix("```") {
        None => t,
        Some(rest) => {
            let rest = rest.strip_prefix("json").unwrap_or(rest);
            match rest.trim_end().strip_suffix("```") {
                Some(inner) => inner.trim(),
                None => t,
            }
        }
    };
    if unfenced.starts_with('{') {
        return unfenced;
    }
    match (unfenced.find('{'), unfenced.rfind('}')) {
        (Some(start), Some(end)) if start < end => &unfenced[start..=end],
        _ => unfenced,
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
    let path = normalize_endpoint_path(e.path.trim());
    if !path_re.is_match(&path) || path.contains("..") {
        return Err(InvalidOutput("endpoint path is not allowed".into()));
    }
    Ok(Endpoint {
        method,
        path,
        purpose: check_text("endpoint purpose", &e.purpose, MAX_TEXT, false)?,
    })
}

/// Endpoint paths are as `handler.handle()` sees them: the web server
/// forwards `api/<x>` to the API's `/<x>`. Models sometimes write the
/// browser-side path (`/api/orders`) instead, which then disagrees with the
/// code; drop that prefix (repeatedly, case-insensitively).
pub(crate) fn normalize_endpoint_path(path: &str) -> String {
    let mut p = path.trim().to_string();
    loop {
        let lower = p.to_ascii_lowercase();
        if lower == "/api" || lower == "/api/" {
            return "/".into();
        }
        if lower.starts_with("/api/") {
            p = p[4..].to_string();
        } else {
            return p;
        }
    }
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

/// Module node id from its name, so the same module keeps its id across
/// versions (the version diff matches nodes by id). Lowercase ASCII
/// letters and digits, other characters as `-`; the index when the name
/// has none of those or the id is already taken.
fn module_id(prefix: &str, name: &str, index: usize, taken: &mut Vec<String>) -> String {
    let mut slug = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.len() >= 40 {
            break;
        }
    }
    let slug = slug.trim_matches('-');
    let mut id = if slug.is_empty() {
        format!("{prefix}-{}", index + 1)
    } else {
        format!("{prefix}-{slug}")
    };
    if taken.contains(&id) {
        id = format!("{id}-{}", index + 1);
    }
    taken.push(id.clone());
    id
}

fn module_nodes(prefix: &str, parent: &str, parent_x: f64, modules: &[Module]) -> Vec<Value> {
    let mut taken = Vec::new();
    modules
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let (x, z) = module_pos(parent_x, i, modules.len());
            let mut node = json!({
                "id": module_id(prefix, &m.name, i, &mut taken),
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

/// The API's endpoints as one sorted line (`GET /menu, POST /orders`), so a
/// version diff shows when they change.
fn routes(plan: &AppPlan) -> String {
    let mut r: Vec<String> = plan
        .endpoints
        .iter()
        .map(|e| format!("{} {}", e.method, e.path))
        .collect();
    r.sort();
    r.join(", ")
}

/// [`architecture`] with dragged positions applied to the nodes they name.
pub fn architecture_with_layout(plan: &AppPlan, app: &str, layout: &crate::meta::Layout) -> Value {
    let mut arch = architecture(plan, app);
    if let Some(nodes) = arch["nodes"].as_array_mut() {
        for n in nodes {
            let Some(id) = n["id"].as_str() else { continue };
            if let Some(p) = layout.get(id) {
                n["pos"] = json!({ "x": p.x, "z": p.z });
            }
        }
    }
    arch
}

fn node_ids(plan: &AppPlan, app: &str) -> Vec<String> {
    architecture(plan, app)["nodes"]
        .as_array()
        .map(|n| {
            n.iter()
                .filter_map(|n| n["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Dragged positions for a changed plan. Positions of nodes that still
/// exist are kept, others dropped. A module id follows its name, so a
/// renamed module looks like one removed and one added: when exactly one
/// positioned module disappears from a component and exactly one new one
/// appears there, the new one takes its position.
pub fn carry_layout(
    old: &AppPlan,
    new: &AppPlan,
    app: &str,
    layout: &crate::meta::Layout,
) -> crate::meta::Layout {
    let old_ids = node_ids(old, app);
    let new_ids = node_ids(new, app);
    let mut out: crate::meta::Layout = layout
        .iter()
        .filter(|(id, _)| new_ids.contains(id))
        .map(|(id, p)| (id.clone(), *p))
        .collect();
    for prefix in ["mod-web-", "mod-api-"] {
        let removed: Vec<&String> = old_ids
            .iter()
            .filter(|id| {
                id.starts_with(prefix) && !new_ids.contains(id) && layout.contains_key(*id)
            })
            .collect();
        let added: Vec<&String> = new_ids
            .iter()
            .filter(|id| id.starts_with(prefix) && !old_ids.contains(id))
            .collect();
        if let ([gone], [new_id]) = (removed.as_slice(), added.as_slice()) {
            out.insert((*new_id).clone(), layout[*gone]);
        }
    }
    out
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
               "detail": 1, "props": {"lang": "Python", "replicas": 1, "endpoints": plan.endpoints.len(),
                                      "routes": routes(plan)},
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

    fn plan_with(web: &[&str], api: &[&str]) -> AppPlan {
        let m = |names: &[&str]| -> Vec<serde_json::Value> {
            names
                .iter()
                .map(|n| json!({"name": n, "purpose": "p"}))
                .collect()
        };
        parse_app_plan(
            &json!({
                "appName": "todo",
                "summary": "s",
                "steps": ["a"],
                "components": {
                    "web": {"modules": m(web)},
                    "api": {"modules": m(api), "endpoints": [{"method": "GET", "path": "/x", "purpose": "p"}]}
                }
            })
            .to_string(),
        )
        .unwrap()
    }

    #[test]
    fn layout_follows_a_renamed_module() {
        use crate::meta::{Layout, Pos};
        let old = plan_with(&["list", "form"], &["todo_api.py"]);
        let layout: Layout = [
            ("mod-api-todo-api-py".to_string(), Pos { x: 5.0, z: 1.0 }),
            ("mod-web-list".to_string(), Pos { x: -3.0, z: 2.0 }),
            ("svc-web".to_string(), Pos { x: -9.0, z: 0.0 }),
        ]
        .into_iter()
        .collect();
        // api module renamed, web "list" removed and nothing added there.
        let new = plan_with(&["form"], &["todo_logic.py"]);
        let out = carry_layout(&old, &new, "todo", &layout);
        let p = out["mod-api-todo-logic-py"];
        assert_eq!((p.x, p.z), (5.0, 1.0));
        assert!(!out.contains_key("mod-api-todo-api-py"));
        assert!(!out.contains_key("mod-web-list"), "removed module dropped");
        assert!(out.contains_key("svc-web"), "unchanged nodes kept");
        // Two new modules in the component: ambiguous, not carried.
        let new2 = plan_with(&["list", "form"], &["a", "b"]);
        let out2 = carry_layout(&old, &new2, "todo", &layout);
        assert!(!out2.keys().any(|k| k.starts_with("mod-api-")));
    }

    #[test]
    fn module_ids_follow_names() {
        let mut taken = Vec::new();
        assert_eq!(
            module_id("mod-web", "Menu View", 0, &mut taken),
            "mod-web-menu-view"
        );
        assert_eq!(
            module_id("mod-web", "menu_view", 1, &mut taken),
            "mod-web-menu-view-2"
        );
        assert_eq!(module_id("mod-web", "訂單", 2, &mut taken), "mod-web-3");
        assert_eq!(
            module_id("mod-api", "orders", 0, &mut Vec::new()),
            "mod-api-orders"
        );
    }

    #[test]
    fn api_prefix_is_dropped_from_endpoint_paths() {
        for (given, want) in [
            ("/api/polls", "/polls"),
            ("/API/polls/{id}", "/polls/{id}"),
            ("/api/api/x", "/x"),
            ("/api", "/"),
            ("/api/", "/"),
            ("/apiary", "/apiary"),
            ("/orders", "/orders"),
            (" /api/count ", "/count"),
        ] {
            assert_eq!(normalize_endpoint_path(given), want, "{given}");
        }
    }

    #[test]
    fn fences_are_stripped() {
        assert_eq!(strip_fence("```json\n{}\n```"), "{}");
        assert_eq!(strip_fence(" {} "), "{}");
        assert_eq!(strip_fence("```\n{}"), "{}");
        assert_eq!(
            strip_fence("Here is the plan:\n```json\n{\"a\": {}}\n```\nHope it helps."),
            "{\"a\": {}}"
        );
        assert_eq!(strip_fence("no json here"), "no json here");
        assert_eq!(strip_fence("} backwards {"), "} backwards {");
    }

    #[test]
    fn module_layout_rows() {
        assert_eq!(module_pos(0.0, 0, 1), (0.0, 2.5));
        assert_eq!(module_pos(0.0, 0, 2), (-0.75, 2.5));
        assert_eq!(module_pos(0.0, 4, 5), (0.0, 4.0));
    }
}
