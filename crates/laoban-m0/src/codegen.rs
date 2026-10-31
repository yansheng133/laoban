//! Step 2: the files the AI generates. Exactly four keys are accepted:
//! `web/index.html`, `web/app.js`, `web/style.css` and `api/handler.py`.
//! Anything else, a missing key, a non-string value or a file over the
//! size limits is rejected as invalid output.

use serde_json::{json, Value};

use crate::plan::strip_fence;
use crate::InvalidOutput;

/// Per-file limit.
pub const MAX_FILE_BYTES: usize = 64 * 1024;
/// Limit for the four files together.
pub const MAX_TOTAL_BYTES: usize = 192 * 1024;

/// Response format appended to the system prompt for the code call.
pub const CODEGEN_INSTRUCTIONS: &str = r#"Task: write the code for the approved application plan in the data block.
Respond with exactly one JSON object and nothing else (no Markdown fences), with exactly these keys and no others:
{"web": {"index.html": string, "app.js": string, "style.css": string}, "api": {"handler.py": string}}
Rules:
- handler.py must define `def handle(method: str, path: str, body: bytes) -> tuple[int, dict]`. `path` is the request path without the api/ prefix and without the query string, for example "/orders"; do not rely on query strings, pass input in a JSON body. Return (HTTP status, JSON-serializable dict). Import only the Python standard library. Keep all data in module-level variables (in memory). Requests are handled one at a time.
- app.js must call the API with relative URLs only: `api/<endpoint path without the leading slash>`, for example fetch("api/orders"). Never use absolute URLs or a leading slash, because the page is served under a proxy sub-path.
- index.html must load style.css and app.js with relative URLs ("style.css", "app.js") and no external resources (no CDNs, fonts or analytics).
- Text shown to users is in the same language as the app description.
- Each file at most 60000 bytes."#;

/// The four generated files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFiles {
    pub index_html: String,
    pub app_js: String,
    pub style_css: String,
    pub handler_py: String,
}

impl GeneratedFiles {
    /// The same shape the code call returns, so [`parse_generated`] reads
    /// it back with the same checks.
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "web": {
                "index.html": self.index_html,
                "app.js": self.app_js,
                "style.css": self.style_css,
            },
            "api": { "handler.py": self.handler_py },
        })
    }

    pub fn total_bytes(&self) -> usize {
        self.index_html.len() + self.app_js.len() + self.style_css.len() + self.handler_py.len()
    }
}

/// Extra instructions for the code call when an existing app is changed.
pub const EDIT_CODEGEN_INSTRUCTIONS: &str = r#"This is a change to an existing application. The data block current_code holds its current four files in the same JSON shape as the response. Change them so they implement the approved plan, keep everything the plan does not change (including working behaviour and styling), and return all four files in full."#;

fn exact_object<'a>(
    v: &'a Value,
    what: &str,
    keys: &[&str],
) -> Result<&'a serde_json::Map<String, Value>, InvalidOutput> {
    let obj = v
        .as_object()
        .ok_or_else(|| InvalidOutput(format!("{what} is not an object")))?;
    // The offending key is AI output, so it is not echoed in the error.
    if obj.keys().any(|k| !keys.contains(&k.as_str())) {
        return Err(InvalidOutput(format!(
            "{what} has a key that is not allowed"
        )));
    }
    if obj.len() != keys.len() {
        return Err(InvalidOutput(format!("{what} is missing a key")));
    }
    Ok(obj)
}

/// The `api` object, accepting one Python file under another name (models
/// sometimes call it after a module of the plan, such as `server.py`) as
/// `handler.py`. Its content must still define `handle()`, checked later.
fn api_object(v: &Value) -> Result<serde_json::Map<String, Value>, InvalidOutput> {
    if let Some(obj) = v.as_object() {
        if let [(key, content)] = obj.iter().collect::<Vec<_>>()[..] {
            if key != "handler.py" && key.ends_with(".py") && !key.contains('/') {
                return Ok([("handler.py".to_string(), content.clone())]
                    .into_iter()
                    .collect());
            }
        }
    }
    exact_object(v, "api", &["handler.py"]).cloned()
}

fn file(obj: &serde_json::Map<String, Value>, key: &str) -> Result<String, InvalidOutput> {
    let s = obj[key]
        .as_str()
        .ok_or_else(|| InvalidOutput(format!("{key} is not a string")))?;
    if s.len() > MAX_FILE_BYTES {
        return Err(InvalidOutput(format!(
            "{key} exceeds {MAX_FILE_BYTES} bytes"
        )));
    }
    Ok(s.to_string())
}

