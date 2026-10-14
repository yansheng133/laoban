//! `apps/<app>/laoban.json`: what laoban needs to list an app and to change
//! it later, written next to the manifests on every apply.
//!
//! It holds the plan and the four generated files in the same JSON shapes
//! the AI returns, so reading it back goes through the same validation
//! ([`parse_app_plan`], [`parse_generated`]) as a fresh AI response, plus a
//! short history of (masked) requests. Fleet is told to skip it with a
//! `.fleetignore` in the same directory.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::codegen::{parse_generated, GeneratedFiles};
use crate::plan::{parse_app_plan, AppPlan};
use crate::InvalidOutput;

pub const META_FILE: &str = "laoban.json";
pub const FLEETIGNORE: &str = "# Written by laoban: not a Kubernetes manifest.\nlaoban.json\n";
pub const SCHEMA: u32 = 1;
/// Oldest entries are dropped beyond this.
pub const MAX_HISTORY: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    /// RFC 3339 UTC.
    pub at: String,
    /// `create`, `edit`, `restore`, `manual` or `layout`.
    pub kind: String,
    /// The user's request, masked.
    pub request: String,
    /// The plan summary, masked.
    pub summary: String,
    pub model: String,
    /// Who made the change: the user name the Kubernetes API server
    /// verified (Rancher user ID). Empty in entries made before identity.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub by: String,
}

/// A 3D position on the layer plane, set by dragging in the page.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pos {
    pub x: f64,
    pub z: f64,
}

/// Largest coordinate accepted (same bound as the page's layout code).
pub const MAX_COORD: f64 = 100.0;
/// Most positions kept.
pub const MAX_LAYOUT_ENTRIES: usize = 100;

/// Node id to dragged position. Applied over the computed layout.
pub type Layout = BTreeMap<String, Pos>;

impl Pos {
    pub fn valid(&self) -> bool {
        self.x.is_finite()
            && self.z.is_finite()
            && self.x.abs() <= MAX_COORD
            && self.z.abs() <= MAX_COORD
    }
}

/// An app read back from the repository.
#[derive(Debug, Clone, PartialEq)]
pub struct AppMeta {
    pub app: String,
    pub plan: AppPlan,
    pub files: GeneratedFiles,
    pub history: Vec<HistoryEntry>,
    pub layout: Layout,
}

impl AppMeta {
    pub fn updated_at(&self) -> &str {
        self.history.last().map(|h| h.at.as_str()).unwrap_or("")
    }
}

/// Serialize for `laoban.json`. `history` already includes this change.
pub fn to_json(
    app: &str,
    plan: &AppPlan,
    files: &GeneratedFiles,
    history: &[HistoryEntry],
) -> String {
    to_json_with_layout(app, plan, files, history, &Layout::new())
}

