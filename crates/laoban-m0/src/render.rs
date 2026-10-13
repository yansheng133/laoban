//! Fixed templates for the generated application. Nothing here comes from
//! the AI except the four file contents placed into the ConfigMaps and the
//! (masked) text in the README. Output is deterministic for the same input.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use sha2::{Digest, Sha256};

use crate::codegen::GeneratedFiles;
use crate::github::CommitFile;

/// Fixed web server template.
pub const SERVER_PY: &str = include_str!("../templates/server.py");
/// Fixed API server template.
pub const MAIN_PY: &str = include_str!("../templates/main.py");
/// Runtime image. Referenced by tag: the digest could not be looked up
/// from the build environment (see the crate README).
pub const IMAGE: &str = "registry.suse.com/bci/python:3.13";
/// Port of both containers and Services.
pub const PORT: u16 = 8080;

/// Everything the templates need.
#[derive(Debug, Clone)]
pub struct RenderInput<'a> {
    pub app: &'a str,
    pub namespace: &'a str,
    pub files: &'a GeneratedFiles,
    /// The user's requirement, already masked.
    pub masked_prompt: &'a str,
    /// The plan summary, already masked.
    pub masked_summary: &'a str,
    pub model: &'a str,
    /// RFC 3339 UTC timestamp.
    pub generated_at: &'a str,
    /// Content of `laoban.json` (see [`crate::meta`]).
    pub meta_json: &'a str,
}

/// sha256 over the ConfigMap data: for each key in sorted order,
/// `key 0x00 value 0x00`. Lowercase hex.
pub fn content_hash(data: &BTreeMap<&str, &str>) -> String {
    let mut h = Sha256::new();
    for (k, v) in data {
        h.update(k.as_bytes());
        h.update([0u8]);
        h.update(v.as_bytes());
        h.update([0u8]);
    }
    h.finalize()
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// YAML double-quoted scalar. Escapes everything a YAML 1.1 or 1.2 parser
/// could read as structure or a line break.
pub fn yaml_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{feff}') => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Whether `s` can be written as a literal block scalar (`|`) and read
/// back byte for byte: ends with exactly one newline, no carriage
/// returns or other control characters, no YAML 1.1 line separators, and
/// a first line that does not start with whitespace (which would change
/// the detected indentation).
fn block_safe(s: &str) -> bool {
    s.ends_with('\n')
        && !s.ends_with("\n\n")
        && !s.starts_with([' ', '\t', '\n'])
        && s.chars().all(|c| {
            (c == '\n' || c == '\t' || !c.is_control())
                && !matches!(c, '\u{2028}' | '\u{2029}' | '\u{feff}')
        })
}

/// `key: value` at `indent`, using a literal block when it round-trips
/// exactly and a double-quoted scalar otherwise.
fn yaml_entry(out: &mut String, indent: usize, key: &str, value: &str) {
    let pad = " ".repeat(indent);
    if block_safe(value) {
        let _ = writeln!(out, "{pad}{key}: |");
        for line in value.lines() {
            if line.is_empty() {
                out.push('\n');
            } else {
                let _ = writeln!(out, "{pad}  {line}");
            }
        }
    } else {
        let _ = writeln!(out, "{pad}{key}: {}", yaml_quote(value));
    }
}

/// Where a label set is written.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LabelPlace {
    /// `metadata.labels` of a ConfigMap, Deployment or Service. Fleet deploys
    /// through Helm, and Helm sets `app.kubernetes.io/managed-by: Helm` on
    /// these, so writing our own value there makes Fleet see the objects as
    /// modified forever and the bundle never becomes Ready.
    Object,
    /// The Deployment's pod template. Helm leaves it alone, so the pods carry
    /// `app.kubernetes.io/managed-by: laoban` (used to find laoban's pods).
    Pod,
}

fn labels(
    out: &mut String,
    indent: usize,
    place: LabelPlace,
    app: &str,
    name: &str,
    component: &str,
) {
    let pad = " ".repeat(indent);
    if place == LabelPlace::Pod {
        let _ = writeln!(out, "{pad}app.kubernetes.io/managed-by: \"laoban\"");
    }
    let _ = writeln!(out, "{pad}laoban.io/app: {}", yaml_quote(app));
    let _ = writeln!(out, "{pad}app.kubernetes.io/name: {}", yaml_quote(name));
    let _ = writeln!(
        out,
        "{pad}app.kubernetes.io/component: {}",
        yaml_quote(component)
    );
}

fn configmap(app: &str, component: &str, name: &str, data: &BTreeMap<&str, &str>) -> String {
    let mut out = String::from(
        "# Generated by laoban. Do not edit by hand.\napiVersion: v1\nkind: ConfigMap\nmetadata:\n",
    );
    let _ = writeln!(out, "  name: {}", yaml_quote(name));
    out.push_str("  labels:\n");
    labels(
        &mut out,
        4,
        LabelPlace::Object,
        app,
        &format!("{app}-{component}"),
        component,
    );
    out.push_str("data:\n");
    for (k, v) in data {
        yaml_entry(&mut out, 2, k, v);
    }
    out
}