/// Parse and check the code generation response.
pub fn parse_generated(text: &str) -> Result<GeneratedFiles, InvalidOutput> {
    let v: Value = serde_json::from_str(strip_fence(text))
        .map_err(|e| InvalidOutput(format!("files are not JSON: {e}")))?;
    let top = exact_object(&v, "response", &["web", "api"])?;
    let web = exact_object(&top["web"], "web", &["index.html", "app.js", "style.css"])?;
    let api = api_object(&top["api"])?;
    let files = GeneratedFiles {
        index_html: file(web, "index.html")?,
        app_js: file(web, "app.js")?,
        style_css: file(web, "style.css")?,
        handler_py: file(&api, "handler.py")?,
    };
    if files.total_bytes() > MAX_TOTAL_BYTES {
        return Err(InvalidOutput(format!(
            "files exceed {MAX_TOTAL_BYTES} bytes in total"
        )));
    }
    if !files.handler_py.contains("def handle(") {
        return Err(InvalidOutput("handler.py does not define handle()".into()));
    }
    Ok(files)
}

/// The four files with their paths ("web/index.html", ...).
fn fields(f: &GeneratedFiles) -> [(&'static str, &String); 4] {
    [
        ("web/index.html", &f.index_html),
        ("web/app.js", &f.app_js),
        ("web/style.css", &f.style_css),
        ("api/handler.py", &f.handler_py),
    ]
}

/// Per file path, the first 16 hex digits of its sha256: enough for the
/// page to tell which files two versions differ in, without the code.
pub fn file_digests(f: &GeneratedFiles) -> serde_json::Map<String, serde_json::Value> {
    use sha2::{Digest, Sha256};
    fields(f)
        .iter()
        .map(|(path, text)| {
            let d = Sha256::digest(text.as_bytes());
            let hex: String = d.iter().take(8).map(|b| format!("{b:02x}")).collect();
            (path.to_string(), serde_json::Value::String(hex))
        })
        .collect()
}

/// Paths of the files that differ between `a` and `b`.
pub fn changed_files(a: &GeneratedFiles, b: &GeneratedFiles) -> Vec<&'static str> {
    fields(a)
        .iter()
        .zip(fields(b).iter())
        .filter(|((_, x), (_, y))| x != y)
        .map(|((p, _), _)| *p)
        .collect()
}

/// Three-way merge per file. `base` is the code the AI started from,
/// `ours` what it wrote, `theirs` the code now (edited by hand since). A
/// file changed on one side only takes that side; changed on both to
/// different contents is a conflict, and the paths are returned.
pub fn merge_files(
    base: &GeneratedFiles,
    ours: &GeneratedFiles,
    theirs: &GeneratedFiles,
) -> Result<GeneratedFiles, Vec<&'static str>> {
    let pick = |b: &String, o: &String, t: &String| -> Option<String> {
        if o == t || t == b {
            Some(o.clone())
        } else if o == b {
            Some(t.clone())
        } else {
            None
        }
    };
    let merged = [
        pick(&base.index_html, &ours.index_html, &theirs.index_html),
        pick(&base.app_js, &ours.app_js, &theirs.app_js),
        pick(&base.style_css, &ours.style_css, &theirs.style_css),
        pick(&base.handler_py, &ours.handler_py, &theirs.handler_py),
    ];
    let conflicts: Vec<&'static str> = merged
        .iter()
        .zip(fields(base).iter())
        .filter(|(m, _)| m.is_none())
        .map(|(_, (p, _))| *p)
        .collect();
    if !conflicts.is_empty() {
        return Err(conflicts);
    }
    let [index_html, app_js, style_css, handler_py] = merged.map(Option::unwrap_or_default);
    Ok(GeneratedFiles {
        index_html,
        app_js,
        style_css,
        handler_py,
    })
}

/// Told to the code call when files were edited by hand during a round;
/// the data block manual_edits lists the lines.
pub const MANUAL_EDITS_INSTRUCTIONS: &str = r#"Some files were edited by hand after the change was discussed. The data block manual_edits lists, per file, lines that were added by hand (keep every one of them, unchanged, unless the approved plan explicitly says to change that line) and lines that were removed by hand (do not bring them back)."#;

