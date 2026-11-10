//! Subagents: the code for an approved plan written by two AI calls at the
//! same time, one for the web part (`index.html`, `app.js`, `style.css`)
//! and one for the API part (`handler.py`), instead of one call for all
//! four files. Each call writes fewer files, so the code is ready sooner;
//! a round whose scope is one part only runs that part's subagent and
//! keeps the other part's files as they are.
//!
//! The two subagents never see each other's output. What keeps them in
//! step is the plan: both get the same approved plan, whose
//! `components.api.endpoints` is the contract between them. After both
//! finish, [`contract_gaps`] compares the two sides and the result is
//! shown with the run (it does not block publishing: the check reads code
//! as text and can be wrong).

use std::collections::BTreeSet;
use std::sync::OnceLock;

use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};

use crate::codegen::{parse_generated, GeneratedFiles};
use crate::plan::{strip_fence, AppPlan};
use crate::rounds::Part;
use crate::InvalidOutput;

/// Shared by both subagents, after the general code rules.
const SUBAGENT_COMMON: &str = r#"You are one of two subagents writing this application at the same time: one writes the web front end, the other writes the API. You do not see the other's code. The contract between the two is exactly the endpoints in approved_plan.components.api.endpoints: the front end calls only those endpoints, and the API implements all of them. Each endpoint's "request" and "response" are JSON examples: send and return exactly those shapes and field names, nothing renamed or nested differently. Where an example is missing, follow these rules, which the other subagent follows too: a response is always a JSON object, never a bare list; a list goes under "items"; records have an "id" field; a request that refers to a record uses "<record>_id"; an error is a non-2xx status with {"error": message}."#;

/// For the web subagent.
pub const WEB_SUBAGENT_INSTRUCTIONS: &str = r#"Task: write the web front end for the approved application plan in the data block.
Respond with exactly one JSON object and nothing else (no Markdown fences), with exactly these keys and no others:
{"web": {"index.html": string, "app.js": string, "style.css": string}}
Rules:
- app.js must call the API with relative URLs only: `api/<endpoint path without the leading slash>`, for example fetch("api/orders"). Never use absolute URLs or a leading slash, because the page is served under a proxy sub-path. Send input as a JSON body (Content-Type: application/json), never in the query string.
- index.html must load style.css and app.js with relative URLs ("style.css", "app.js") and no external resources (no CDNs, fonts or analytics).
- Show the API's error responses to the user instead of failing silently.
- Text shown to users is in the same language as the app description.
- Each file at most 60000 bytes."#;

/// For the API subagent.
pub const API_SUBAGENT_INSTRUCTIONS: &str = r#"Task: write the API for the approved application plan in the data block.
Respond with exactly one JSON object and nothing else (no Markdown fences), with exactly these keys and no others:
{"api": {"handler.py": string}}
Rules:
- handler.py must define `def handle(method: str, path: str, body: bytes) -> tuple[int, dict]`. `path` is the request path without the api/ prefix and without the query string, for example "/orders"; input comes as a JSON body. Return (HTTP status, JSON-serializable dict). Implement every endpoint of the plan, and answer other paths with 404 and other methods with 405. Import only the Python standard library. Keep all data in module-level variables (in memory). Requests are handled one at a time.
- Error messages in responses are in the same language as the app description.
- At most 60000 bytes."#;

/// Added to a subagent's instructions when an existing app is changed.
pub const EDIT_SUBAGENT_INSTRUCTIONS: &str = r#"This is a change to an existing application. The data block current_code holds the current files of your part in the same JSON shape as the response. Change them so they implement the approved plan, keep everything the plan does not change (including working behaviour and styling), and return your part's files in full."#;

impl Part {
    /// The name used in the API and the page.
    pub fn name(self) -> &'static str {
        match self {
            Part::Web => "web",
            Part::Api => "api",
        }
    }

    /// The task-specific instructions of this part's subagent.
    pub fn instructions(self) -> String {
        let task = match self {
            Part::Web => WEB_SUBAGENT_INSTRUCTIONS,
            Part::Api => API_SUBAGENT_INSTRUCTIONS,
        };
        format!("{task}\n{SUBAGENT_COMMON}")
    }

    /// This part's files of `files`, in the response shape (the value of
    /// the `web` or `api` key).
    pub fn of(self, files: &GeneratedFiles) -> Value {
        files.to_json()[self.name()].clone()
    }
}

