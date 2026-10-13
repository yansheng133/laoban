//! Checks that need python3: the server.py/main.py runtime test, and a
//! YAML round trip of the rendered manifests with PyYAML (a YAML 1.1
//! parser, like the Go YAML library Fleet uses). Skipped with a message
//! when python3 (or PyYAML) is not installed, as in the Rust CI image.

use std::path::PathBuf;
use std::process::Command;

use laoban_m0::codegen::GeneratedFiles;
use laoban_m0::render::{render, RenderInput, MAIN_PY, SERVER_PY};

fn python_has(module: &str) -> bool {
    Command::new("python3")
        .args(["-c", &format!("import {module}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("laoban-m0-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn server_and_main_templates_end_to_end() {
    if !python_has("http.server") {
        eprintln!("SKIPPED: python3 not available");
        return;
    }
    let out = Command::new("python3")
        .args(["-m", "unittest", "discover", "-s", "python-tests", "-v"])
        .current_dir(crate_dir())
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "python tests failed:\n{stderr}");
    assert!(stderr.contains("OK"), "{stderr}");
}

const CHECK_YAML: &str = r#"
import json, sys, yaml
root, expected = sys.argv[1], json.load(open(sys.argv[2], encoding="utf-8"))
docs = {}
for name in ["fleet.yaml", "configmap-web.yaml", "configmap-api.yaml", "web.yaml", "api.yaml"]:
    with open(f"{root}/{name}", encoding="utf-8") as f:
        docs[name] = list(yaml.safe_load_all(f))
fleet = docs["fleet.yaml"][0]
assert fleet == {"defaultNamespace": "laoban-dev", "namespace": "laoban-dev"}, fleet
for name, cm_name in (("configmap-web.yaml", "tricky-web-files"), ("configmap-api.yaml", "tricky-api-code")):
    cm = docs[name][0]
    assert cm["kind"] == "ConfigMap" and cm["metadata"]["name"] == cm_name, cm["metadata"]
    assert "app.kubernetes.io/managed-by" not in cm["metadata"]["labels"], cm["metadata"]
    for k, v in cm["data"].items():
        assert v == expected[k], (k, v, expected[k])
    assert set(cm["data"]) == {k for k in expected if k in cm["data"]}
for name in ("web.yaml", "api.yaml"):
    dep, svc = docs[name]
    for obj in (dep, svc):
        labels = obj["metadata"]["labels"]
        # Helm (under Fleet) owns this label on top-level objects.
        assert "app.kubernetes.io/managed-by" not in labels, labels
        assert labels["laoban.io/app"] == "tricky", labels
        assert all(isinstance(v, str) for v in labels.values()), labels
    pod = dep["spec"]["template"]
    assert pod["metadata"]["labels"]["app.kubernetes.io/managed-by"] == "laoban", pod["metadata"]["labels"]
    assert pod["metadata"]["labels"]["laoban.io/app"] == "tricky", pod["metadata"]["labels"]
    assert len(pod["metadata"]["annotations"]["laoban.io/content-hash"]) == 64
    spec = pod["spec"]
    assert spec["automountServiceAccountToken"] is False
    assert spec["securityContext"] == {"runAsNonRoot": True, "runAsUser": 65532, "seccompProfile": {"type": "RuntimeDefault"}}
    c = spec["containers"][0]
    assert c["image"] == "registry.suse.com/bci/python:3.13"
    assert c["securityContext"] == {
        "allowPrivilegeEscalation": False, "readOnlyRootFilesystem": True, "runAsNonRoot": True,
        "runAsUser": 65532, "capabilities": {"drop": ["ALL"]}, "seccompProfile": {"type": "RuntimeDefault"},
    }, c["securityContext"]
    assert c["resources"] == {"requests": {"cpu": "50m", "memory": "64Mi"}, "limits": {"cpu": "200m", "memory": "128Mi"}}
    assert c["ports"][0]["containerPort"] == 8080
    assert svc["spec"]["ports"][0]["port"] == 8080
    assert svc["spec"]["selector"] == dep["spec"]["selector"]["matchLabels"]
    assert all(dep["spec"]["selector"]["matchLabels"][k] == v for k, v in pod["metadata"]["labels"].items() if k in dep["spec"]["selector"]["matchLabels"])
print("YAML OK")
"#;

#[test]
fn rendered_manifests_round_trip_through_a_yaml_parser() {
    if !python_has("yaml") {
        eprintln!("SKIPPED: python3 with PyYAML not available");
        return;
    }
    // Content chosen to break naive YAML emission.
    let files = GeneratedFiles {
        index_html: "  <!-- leading spaces -->\n<p>a: b</p>\n".into(),
        app_js: "const s = \"---\";\r\n// CRLF and no final newline".into(),
        style_css: "body {\n\tcolor: red; /* tab */\n}\n\n\n".into(),
        handler_py: "# comment line\ndef handle(m, p, b):\n    return 200, {\"k\": \"yes\\u2028\"}\n\n    \nx = '\u{2028}'\n".into(),
    };
    let rendered = render(&RenderInput {
        app: "tricky",
        namespace: "laoban-dev",
        files: &files,
        masked_prompt: "需求",
        masked_summary: "摘要",
        model: "model-a-test",
        generated_at: "2026-10-13T00:00:00Z",
    });
    let dir = temp_dir("yaml");
    for f in &rendered {
        let name = f.path.rsplit('/').next().unwrap();
        std::fs::write(dir.join(name), &f.content).unwrap();
    }
    let expected = serde_json::json!({
        "index.html": files.index_html, "app.js": files.app_js, "style.css": files.style_css,
        "server.py": SERVER_PY, "handler.py": files.handler_py, "main.py": MAIN_PY,
    });
    std::fs::write(dir.join("expected.json"), expected.to_string()).unwrap();
    std::fs::write(dir.join("check.py"), CHECK_YAML).unwrap();
    let out = Command::new("python3")
        .arg("-I")
        .arg(dir.join("check.py"))
        .arg(&dir)
        .arg(dir.join("expected.json"))
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