/// Lines added and removed by hand in one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualEdit {
    pub path: &'static str,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

fn line_set(text: &str) -> std::collections::BTreeSet<&str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect()
}

/// What was edited by hand between `start` and `now`, per file. Lines are
/// compared trimmed; blank lines are ignored.
pub fn manual_edits(start: &GeneratedFiles, now: &GeneratedFiles) -> Vec<ManualEdit> {
    fields(start)
        .iter()
        .zip(fields(now).iter())
        .filter(|((_, a), (_, b))| a != b)
        .filter_map(|((path, a), (_, b))| {
            let (sa, sb) = (line_set(a), line_set(b));
            let added: Vec<String> = sb.difference(&sa).map(|l| l.to_string()).collect();
            let removed: Vec<String> = sa.difference(&sb).map(|l| l.to_string()).collect();
            (!added.is_empty() || !removed.is_empty()).then_some(ManualEdit {
                path,
                added,
                removed,
            })
        })
        .collect()
}

/// The data block for [`MANUAL_EDITS_INSTRUCTIONS`].
pub fn manual_edits_block(edits: &[ManualEdit]) -> String {
    let v: Vec<Value> = edits
        .iter()
        .map(|e| json!({"file": e.path, "addedLines": e.added, "removedLines": e.removed}))
        .collect();
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

/// Files whose hand edits the AI's code drops: a line added by hand is
/// missing, or a line removed by hand is back. Files the AI did not change
/// (`ours` equal to `now`) are fine whatever was edited.
pub fn lost_edits(
    edits: &[ManualEdit],
    now: &GeneratedFiles,
    ours: &GeneratedFiles,
) -> Vec<&'static str> {
    let ours_by_path: std::collections::HashMap<&str, &String> = fields(ours).into_iter().collect();
    let now_by_path: std::collections::HashMap<&str, &String> = fields(now).into_iter().collect();
    edits
        .iter()
        .filter(|e| ours_by_path[e.path] != now_by_path[e.path])
        .filter(|e| {
            let o = line_set(ours_by_path[e.path]);
            e.added.iter().any(|l| !o.contains(l.as_str()))
                || e.removed.iter().any(|l| o.contains(l.as_str()))
        })
        .map(|e| e.path)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ok() -> Value {
        json!({"web": {"index.html": "<p>", "app.js": "x", "style.css": "y"},
               "api": {"handler.py": "def handle(m, p, b):\n    return 200, {}\n"}})
    }

    #[test]
    fn accepts_exact_shape() {
        let f = parse_generated(&ok().to_string()).unwrap();
        assert_eq!(f.app_js, "x");
        assert!(parse_generated(&format!("```json\n{}\n```", ok())).is_ok());
    }

    #[test]
    fn accepts_one_api_file_under_another_name() {
        let renamed = json!({"web": {"index.html": "<p>", "app.js": "x", "style.css": "y"},
                             "api": {"server.py": "def handle(m, p, b):\n    return 200, {}\n"}});
        let f = parse_generated(&renamed.to_string()).unwrap();
        assert!(f.handler_py.contains("def handle("));
        // Still refused: not Python, a path, or without handle().
        for (key, body) in [
            ("server.js", "def handle(m, p, b): pass"),
            ("a/server.py", "def handle(m, p, b): pass"),
            ("server.py", "print(1)"),
        ] {
            let mut v = renamed.clone();
            v["api"] = json!({ key: body });
            assert!(parse_generated(&v.to_string()).is_err(), "{key}");
        }
    }

    #[test]
    fn rejects_other_shapes() {
        let mut extra_top = ok();
        extra_top["k8s"] = json!({"deploy.yaml": "x"});
        let mut extra_web = ok();
        extra_web["web"]["server.py"] = json!("x");
        let mut extra_api = ok();
        extra_api["api"]["main.py"] = json!("x");
        let mut missing = ok();
        missing["web"].as_object_mut().unwrap().remove("style.css");
        let mut not_string = ok();
        not_string["web"]["app.js"] = json!(1);
        let mut no_handle = ok();
        no_handle["api"]["handler.py"] = json!("print(1)");
        let mut path_key = ok();
        path_key["web"].as_object_mut().unwrap().remove("app.js");
        path_key["web"]["../app.js"] = json!("x");
        for bad in [
            extra_top, extra_web, extra_api, missing, not_string, no_handle, path_key,
        ] {
            assert!(parse_generated(&bad.to_string()).is_err(), "{bad}");
        }
        assert!(parse_generated("not json").is_err());
        assert!(parse_generated("[]").is_err());
    }

    #[test]
    fn size_limits() {
        let mut big = ok();
        big["web"]["app.js"] = json!("a".repeat(MAX_FILE_BYTES + 1));
        assert!(parse_generated(&big.to_string()).is_err());
        let mut at_limit = ok();
        at_limit["web"]["app.js"] = json!("a".repeat(MAX_FILE_BYTES));
        assert!(parse_generated(&at_limit.to_string()).is_ok());
        let mut total = ok();
        for k in ["index.html", "app.js", "style.css"] {
            total["web"][k] = json!("a".repeat(MAX_FILE_BYTES));
        }
        total["api"]["handler.py"] = json!(format!("def handle(m,p,b): pass\n{}", "#".repeat(100)));
        assert!(parse_generated(&total.to_string()).is_err());
    }

    #[test]
    fn three_way_merge_per_file() {
        let f = |a: &str, b: &str| GeneratedFiles {
            index_html: a.into(),
            app_js: b.into(),
            style_css: "c".into(),
            handler_py: "def handle(m, p, b):\n    return 200, {}\n".into(),
        };
        let base = f("title", "js");
        let ours = f("title", "js2"); // AI changed app.js
        let theirs = f("title by hand", "js"); // someone changed index.html
        let m = merge_files(&base, &ours, &theirs).unwrap();
        assert_eq!(
            (m.index_html.as_str(), m.app_js.as_str()),
            ("title by hand", "js2")
        );
        assert_eq!(changed_files(&base, &m), ["web/index.html", "web/app.js"]);
        // Both changed app.js differently.
        let theirs2 = f("title", "js by hand");
        assert_eq!(
            merge_files(&base, &ours, &theirs2).unwrap_err(),
            ["web/app.js"]
        );
        // Both made the same change: fine.
        assert!(merge_files(&base, &ours, &ours).is_ok());
    }

    #[test]
    fn hand_edits_dropped_by_the_ai_are_found() {
        let f = |py: &str| GeneratedFiles {
            index_html: "<p>x</p>".into(),
            app_js: "a();".into(),
            style_css: "p{}".into(),
            handler_py: py.into(),
        };
        let start = f("def handle():\n    return 1\n# old\n");
        let now = f("# by hand\ndef handle():\n    return 1\n");
        let edits = manual_edits(&start, &now);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].path, "api/handler.py");
        assert_eq!(edits[0].added, ["# by hand"]);
        assert_eq!(edits[0].removed, ["# old"]);
        assert!(manual_edits_block(&edits).contains("# by hand"));

        // AI keeps the line (and changes something else): fine.
        let kept = f("# by hand\ndef handle():\n    return 2\n");
        assert!(lost_edits(&edits, &now, &kept).is_empty());
        // AI drops it, or brings the removed line back: lost.
        let dropped = f("def handle():\n    return 2\n");
        assert_eq!(lost_edits(&edits, &now, &dropped), ["api/handler.py"]);
        let back = f("# by hand\ndef handle():\n    return 2\n# old\n");
        assert_eq!(lost_edits(&edits, &now, &back), ["api/handler.py"]);
        // AI does not touch the file: fine.
        assert!(lost_edits(&edits, &now, &now).is_empty());
        // Nothing edited by hand: nothing to check.
        assert!(manual_edits(&now, &now).is_empty());
    }
}

#[cfg(test)]
mod digest_tests {
    use super::*;

    #[test]
    fn digests_change_only_for_the_changed_file() {
        let a = GeneratedFiles {
            index_html: "<p>a</p>".into(),
            app_js: "x()".into(),
            style_css: "p{}".into(),
            handler_py: "def handle(m, p, b):\n    return 200, {}\n".into(),
        };
        let mut b = a.clone();
        b.index_html.push_str("<!-- edit -->");
        let (da, db) = (file_digests(&a), file_digests(&b));
        assert_eq!(da.len(), 4);
        assert_eq!(da["web/index.html"].as_str().unwrap().len(), 16);
        assert_ne!(da["web/index.html"], db["web/index.html"]);
        for p in ["web/app.js", "web/style.css", "api/handler.py"] {
            assert_eq!(da[p], db[p], "{p}");
        }
    }
}