/// Read one subagent's answer: the value of its part's key. Every check
/// on the files themselves is made by [`assemble`], once for all four.
pub fn parse_part(text: &str, part: Part) -> Result<Value, InvalidOutput> {
    let v: Value = serde_json::from_str(strip_fence(text))
        .map_err(|e| InvalidOutput(format!("{} subagent answer is not JSON: {e}", part.name())))?;
    let obj = v.as_object().ok_or_else(|| {
        InvalidOutput(format!("{} subagent answer is not an object", part.name()))
    })?;
    // Models sometimes repeat the other part as well: it is ignored, the
    // other subagent (or the current code) provides it.
    obj.get(part.name())
        .cloned()
        .ok_or_else(|| InvalidOutput(format!("{} subagent answer is missing a key", part.name())))
}

/// The four files from the parts that were written and, for a part no
/// subagent wrote, `keep` (the current code). Checked like the answer of
/// a single code call.
pub fn assemble(
    web: Option<Value>,
    api: Option<Value>,
    keep: Option<&GeneratedFiles>,
) -> Result<GeneratedFiles, InvalidOutput> {
    let take = |written: Option<Value>, part: Part| -> Result<Value, InvalidOutput> {
        match (written, keep) {
            (Some(v), _) => Ok(v),
            (None, Some(k)) => Ok(part.of(k)),
            (None, None) => Err(InvalidOutput(format!("no {} files", part.name()))),
        }
    };
    let all = json!({ "web": take(web, Part::Web)?, "api": take(api, Part::Api)? });
    parse_generated(&all.to_string())
}

/// What one subagent did, as shown on the page while and after it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentRun {
    /// "web" or "api".
    pub part: &'static str,
    /// "running", "done", "failed" or "skipped" (the part was kept).
    pub state: &'static str,
    /// Whole seconds from start to finish (so far, while running).
    pub seconds: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// Files this subagent writes.
    pub files: Vec<&'static str>,
}

impl SubagentRun {
    pub fn new(part: Part, state: &'static str) -> Self {
        SubagentRun {
            part: part.name(),
            state,
            seconds: 0,
            input_tokens: 0,
            output_tokens: 0,
            model: String::new(),
            files: part.files().to_vec(),
        }
    }
}

/// A place where the web and the API code disagree with the plan.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractGap {
    /// "apiMissing": a plan endpoint handler.py does not seem to handle.
    /// "unknownCall": app.js calls a path that is not in the plan.
    pub kind: &'static str,
    /// The endpoint path, or the path app.js calls (`/<segment>`).
    pub path: String,
}

/// First fixed segment of an endpoint path: `/orders/{id}` → `orders`.
fn first_segment(path: &str) -> Option<&str> {
    path.split('/')
        .find(|s| !s.is_empty())
        .filter(|s| !s.contains(['{', '<', ':', '*']))
}

