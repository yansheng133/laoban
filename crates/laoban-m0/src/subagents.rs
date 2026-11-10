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
- Attach event handlers in app.js with addEventListener, never with inline attributes such as onclick (functions inside app.js are not global).
- Show the API's error responses to the user instead of failing silently.
- Text shown to users is in the same language as the app description, with the same script and regional wording (for example Traditional Chinese as written in Taiwan when the description is written that way).
- Each file at most 60000 bytes."#;

/// For the API subagent.
pub const API_SUBAGENT_INSTRUCTIONS: &str = r#"Task: write the API for the approved application plan in the data block.
Respond with exactly one JSON object and nothing else (no Markdown fences), with exactly these keys and no others:
{"api": {"handler.py": string}}
Rules:
- handler.py must define `def handle(method: str, path: str, body: bytes) -> tuple[int, dict]`. `path` is the request path without the api/ prefix and without the query string, for example "/orders"; input comes as a JSON body. Return (HTTP status, JSON-serializable dict). Implement every endpoint of the plan, and answer other paths with 404 and other methods with 405. Import only the Python standard library. Keep all data in module-level variables (in memory). Requests are handled one at a time.
- Error messages in responses are in the same language as the app description, with the same script and regional wording.
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
    /// It ran a second time to fix what the check found.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub fixed: bool,
    /// The advisor model it consulted (`LAOBAN_AI_ADVISOR_MODEL`), if any.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub advisor_model: String,
    /// Tokens the advisor used for this subagent (input and output).
    #[serde(skip_serializing_if = "is_zero")]
    pub advisor_tokens: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
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
            fixed: false,
            advisor_model: String::new(),
            advisor_tokens: 0,
        }
    }
}

/// A place where the web and the API code disagree with the plan.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractGap {
    /// "apiMissing": a plan endpoint handler.py does not seem to handle.
    /// "unknownCall": app.js calls a path (with that method) that is not in
    /// the plan.
    pub kind: &'static str,
    /// The HTTP method, or "" when app.js does not say which.
    pub method: String,
    /// The endpoint path, or the path app.js calls (`/cart/update`).
    pub path: String,
}

impl ContractGap {
    /// The part whose subagent should fix it.
    pub fn part(&self) -> Part {
        if self.kind == "apiMissing" {
            Part::Api
        } else {
            Part::Web
        }
    }
}

/// A path segment that stands for a value: `{id}`, `<id>`, `:id`, `*`, or
/// on the app.js side a template value (`${id}`, read as `{p}`).
fn is_param(seg: &str) -> bool {
    seg.contains(['{', '<', ':', '*', '$'])
}

fn segments(path: &str) -> Vec<&str> {
    path.trim_start_matches('/').split('/').collect()
}

fn seg_matches(a: &str, b: &str) -> bool {
    is_param(a) || is_param(b) || a == b
}

/// Does an app.js call match an endpoint path, segment by segment? A call
/// whose end is built at run time (`'api/orders/' + id`, `${f(x)}`) is a
/// prefix: it matches any longer endpoint that starts with it.
fn path_matches(endpoint: &str, call: &Call) -> bool {
    let (e, c) = (segments(endpoint), segments(&call.path));
    if call.prefix {
        e.len() > c.len() && e.iter().zip(&c).all(|(a, b)| seg_matches(a, b))
    } else {
        e.len() == c.len() && e.iter().zip(&c).all(|(a, b)| seg_matches(a, b))
    }
}

/// Does handler.py appear to handle `path`? A path without values must be
/// written out as a string ("/cart" or '/cart', or a pattern "^/cart$");
/// for a path with values, its fixed beginning ("/orders/") or every fixed
/// segment as a string.
fn handled(handler: &str, path: &str) -> bool {
    let quoted =
        |s: &str| handler.contains(&format!("\"{s}\"")) || handler.contains(&format!("'{s}'"));
    let segs = segments(path);
    if !segs.iter().any(|s| is_param(s)) {
        return quoted(path)
            || quoted(path.trim_start_matches('/'))
            || handler.contains(&format!("^{path}$"))
            || handler.contains(&format!("^{path}/?$"));
    }
    let fixed: Vec<&str> = segs.iter().take_while(|s| !is_param(s)).copied().collect();
    let prefix = format!("/{}/", fixed.join("/"));
    handler.contains(&prefix) || segs.iter().filter(|s| !is_param(s)).all(|s| quoted(s))
}