struct Workload<'a> {
    app: &'a str,
    component: &'a str,
    script: &'a str,
    configmap: &'a str,
    hash: &'a str,
    extra_env: &'a [(&'a str, String)],
}

fn workload(w: &Workload<'_>) -> String {
    let name = format!("{}-{}", w.app, w.component);
    let mut out = String::from("# Generated by laoban. Do not edit by hand.\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n");
    let _ = writeln!(out, "  name: {}", yaml_quote(&name));
    out.push_str("  labels:\n");
    labels(&mut out, 4, LabelPlace::Object, w.app, &name, w.component);
    out.push_str("spec:\n  replicas: 1\n  selector:\n    matchLabels:\n");
    let _ = writeln!(out, "      app.kubernetes.io/name: {}", yaml_quote(&name));
    let _ = writeln!(out, "      laoban.io/app: {}", yaml_quote(w.app));
    out.push_str("  template:\n    metadata:\n      labels:\n");
    labels(&mut out, 8, LabelPlace::Pod, w.app, &name, w.component);
    out.push_str("      annotations:\n");
    let _ = writeln!(
        out,
        "        laoban.io/content-hash: {}",
        yaml_quote(w.hash)
    );
    out.push_str(
        "    spec:
      automountServiceAccountToken: false
      securityContext:
        runAsNonRoot: true
        runAsUser: 65532
        seccompProfile:
          type: RuntimeDefault
      containers:
",
    );
    let _ = writeln!(out, "        - name: {}", yaml_quote(w.component));
    let _ = writeln!(out, "          image: {}", yaml_quote(IMAGE));
    out.push_str("          imagePullPolicy: IfNotPresent\n");
    let _ = writeln!(
        out,
        "          command: [\"python3\", {}]",
        yaml_quote(&format!("/app/{}", w.script))
    );
    out.push_str("          env:\n");
    let mut env: Vec<(&str, String)> = vec![
        ("PORT", PORT.to_string()),
        ("PYTHONDONTWRITEBYTECODE", "1".into()),
        ("PYTHONUNBUFFERED", "1".into()),
    ];
    env.extend(w.extra_env.iter().cloned());
    for (k, v) in env {
        let _ = writeln!(
            out,
            "            - name: {k}\n              value: {}",
            yaml_quote(&v)
        );
    }
    let _ = write!(
        out,
        "          ports:
            - name: http
              containerPort: {PORT}
              protocol: TCP
          readinessProbe:
            tcpSocket:
              port: http
            periodSeconds: 5
          livenessProbe:
            tcpSocket:
              port: http
            initialDelaySeconds: 10
            periodSeconds: 20
          securityContext:
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            runAsNonRoot: true
            runAsUser: 65532
            capabilities:
              drop: [\"ALL\"]
            seccompProfile:
              type: RuntimeDefault
          resources:
            requests:
              cpu: 50m
              memory: 64Mi
            limits:
              cpu: 200m
              memory: 128Mi
          volumeMounts:
            - name: app
              mountPath: /app
              readOnly: true
            - name: tmp
              mountPath: /tmp
      volumes:
        - name: app
          configMap:
            name: {}
        - name: tmp
          emptyDir:
            sizeLimit: 16Mi
---
apiVersion: v1
kind: Service
metadata:
",
        yaml_quote(w.configmap)
    );
    let _ = writeln!(out, "  name: {}", yaml_quote(&name));
    out.push_str("  labels:\n");
    labels(&mut out, 4, LabelPlace::Object, w.app, &name, w.component);
    out.push_str("spec:\n  type: ClusterIP\n  selector:\n");
    let _ = writeln!(out, "    app.kubernetes.io/name: {}", yaml_quote(&name));
    let _ = writeln!(out, "    laoban.io/app: {}", yaml_quote(w.app));
    let _ = write!(
        out,
        "  ports:
    - name: http
      port: {PORT}
      targetPort: http
      protocol: TCP
"
    );
    out
}

fn quote_block(text: &str) -> String {
    let mut out = String::new();
    for line in text.trim_end().lines() {
        if line.trim().is_empty() {
            out.push_str(">\n");
        } else {
            let _ = writeln!(out, "> {line}");
        }
    }
    out
}

fn readme(input: &RenderInput<'_>) -> String {
    format!(
        "# {app}\n\n此目錄由 laoban 產生，請勿手動修改；要變更請在 laoban 頁面的應用清單按「編輯」。\n\n\
## 這次的需求（已遮蔽）\n\n{prompt}\n\
## 計畫摘要\n\n{summary}\n\
## 產生資訊\n\n\
- 產生時間：{at}\n\
- 模型：`{model}`\n\
- 部署目標 namespace：`{ns}`\n\
- 映像：`{IMAGE}`\n",
        app = input.app,
        prompt = quote_block(input.masked_prompt),
        summary = quote_block(input.masked_summary),
        at = input.generated_at,
        model = input.model,
        ns = input.namespace,
    )
}

/// Render every file under `apps/<app>/`.
pub fn render(input: &RenderInput<'_>) -> Vec<CommitFile> {
    let app = input.app;
    let f = input.files;
    let web_cm = format!("{app}-web-files");
    let api_cm = format!("{app}-api-code");

    let web_data: BTreeMap<&str, &str> = [
        ("index.html", f.index_html.as_str()),
        ("app.js", f.app_js.as_str()),
        ("style.css", f.style_css.as_str()),
        ("server.py", SERVER_PY),
    ]
    .into_iter()
    .collect();
    let api_data: BTreeMap<&str, &str> =
        [("handler.py", f.handler_py.as_str()), ("main.py", MAIN_PY)]
            .into_iter()
            .collect();
    let web_hash = content_hash(&web_data);
    let api_hash = content_hash(&api_data);

    let fleet = format!(
        "# Generated by laoban. Do not edit by hand.\ndefaultNamespace: {ns}\nnamespace: {ns}\n",
        ns = yaml_quote(input.namespace)
    );
    let web = workload(&Workload {
        app,
        component: "web",
        script: "server.py",
        configmap: &web_cm,
        hash: &web_hash,
        extra_env: &[("LAOBAN_API_URL", format!("http://{app}-api:{PORT}/"))],
    });
    let api = workload(&Workload {
        app,
        component: "api",
        script: "main.py",
        configmap: &api_cm,
        hash: &api_hash,
        extra_env: &[],
    });

    let dir = format!("apps/{app}");
    vec![
        CommitFile {
            path: format!("{dir}/fleet.yaml"),
            content: fleet,
        },
        CommitFile {
            path: format!("{dir}/configmap-web.yaml"),
            content: configmap(app, "web", &web_cm, &web_data),
        },
        CommitFile {
            path: format!("{dir}/configmap-api.yaml"),
            content: configmap(app, "api", &api_cm, &api_data),
        },
        CommitFile {
            path: format!("{dir}/web.yaml"),
            content: web,
        },
        CommitFile {
            path: format!("{dir}/api.yaml"),
            content: api,
        },
        CommitFile {
            path: format!("{dir}/README.md"),
            content: readme(input),
        },
        CommitFile {
            path: format!("{dir}/{}", crate::meta::META_FILE),
            content: input.meta_json.to_string(),
        },
        CommitFile {
            path: format!("{dir}/.fleetignore"),
            content: crate::meta::FLEETIGNORE.to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(yaml_quote("a\"b\\c\nd"), r#""a\"b\\c\nd""#);
        assert_eq!(yaml_quote("\u{2028}\u{7}"), "\"\\u2028\\u0007\"");
        assert_eq!(yaml_quote("yes"), "\"yes\"");
    }

    #[test]
    fn block_safety() {
        assert!(block_safe("a\n  b\n\nc\n"));
        for bad in [
            "a",
            "a\n\n",
            " a\n",
            "\na\n",
            "a\r\n",
            "a\u{2028}b\n",
            "\u{feff}a\n",
        ] {
            assert!(!block_safe(bad), "{bad:?}");
        }
    }

    #[test]
    fn hash_is_stable_and_order_independent() {
        let a: BTreeMap<&str, &str> = [("b", "2"), ("a", "1")].into_iter().collect();
        let b: BTreeMap<&str, &str> = [("a", "1"), ("b", "2")].into_iter().collect();
        assert_eq!(content_hash(&a), content_hash(&b));
        assert_eq!(content_hash(&a).len(), 64);
        let c: BTreeMap<&str, &str> = [("a", "1"), ("b", "3")].into_iter().collect();
        assert_ne!(content_hash(&a), content_hash(&c));
        // Key/value boundaries matter.
        let d: BTreeMap<&str, &str> = [("a1", "")].into_iter().collect();
        let e: BTreeMap<&str, &str> = [("a", "1")].into_iter().collect();
        assert_ne!(content_hash(&d), content_hash(&e));
    }

    #[test]
    fn managed_by_only_on_pods() {
        let mut object = String::new();
        labels(&mut object, 4, LabelPlace::Object, "a", "a-web", "web");
        assert!(!object.contains("managed-by"), "{object}");
        assert!(object.contains("laoban.io/app: \"a\""), "{object}");
        let mut pod = String::new();
        labels(&mut pod, 8, LabelPlace::Pod, "a", "a-web", "web");
        assert!(
            pod.contains("app.kubernetes.io/managed-by: \"laoban\""),
            "{pod}"
        );
    }
}