/// Compare the code of both parts with the plan's endpoints. Reads code as
/// text, so it only finds clear mismatches: an endpoint whose first path
/// segment does not appear in handler.py at all, and an `api/<segment>`
/// in app.js that no endpoint starts with.
pub fn contract_gaps(plan: &AppPlan, files: &GeneratedFiles) -> Vec<ContractGap> {
    static CALL: OnceLock<Regex> = OnceLock::new();
    let call = CALL.get_or_init(|| Regex::new(r#"["'`]api/([A-Za-z0-9_-]+)"#).unwrap());
    let segments: BTreeSet<&str> = plan
        .endpoints
        .iter()
        .filter_map(|e| first_segment(&e.path))
        .collect();
    let mut gaps = BTreeSet::new();
    for e in &plan.endpoints {
        if let Some(seg) = first_segment(&e.path) {
            if !files.handler_py.contains(seg) {
                gaps.insert(ContractGap {
                    kind: "apiMissing",
                    path: e.path.clone(),
                });
            }
        }
    }
    // Only when the plan has fixed segments to compare with.
    if !segments.is_empty() {
        for c in call.captures_iter(&files.app_js) {
            let seg = &c[1];
            if !segments.contains(seg) {
                gaps.insert(ContractGap {
                    kind: "unknownCall",
                    path: format!("/{seg}"),
                });
            }
        }
    }
    gaps.into_iter().take(20).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::Endpoint;

    fn files(app_js: &str, handler: &str) -> GeneratedFiles {
        GeneratedFiles {
            index_html: "<html></html>".into(),
            app_js: app_js.into(),
            style_css: "body{}".into(),
            handler_py: handler.into(),
        }
    }

    fn plan(paths: &[&str]) -> AppPlan {
        AppPlan {
            app_name: None,
            summary: String::new(),
            steps: Vec::new(),
            web_modules: Vec::new(),
            api_modules: Vec::new(),
            endpoints: paths
                .iter()
                .map(|p| Endpoint {
                    method: "GET".into(),
                    path: (*p).into(),
                    purpose: String::new(),
                    request: String::new(),
                    response: String::new(),
                })
                .collect(),
            notes: String::new(),
        }
    }

    #[test]
    fn parse_part_takes_its_own_key_only() {
        let web = parse_part(
            r#"{"web": {"index.html": "a", "app.js": "b", "style.css": "c"}, "api": {"x.py": 1}}"#,
            Part::Web,
        )
        .unwrap();
        assert_eq!(web["app.js"], "b");
        assert!(parse_part(r#"{"api": {}}"#, Part::Web).is_err());
        assert!(parse_part("not json", Part::Api).is_err());
    }

    #[test]
    fn assemble_checks_like_one_call_and_keeps_a_skipped_part() {
        let web = json!({"index.html": "i", "app.js": "a", "style.css": "s"});
        let api = json!({"handler.py": "def handle(method, path, body):\n    return 200, {}"});
        let f = assemble(Some(web.clone()), Some(api), None).unwrap();
        assert_eq!(f.app_js, "a");
        // Without handle() it fails the same check as a single call.
        let bad = json!({"handler.py": "print(1)"});
        assert!(assemble(Some(web.clone()), Some(bad), None).is_err());
        // A part nobody wrote comes from the current code.
        let current = files("old", "def handle(m, p, b):\n    return 200, {}");
        let f = assemble(Some(web), None, Some(&current)).unwrap();
        assert_eq!(f.app_js, "a");
        assert_eq!(f.handler_py, current.handler_py);
        // And without current code it is an error.
        assert!(assemble(None, None, None).is_err());
    }

    #[test]
    fn subagent_instructions_name_their_part_and_the_contract() {
        let web = Part::Web.instructions();
        assert!(web.contains(r#"{"web": {"index.html""#));
        assert!(web.contains("approved_plan.components.api.endpoints"));
        let api = Part::Api.instructions();
        assert!(api.contains(r#"{"api": {"handler.py""#));
        assert!(!api.contains("index.html\": string"));
    }

    #[test]
    fn contract_gaps_find_clear_mismatches_only() {
        let p = plan(&["/orders", "/orders/{id}", "/menu"]);
        let ok = files(
            "fetch('api/orders'); fetch(`api/orders/${id}`); fetch(\"api/menu\")",
            "def handle(m, path, b):\n    if path == '/orders': ...\n    if path == '/menu': ...",
        );
        assert!(contract_gaps(&p, &ok).is_empty());
        let bad = files(
            "fetch('api/orders'); fetch('api/cart')",
            "def handle(m, path, b):\n    if path == '/orders': ...",
        );
        assert_eq!(
            contract_gaps(&p, &bad),
            vec![
                ContractGap {
                    kind: "apiMissing",
                    path: "/menu".into()
                },
                ContractGap {
                    kind: "unknownCall",
                    path: "/cart".into()
                },
            ]
        );
        // A plan without fixed segments: nothing to compare app.js with.
        assert!(contract_gaps(
            &plan(&["/{x}"]),
            &files("fetch('api/a')", "def handle(): pass")
        )
        .is_empty());
    }
}