/// One `api/...` call found in app.js.
struct Call {
    method: Option<String>,
    path: String,
    prefix: bool,
}

/// The `api/...` calls in app.js, with the method written next to each
/// (`method: 'POST'`, or a bare method string as a helper's argument)
/// when there is one before the next call; `fetch(url)` alone is a GET.
fn calls(app_js: &str) -> Vec<Call> {
    static CALL: OnceLock<Regex> = OnceLock::new();
    static TPL: OnceLock<Regex> = OnceLock::new();
    static METHOD: OnceLock<Regex> = OnceLock::new();
    static BARE: OnceLock<Regex> = OnceLock::new();
    let call = CALL
        .get_or_init(|| Regex::new(r#"(fetch\s*\(\s*)?["'`]api/([A-Za-z0-9_./${}()-]*)"#).unwrap());
    let tpl = TPL.get_or_init(|| Regex::new(r"\$\{[A-Za-z0-9_.]*\}").unwrap());
    let method = METHOD.get_or_init(|| {
        Regex::new(r#"(?i)method\s*:\s*["'`](GET|POST|PUT|PATCH|DELETE)["'`]"#).unwrap()
    });
    let bare = BARE.get_or_init(|| Regex::new(r#"["'`](GET|POST|PUT|PATCH|DELETE)["'`]"#).unwrap());
    let found: Vec<_> = call.captures_iter(app_js).collect();
    let mut out = Vec::new();
    for (n, c) in found.iter().enumerate() {
        let whole = c.get(0).unwrap();
        let mut raw = tpl.replace_all(&c[2], "{p}").into_owned();
        let mut prefix = false;
        // An expression in a template (`${encodeURIComponent(id)}`): the
        // rest is built at run time.
        if let Some(k) = raw.find("${") {
            raw.truncate(k);
            prefix = true;
        }
        let raw = raw.trim_end_matches('.').to_string();
        if raw.ends_with('/') {
            prefix = true;
        }
        let path = format!("/{}", raw.trim_end_matches('/'));
        // The method is in the text after the URL, before the next call.
        let end = found
            .get(n + 1)
            .map(|x| x.get(0).unwrap().start())
            .unwrap_or(app_js.len());
        let window_end = end.min(whole.end() + 400);
        let window = &app_js[whole.end()..window_end];
        let m = method
            .captures(window)
            .or_else(|| bare.captures(window))
            .map(|m| m[1].to_ascii_uppercase())
            // `fetch(url)` alone is a GET. With a second argument built
            // elsewhere (`fetch(url, postOptions(x))`) or through a helper,
            // the method is unknown (lab, t62-rv: such POSTs were read as
            // GETs and sent the web part to a needless fix).
            .or_else(|| {
                let after = window.trim_start_matches(['"', '\'', '`']).trim_start();
                (c.get(1).is_some() && after.starts_with(')')).then(|| "GET".to_string())
            });
        out.push(Call {
            method: m,
            path,
            prefix,
        });
    }
    out
}

/// Compare the code of both parts with the plan's endpoints. Reads code as
/// text, so it only finds clear mismatches: an endpoint path handler.py
/// never writes out (or, when a path has several methods, a method it never
/// names), and an `api/...` call in app.js that matches no endpoint of the
/// plan with its method.
pub fn contract_gaps(plan: &AppPlan, files: &GeneratedFiles) -> Vec<ContractGap> {
    let mut gaps = BTreeSet::new();
    let quoted = |m: &str| {
        files.handler_py.contains(&format!("\"{m}\""))
            || files.handler_py.contains(&format!("'{m}'"))
    };
    for e in &plan.endpoints {
        let shared = plan.endpoints.iter().filter(|o| o.path == e.path).count() > 1;
        if !handled(&files.handler_py, &e.path) || (shared && !quoted(&e.method)) {
            gaps.insert(ContractGap {
                kind: "apiMissing",
                method: e.method.clone(),
                path: e.path.clone(),
            });
        }
    }
    if !plan.endpoints.is_empty() {
        for c in calls(&files.app_js) {
            let known = plan.endpoints.iter().any(|e| {
                path_matches(&e.path, &c)
                    && c.method
                        .as_deref()
                        .map_or(true, |m| m.eq_ignore_ascii_case(&e.method))
            });
            if !known {
                gaps.insert(ContractGap {
                    kind: "unknownCall",
                    method: c.method.unwrap_or_default(),
                    path: c.path,
                });
            }
        }
    }
    gaps.into_iter().take(20).collect()
}

/// Told to a subagent that runs a second time because the check found
/// gaps in its part; the data block contract_gaps lists them.
pub const FIX_SUBAGENT_INSTRUCTIONS: &str = r#"Your first answer did not match the plan's endpoints; the data block contract_gaps lists where. "apiMissing": the API must handle that endpoint exactly as the plan writes it (method and path). "unknownCall": the web page calls a path that is not in the plan; call the plan's endpoint for it instead. The data block current_code holds your first answer: fix only these gaps, keep everything else, and return your part's files in full."#;

/// The gaps of one part, as the data block of its fix.
pub fn gaps_block(gaps: &[ContractGap], part: Part) -> String {
    let mine: Vec<&ContractGap> = gaps.iter().filter(|g| g.part() == part).collect();
    serde_json::to_string_pretty(&mine).unwrap_or_default()
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

    /// Endpoints as "METHOD /path" (a bare path is a GET).
    fn plan(eps: &[&str]) -> AppPlan {
        AppPlan {
            app_name: None,
            summary: String::new(),
            steps: Vec::new(),
            web_modules: Vec::new(),
            api_modules: Vec::new(),
            endpoints: eps
                .iter()
                .map(|e| {
                    let (method, path) = e.split_once(' ').unwrap_or(("GET", e));
                    Endpoint {
                        method: method.into(),
                        path: path.into(),
                        purpose: String::new(),
                        request: String::new(),
                        response: String::new(),
                    }
                })
                .collect(),
            notes: String::new(),
        }
    }

    fn gap(kind: &'static str, method: &str, path: &str) -> ContractGap {
        ContractGap {
            kind,
            method: method.into(),
            path: path.into(),
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
            "fetch('api/orders'); fetch(`api/orders/${id}`); fetch('api/orders/' + id); fetch(\"api/menu\")",
            "def handle(m, path, b):\n    if path == '/orders': ...\n    if path.startswith('/orders/'): ...\n    if path == '/menu': ...",
        );
        assert_eq!(contract_gaps(&p, &ok), vec![]);
        let bad = files(
            "fetch('api/orders'); fetch('api/cart')",
            "def handle(m, path, b):\n    if path == '/orders': ...",
        );
        assert_eq!(
            contract_gaps(&p, &bad),
            vec![
                gap("apiMissing", "GET", "/menu"),
                gap("apiMissing", "GET", "/orders/{id}"),
                gap("unknownCall", "GET", "/cart"),
            ]
        );
        // A path with values may also be split into segments.
        let split = files("", "parts = path.strip('/').split('/')\nif parts[0] == 'orders' and len(parts) == 2: ...\nif path == '/orders' or path == '/menu': ...");
        assert_eq!(contract_gaps(&p, &split), vec![]);
    }

    /// Lab app t62-shop (2026-11-10): the API had /cart/add but not GET
    /// /cart, and the web page called /cart/update, which the plan lacks.
    #[test]
    fn contract_gaps_compare_whole_paths() {
        let p = plan(&["/cart", "POST /cart/add"]);
        let f = files(
            "fetch('api/cart/add', {method: 'POST'}); fetch('api/cart/update', {method: 'PUT'})",
            "def handle(method, path, body):\n    if method == 'POST' and path == '/cart/add': ...",
        );
        let gaps = contract_gaps(&p, &f);
        assert_eq!(
            gaps,
            vec![
                gap("apiMissing", "GET", "/cart"),
                gap("unknownCall", "PUT", "/cart/update"),
            ]
        );
        assert_eq!(gaps[0].part(), Part::Api);
        assert_eq!(gaps[1].part(), Part::Web);
        assert!(gaps_block(&gaps, Part::Api).contains("/cart\""));
        assert!(!gaps_block(&gaps, Part::Api).contains("update"));
    }

    /// Review H2: methods count, and a value in the middle of a path.
    #[test]
    fn contract_gaps_see_methods_and_values_mid_path() {
        let p = plan(&["GET /orders", "POST /orders", "GET /orders/{id}/items"]);
        // The API only answers GET /orders: POST is missing.
        let api_get_only = files(
            "fetch('api/orders'); fetch(`api/orders/${id}/items`)",
            "def handle(method, path, body):\n    if method == 'GET' and path == '/orders': ...\n    if path.startswith('/orders/'): ...",
        );
        assert_eq!(
            contract_gaps(&p, &api_get_only),
            vec![gap("apiMissing", "POST", "/orders")]
        );
        // Both methods named, and calls with a value mid-path, a run-time
        // suffix and a helper without a method all match.
        let ok = files(
            "fetch('api/orders', {method: 'POST', body: x}); fetch(`api/orders/${id}/items`); fetch('api/orders/' + id + '/items'); call('api/orders')",
            "def handle(method, path, body):\n    if path == '/orders' and method == 'GET': ...\n    if path == '/orders' and method == 'POST': ...\n    if path.startswith('/orders/'): ...",
        );
        assert_eq!(contract_gaps(&p, &ok), vec![]);
        // A web call with a method the plan does not have for that path.
        let delete = files(
            "fetch('api/orders', {method: 'DELETE'})",
            "def handle(method, path, body):\n    if path == '/orders' and method == 'GET': ...\n    if path == '/orders' and method == 'POST': ...\n    if path.startswith('/orders/'): ...",
        );
        assert_eq!(
            contract_gaps(&p, &delete),
            vec![gap("unknownCall", "DELETE", "/orders")]
        );
        // A route written as a pattern counts as handled.
        let pattern = files("", "ROUTES = [(r\"^/menu$\", menu)]");
        assert_eq!(contract_gaps(&plan(&["/menu"]), &pattern), vec![]);
        // An expression in a template is a prefix, not an unknown path.
        let expr = files(
            "fetch(`api/orders/${encodeURIComponent(id)}/items`)",
            "if path.startswith('/orders/'): ...\nif path == '/orders': 'GET' 'POST'",
        );
        assert_eq!(contract_gaps(&p, &expr), vec![]);
    }

    /// Lab app t62-rv (2026-11-10): options built by a helper are not a GET.
    #[test]
    fn options_built_elsewhere_leave_the_method_unknown() {
        let p = plan(&["POST /loans", "POST /returns", "GET /loans/active"]);
        let f = files(
            "request(fetch(\"api/loans/active\")); request(fetch(\"api/loans\", postOptions({ book_id: 1 }))); request(fetch(\"api/returns\", postOptions(x)))",
            "def handle(method, path, body):\n    if path == '/loans': ...\n    if path == '/returns': ...\n    if path == '/loans/active': ...",
        );
        assert_eq!(contract_gaps(&p, &f), vec![]);
        // A bare fetch is still a GET, and a GET the plan lacks is reported.
        let get = files(
            "fetch('api/returns')",
            "if path == '/loans' or path == '/returns' or path == '/loans/active': ...",
        );
        assert_eq!(
            contract_gaps(&p, &get),
            vec![gap("unknownCall", "GET", "/returns")]
        );
    }
}