/// [`to_json`] plus dragged 3D positions (omitted when empty).
pub fn to_json_with_layout(
    app: &str,
    plan: &AppPlan,
    files: &GeneratedFiles,
    history: &[HistoryEntry],
    layout: &Layout,
) -> String {
    let start = history.len().saturating_sub(MAX_HISTORY);
    let mut v = json!({
        "schema": SCHEMA,
        "appName": app,
        "plan": plan.to_codegen_json(app),
        "files": files.to_json(),
        "history": &history[start..],
    });
    if !layout.is_empty() {
        v["layout"] = json!(layout);
    }
    let mut s = serde_json::to_string_pretty(&v).unwrap_or_default();
    s.push('\n');
    s
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMeta {
    schema: u32,
    app_name: String,
    plan: Value,
    files: Value,
    #[serde(default)]
    history: Vec<HistoryEntry>,
    #[serde(default)]
    layout: Layout,
}

/// Parse `laoban.json` for `app`. Anything unexpected is rejected.
pub fn parse(app: &str, text: &str) -> Result<AppMeta, InvalidOutput> {
    let raw: RawMeta = serde_json::from_str(text)
        .map_err(|e| InvalidOutput(format!("{META_FILE} is not valid: {e}")))?;
    if raw.schema != SCHEMA {
        return Err(InvalidOutput(format!(
            "unsupported {META_FILE} schema {}",
            raw.schema
        )));
    }
    if raw.app_name != app {
        return Err(InvalidOutput(format!("{META_FILE} belongs to another app")));
    }
    let plan = parse_app_plan(&raw.plan.to_string())?;
    let files = parse_generated(&raw.files.to_string())?;
    let mut history = raw.history;
    let start = history.len().saturating_sub(MAX_HISTORY);
    history.drain(..start);
    // Positions are display hints: drop anything out of bounds.
    let layout: Layout = raw
        .layout
        .into_iter()
        .filter(|(id, p)| p.valid() && id.len() <= 100)
        .take(MAX_LAYOUT_ENTRIES)
        .collect();
    Ok(AppMeta {
        app: app.to_string(),
        plan,
        files,
        history,
        layout,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> AppPlan {
        parse_app_plan(
            &json!({
                "appName": "todo",
                "summary": "待辦清單",
                "steps": ["a"],
                "components": {"web": {"modules": ["list"]}, "api": {"modules": [], "endpoints": [{"method": "GET", "path": "/items", "purpose": "x"}]}},
                "notes": ""
            })
            .to_string(),
        )
        .unwrap()
    }

    fn files() -> GeneratedFiles {
        GeneratedFiles {
            index_html: "<html></html>\n".into(),
            app_js: "fetch(\"api/items\");\n".into(),
            style_css: "body{}\n".into(),
            handler_py: "def handle(method, path, body):\n    return 200, {}\n".into(),
        }
    }

    fn entry(n: usize) -> HistoryEntry {
        HistoryEntry {
            at: format!("2026-10-13T00:00:{n:02}Z"),
            kind: if n == 0 { "create" } else { "edit" }.into(),
            request: format!("r{n}"),
            summary: "s".into(),
            model: "model-a-test".into(),
            by: String::new(),
        }
    }

    #[test]
    fn round_trip() {
        let text = to_json("todo", &plan(), &files(), &[entry(0), entry(1)]);
        let m = parse("todo", &text).unwrap();
        assert_eq!(m.plan, plan());
        assert_eq!(m.files, files());
        assert_eq!(m.history.len(), 2);
        assert_eq!(m.updated_at(), "2026-10-13T00:00:01Z");
    }

    #[test]
    fn layout_round_trip_and_bad_positions_dropped() {
        let mut layout = Layout::new();
        layout.insert("app-web".into(), Pos { x: 1.5, z: -2.0 });
        let text = to_json_with_layout("todo", &plan(), &files(), &[entry(0)], &layout);
        assert_eq!(parse("todo", &text).unwrap().layout, layout);
        let mut v: Value = serde_json::from_str(&text).unwrap();
        v["layout"]["far"] = json!({"x": 1000.0, "z": 0.0});
        assert_eq!(parse("todo", &v.to_string()).unwrap().layout, layout);
        // No layout key at all is an empty layout.
        let plain = to_json("todo", &plan(), &files(), &[entry(0)]);
        assert!(!plain.contains("\"layout\""));
        assert!(parse("todo", &plain).unwrap().layout.is_empty());
    }

    #[test]
    fn history_is_capped() {
        let h: Vec<_> = (0..30).map(entry).collect();
        let m = parse("todo", &to_json("todo", &plan(), &files(), &h)).unwrap();
        assert_eq!(m.history.len(), MAX_HISTORY);
        assert_eq!(m.history[0].request, "r10");
    }

    #[test]
    fn rejects_wrong_app_schema_and_files() {
        let text = to_json("todo", &plan(), &files(), &[entry(0)]);
        assert!(parse("other", &text).is_err());
        let mut v: Value = serde_json::from_str(&text).unwrap();
        v["schema"] = json!(2);
        assert!(parse("todo", &v.to_string()).is_err());
        let mut v: Value = serde_json::from_str(&text).unwrap();
        v["files"]["web"]["extra.js"] = json!("x");
        assert!(parse("todo", &v.to_string()).is_err());
        assert!(parse("todo", "not json").is_err());
    }
}
