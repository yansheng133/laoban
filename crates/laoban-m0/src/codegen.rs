//! Step 2: the files the AI generates. Exactly four keys are accepted:
//! `web/index.html`, `web/app.js`, `web/style.css` and `api/handler.py`.
//! Anything else, a missing key, a non-string value or a file over the
//! size limits is rejected as invalid output.

use serde_json::Value;

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
- Text shown to users is Traditional Chinese (Taiwan).
- Each file at most 60000 bytes."#;

/// The four generated files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFiles {
    pub index_html: String,
    pub app_js: String,
    pub style_css: String,
    pub handler_py: String,
}

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
    let api = exact_object(&top["api"], "api", &["handler.py"])?;
    let files = GeneratedFiles {
        index_html: file(web, "index.html")?,
        app_js: file(web, "app.js")?,
        style_css: file(web, "style.css")?,
        handler_py: file(api, "handler.py")?,
    };
    let total = files.index_html.len()
        + files.app_js.len()
        + files.style_css.len()
        + files.handler_py.len();
    if total > MAX_TOTAL_BYTES {
        return Err(InvalidOutput(format!(
            "files exceed {MAX_TOTAL_BYTES} bytes in total"
        )));
    }
    if !files.handler_py.contains("def handle(") {
        return Err(InvalidOutput("handler.py does not define handle()".into()));
    }
    Ok(files)
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
}
