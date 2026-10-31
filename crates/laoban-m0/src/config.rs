//! Configuration from environment variables (see the M0 design, 設定).
//! Errors name the variable, never its value.

use std::fmt;
use std::sync::OnceLock;

use regex::Regex;
use thiserror::Error;

use crate::trf;

pub const ENV_ANTHROPIC_API_KEY: &str = "ANTHROPIC_API_KEY";
pub const ENV_MODEL: &str = "LAOBAN_MODEL";
pub const ENV_TOKEN_CAP: &str = "LAOBAN_TOKEN_CAP";
pub const ENV_GITHUB_REPO: &str = "LAOBAN_GITHUB_REPO";
pub const ENV_GITHUB_TOKEN: &str = "LAOBAN_GITHUB_TOKEN";
pub const ENV_TARGET_NAMESPACE: &str = "LAOBAN_TARGET_NAMESPACE";
pub const ENV_CLUSTER_ID: &str = "LAOBAN_CLUSTER_ID";
/// Address the generated apps are opened at, with `{app}` in the host, for
/// example `https://{app}.apps.example.com/`. Each app then gets an Ingress
/// for that host. Unset: apps open through the Rancher service proxy, on
/// the Rancher origin (with the viewer's Rancher login; see SECURITY.md).
pub const ENV_APP_URL: &str = "LAOBAN_APP_URL";
/// `spec.ingressClassName` of the app Ingresses. Unset: the cluster default.
pub const ENV_APP_INGRESS_CLASS: &str = "LAOBAN_APP_INGRESS_CLASS";
/// TLS Secret (in the target namespace) for the app Ingresses, for example
/// a wildcard certificate. Only with an `https://` app URL.
pub const ENV_APP_TLS_SECRET: &str = "LAOBAN_APP_TLS_SECRET";
/// Base URL of an Anthropic Messages API compatible endpoint (for example a
/// local Ollama server). Unset means the Anthropic API itself.
pub const ENV_AI_BASE_URL: &str = "LAOBAN_AI_BASE_URL";
/// Upper bound for one AI call, in seconds.
pub const ENV_AI_TIMEOUT: &str = "LAOBAN_AI_TIMEOUT_SECONDS";
/// Lower limits for rounds of 意見收斂 (chart `rounds.limits`). Each may
/// only be lowered: a value above the built-in maximum is refused.
pub const ENV_ROUND_OPINION_CHARS: &str = "LAOBAN_ROUND_MAX_OPINION_CHARS";
pub const ENV_ROUND_REPLY_CHARS: &str = "LAOBAN_ROUND_MAX_REPLY_CHARS";
pub const ENV_ROUND_CHARS: &str = "LAOBAN_ROUND_MAX_CHARS";
pub const ENV_ROUND_OPINIONS: &str = "LAOBAN_ROUND_MAX_OPINIONS";
pub const ENV_ROUND_REPLIES: &str = "LAOBAN_ROUND_MAX_REPLIES";

pub const DEFAULT_TOKEN_CAP: u64 = 2_000_000;
pub const DEFAULT_TARGET_NAMESPACE: &str = "laoban-dev";
pub const DEFAULT_CLUSTER_ID: &str = "c-m-26kbxk5f";
pub const DEFAULT_AI_TIMEOUT_SECS: u64 = 300;
/// Accepted range for `LAOBAN_AI_TIMEOUT_SECONDS`.
pub const AI_TIMEOUT_RANGE: std::ops::RangeInclusive<u64> = 30..=1800;

/// Non-secret settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct M0Settings {
    pub model: String,
    pub token_cap: u64,
    pub repo_owner: String,
    pub repo_name: String,
    pub target_namespace: String,
    pub cluster_id: String,
    /// Upper bound for one AI call; the page waits a little longer.
    pub ai_timeout_secs: u64,
    /// Own address for each app (`LAOBAN_APP_URL`). `None`: the Rancher
    /// service proxy.
    pub app_url: Option<AppUrl>,
}

/// Where the generated apps are opened when they have their own host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppUrl {
    /// `http` or `https`.
    pub scheme: String,
    /// Host name containing `{app}` once, for example `{app}.apps.example.com`.
    pub host: String,
    /// Port written into the link only (`:8443`), empty for the default.
    /// The Ingress matches the host name alone.
    pub port: String,
    pub ingress_class: Option<String>,
    pub tls_secret: Option<String>,
}

impl AppUrl {
    /// Host name of one app.
    pub fn host_for(&self, app: &str) -> String {
        self.host.replace("{app}", app)
    }

    /// Link to one app.
    pub fn url_for(&self, app: &str) -> String {
        format!("{}://{}{}/", self.scheme, self.host_for(app), self.port)
    }
}

impl M0Settings {
    /// `<owner>/<repo>`
    pub fn repo(&self) -> String {
        format!("{}/{}", self.repo_owner, self.repo_name)
    }
}

/// A secret value. `Debug` never shows it.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

/// Where the AI calls go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiEndpoint {
    /// `None`: the Anthropic API. `Some`: an Anthropic-compatible endpoint
    /// set with `LAOBAN_AI_BASE_URL`.
    pub base_url: Option<String>,
}

#[derive(Debug)]
pub struct EnvConfig {
    pub settings: M0Settings,
    pub ai: AiEndpoint,
    /// Required for the Anthropic API; optional for a compatible endpoint
    /// (a local Ollama server does not check keys).
    pub anthropic_api_key: Option<Secret>,
    pub github_token: Secret,
    pub round_limits: crate::rounds::Limits,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConfigError {
    #[error("missing required environment variable(s): {}", .0.join(", "))]
    Missing(Vec<&'static str>),
    #[error("environment variable {0} has an invalid value")]
    Invalid(&'static str),
}

impl ConfigError {
    /// Message for the UI, in the configured language. Names variables only.
    pub fn user_message(&self) -> String {
        match self {
            ConfigError::Missing(vars) => crate::lang::pick(
                crate::lang::current(),
                format!(
                    "Backend configuration is incomplete: missing environment variables {}.",
                    vars.join(", ")
                ),
                format!("後端設定不完整：缺少環境變數 {}。", vars.join("、")),
            ),
            ConfigError::Invalid(var) => {
                trf!(en: "Backend configuration error: environment variable {var} has an invalid value.", zh: "後端設定錯誤：環境變數 {var} 的值不正確。")
            }
        }
    }
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

/// `http(s)://<host with {app} once>[:port][/]`. App names are DNS labels,
/// so the host stays a valid DNS name after `{app}` is replaced. No path:
/// the app's own files and its `api/` are served from the root.
fn parse_app_url(v: &str) -> Option<AppUrl> {
    static APP_URL: OnceLock<Regex> = OnceLock::new();
    let caps = re(
        &APP_URL,
        r"^(https?)://([a-z0-9.{}-]{1,200})(:[0-9]{1,5})?/?$",
    )
    .captures(v)?;
    let host = caps[2].to_string();
    if host.matches("{app}").count() != 1 {
        return None;
    }
    // Every label must be a DNS label once {app} is replaced by a name.
    let probe = host.replace("{app}", "a");
    if probe.contains(['{', '}'])
        || !probe
            .split('.')
            .all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-'))
    {
        return None;
    }
    let port = caps.get(3).map(|m| m.as_str()).unwrap_or("");
    if !port.is_empty() && !port[1..].parse::<u16>().is_ok_and(|p| p > 0) {
        return None;
    }
    Some(AppUrl {
        scheme: caps[1].to_string(),
        host,
        port: port.to_string(),
        ingress_class: None,
        tls_secret: None,
    })
}

/// Read configuration through `get` (use `|k| std::env::var(k).ok()`).
/// Empty values count as missing.
pub fn from_env_with(get: impl Fn(&str) -> Option<String>) -> Result<EnvConfig, ConfigError> {
    let value = |k: &str| {
        get(k)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };

    let base_url = value(ENV_AI_BASE_URL);
    // The key is only required when the calls go to the Anthropic API.
    let mut required = vec![];
    if base_url.is_none() {
        required.push(ENV_ANTHROPIC_API_KEY);
    }
    required.extend([ENV_MODEL, ENV_GITHUB_REPO, ENV_GITHUB_TOKEN]);
    let missing: Vec<&'static str> = required
        .iter()
        .copied()
        .filter(|k| value(k).is_none())
        .collect();
    if !missing.is_empty() {
        return Err(ConfigError::Missing(missing));
    }
    let key = value(ENV_ANTHROPIC_API_KEY);
    let model = value(ENV_MODEL).unwrap_or_default();
    let repo = value(ENV_GITHUB_REPO).unwrap_or_default();
    let token = value(ENV_GITHUB_TOKEN).unwrap_or_default();

    if let Some(key) = &key {
        if !key.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(ConfigError::Invalid(ENV_ANTHROPIC_API_KEY));
        }
    }
    if let Some(url) = &base_url {
        static BASE_URL: OnceLock<Regex> = OnceLock::new();
        // Scheme, host and optional port only: the client appends /v1/messages.
        if !re(&BASE_URL, r"^https?://[A-Za-z0-9.-]+(:[0-9]{1,5})?/?$").is_match(url) {
            return Err(ConfigError::Invalid(ENV_AI_BASE_URL));
        }
    }
    let ai_timeout_secs = match value(ENV_AI_TIMEOUT) {
        None => DEFAULT_AI_TIMEOUT_SECS,
        Some(v) => v
            .parse::<u64>()
            .ok()
            .filter(|n| AI_TIMEOUT_RANGE.contains(n))
            .ok_or(ConfigError::Invalid(ENV_AI_TIMEOUT))?,
    };
    if !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(ConfigError::Invalid(ENV_GITHUB_TOKEN));
    }
    if !model.bytes().all(|b| b.is_ascii_graphic()) || model.len() > 200 {
        return Err(ConfigError::Invalid(ENV_MODEL));
    }

    static REPO: OnceLock<Regex> = OnceLock::new();
    let caps = re(
        &REPO,
        r"^([A-Za-z0-9](?:[A-Za-z0-9-]{0,38}))/([A-Za-z0-9._-]{1,100})$",
    )
    .captures(&repo)
    .filter(|c| &c[2] != "." && &c[2] != "..")
    .ok_or(ConfigError::Invalid(ENV_GITHUB_REPO))?;
    let (repo_owner, repo_name) = (caps[1].to_string(), caps[2].to_string());

    let token_cap = match value(ENV_TOKEN_CAP) {
        None => DEFAULT_TOKEN_CAP,
        Some(v) => v
            .replace(['_', ','], "")
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or(ConfigError::Invalid(ENV_TOKEN_CAP))?,
    };

    static DNS_LABEL: OnceLock<Regex> = OnceLock::new();
    let target_namespace =
        value(ENV_TARGET_NAMESPACE).unwrap_or_else(|| DEFAULT_TARGET_NAMESPACE.into());
    if !re(&DNS_LABEL, r"^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$").is_match(&target_namespace) {
        return Err(ConfigError::Invalid(ENV_TARGET_NAMESPACE));
    }
    let cluster_id = value(ENV_CLUSTER_ID).unwrap_or_else(|| DEFAULT_CLUSTER_ID.into());
    if !re(&DNS_LABEL, r"^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$").is_match(&cluster_id) {
        return Err(ConfigError::Invalid(ENV_CLUSTER_ID));
    }

    let app_url = match value(ENV_APP_URL) {
        None => None,
        Some(v) => Some(parse_app_url(&v).ok_or(ConfigError::Invalid(ENV_APP_URL))?),
    };
    let ingress_class = value(ENV_APP_INGRESS_CLASS);
    let tls_secret = value(ENV_APP_TLS_SECRET);
    for (var, v) in [
        (ENV_APP_INGRESS_CLASS, &ingress_class),
        (ENV_APP_TLS_SECRET, &tls_secret),
    ] {
        static DNS_SUBDOMAIN: OnceLock<Regex> = OnceLock::new();
        if let Some(v) = v {
            let ok = app_url.is_some()
                && v.len() <= 253
                && re(
                    &DNS_SUBDOMAIN,
                    r"^[a-z0-9]([-a-z0-9]*[a-z0-9])?(\.[a-z0-9]([-a-z0-9]*[a-z0-9])?)*$",
                )
                .is_match(v);
            if !ok {
                return Err(ConfigError::Invalid(var));
            }
        }
    }
    if tls_secret.is_some() && app_url.as_ref().is_some_and(|u| u.scheme != "https") {
        return Err(ConfigError::Invalid(ENV_APP_TLS_SECRET));
    }
    let app_url = app_url.map(|u| AppUrl {
        ingress_class,
        tls_secret,
        ..u
    });

    let mut round_limits = crate::rounds::Limits::default();
    for (var, slot) in [
        (ENV_ROUND_OPINION_CHARS, &mut round_limits.opinion_chars),
        (ENV_ROUND_REPLY_CHARS, &mut round_limits.reply_chars),
        (ENV_ROUND_CHARS, &mut round_limits.round_chars),
        (ENV_ROUND_OPINIONS, &mut round_limits.opinions),
        (ENV_ROUND_REPLIES, &mut round_limits.replies),
    ] {
        if let Some(v) = value(var) {
            let max = *slot;
            *slot = v
                .parse::<usize>()
                .ok()
                .filter(|n| (1..=max).contains(n))
                .ok_or(ConfigError::Invalid(var))?;
        }
    }

    Ok(EnvConfig {
        settings: M0Settings {
            model,
            token_cap,
            repo_owner,
            repo_name,
            target_namespace,
            cluster_id,
            ai_timeout_secs,
            app_url,
        },
        ai: AiEndpoint { base_url },
        anthropic_api_key: key.map(Secret),
        github_token: Secret(token),
        round_limits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    const FULL: &[(&str, &str)] = &[
        (ENV_ANTHROPIC_API_KEY, "sk-test-secret-value"),
        (ENV_MODEL, "model-a-test"),
        (ENV_GITHUB_REPO, "example-owner/laoban-apps"),
        (ENV_GITHUB_TOKEN, "github_pat_secret_value"),
    ];

    #[test]
    fn defaults() {
        let c = from_env_with(env(FULL)).unwrap();
        assert_eq!(c.settings.token_cap, DEFAULT_TOKEN_CAP);
        assert_eq!(c.settings.target_namespace, "laoban-dev");
        assert_eq!(c.settings.cluster_id, "c-m-26kbxk5f");
        assert_eq!(c.settings.repo(), "example-owner/laoban-apps");
        assert_eq!(c.ai.base_url, None);
        assert_eq!(c.settings.ai_timeout_secs, DEFAULT_AI_TIMEOUT_SECS);
        assert!(c.anthropic_api_key.is_some());
        let dbg = format!("{c:?}");
        assert!(
            !dbg.contains("secret_value") && !dbg.contains("secret-value"),
            "{dbg}"
        );
    }

    #[test]
    fn missing_names_variables_not_values() {
        let err = from_env_with(env(&[(ENV_MODEL, "m"), (ENV_GITHUB_TOKEN, "  ")])).unwrap_err();
        assert_eq!(
            err,
            ConfigError::Missing(vec![
                ENV_ANTHROPIC_API_KEY,
                ENV_GITHUB_REPO,
                ENV_GITHUB_TOKEN
            ])
        );
        assert!(err.user_message().contains("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn invalid_values() {
        for (k, v) in [
            (ENV_GITHUB_REPO, "no-slash"),
            (ENV_GITHUB_REPO, "a/b/c"),
            (ENV_GITHUB_REPO, "owner/.."),
            (ENV_TOKEN_CAP, "lots"),
            (ENV_TOKEN_CAP, "0"),
            (ENV_TARGET_NAMESPACE, "Bad_NS"),
            (ENV_CLUSTER_ID, "c/m"),
            (ENV_AI_BASE_URL, "ftp://192.0.2.1:11434"),
            (ENV_AI_BASE_URL, "http://192.0.2.1:11434/v1/messages"),
            (ENV_AI_BASE_URL, "http://user@192.0.2.1"),
            (ENV_AI_TIMEOUT, "10"),
            (ENV_AI_TIMEOUT, "3600"),
            (ENV_AI_TIMEOUT, "soon"),
            (ENV_ROUND_OPINION_CHARS, "1001"),
            (ENV_ROUND_REPLY_CHARS, "0"),
            (ENV_ROUND_CHARS, "8001"),
            (ENV_ROUND_OPINIONS, "many"),
            (ENV_ROUND_REPLIES, "21"),
            (ENV_APP_URL, "https://apps.example.com/"),
            (ENV_APP_URL, "https://{app}.{app}.example.com/"),
            (ENV_APP_URL, "https://{app}.example.com/path/"),
            (ENV_APP_URL, "ftp://{app}.example.com/"),
            (ENV_APP_URL, "https://{app}..example.com/"),
            (ENV_APP_URL, "https://{app}.Example.com/"),
            (ENV_APP_URL, "https://user@{app}.example.com/"),
            (ENV_APP_URL, "https://{app}.example.com:0/"),
            (ENV_APP_URL, "https://{app}.example.com:70000/"),
            (ENV_APP_INGRESS_CLASS, "traefik"),
            (ENV_APP_TLS_SECRET, "apps-tls"),
        ] {
            let mut pairs = FULL.to_vec();
            pairs.push((k, v));
            assert_eq!(
                from_env_with(env(&pairs)).unwrap_err(),
                ConfigError::Invalid(k),
                "{k}={v}"
            );
        }
        let mut pairs = FULL.to_vec();
        pairs.push((ENV_TOKEN_CAP, "1_000"));
        assert_eq!(from_env_with(env(&pairs)).unwrap().settings.token_cap, 1000);
    }

    #[test]
    fn app_url() {
        assert_eq!(from_env_with(env(FULL)).unwrap().settings.app_url, None);
        let mut pairs = FULL.to_vec();
        pairs.extend([
            (ENV_APP_URL, "http://{app}.apps.localhost:8480"),
            (ENV_APP_INGRESS_CLASS, "traefik"),
        ]);
        let u = from_env_with(env(&pairs))
            .unwrap()
            .settings
            .app_url
            .unwrap();
        assert_eq!(u.url_for("todo"), "http://todo.apps.localhost:8480/");
        assert_eq!(u.host_for("todo"), "todo.apps.localhost");
        assert_eq!(u.ingress_class.as_deref(), Some("traefik"));
        assert_eq!(u.tls_secret, None);

        // TLS needs https.
        pairs.push((ENV_APP_TLS_SECRET, "apps-tls"));
        assert_eq!(
            from_env_with(env(&pairs)).unwrap_err(),
            ConfigError::Invalid(ENV_APP_TLS_SECRET)
        );
        let mut pairs = FULL.to_vec();
        pairs.extend([
            (ENV_APP_URL, "https://laoban-{app}.apps.example.com/"),
            (ENV_APP_TLS_SECRET, "apps-tls"),
        ]);
        let u = from_env_with(env(&pairs))
            .unwrap()
            .settings
            .app_url
            .unwrap();
        assert_eq!(u.url_for("todo"), "https://laoban-todo.apps.example.com/");
        assert_eq!(u.tls_secret.as_deref(), Some("apps-tls"));
    }

    #[test]
    fn round_limits_can_only_be_lowered() {
        let c = from_env_with(env(FULL)).unwrap();
        assert_eq!(c.round_limits, crate::rounds::Limits::default());
        let mut pairs = FULL.to_vec();
        pairs.extend([
            (ENV_ROUND_OPINION_CHARS, "500"),
            (ENV_ROUND_REPLY_CHARS, "300"),
            (ENV_ROUND_CHARS, "4000"),
            (ENV_ROUND_OPINIONS, "10"),
            (ENV_ROUND_REPLIES, "5"),
        ]);
        let l = from_env_with(env(&pairs)).unwrap().round_limits;
        assert_eq!(
            (
                l.opinion_chars,
                l.reply_chars,
                l.round_chars,
                l.opinions,
                l.replies,
                l.batch
            ),
            (500, 300, 4000, 10, 5, 10)
        );
        assert!(l.within_maximum());
    }

    #[test]
    fn compatible_endpoint_makes_the_key_optional() {
        let pairs = [
            (ENV_AI_BASE_URL, "http://192.0.2.1:11434"),
            (ENV_AI_TIMEOUT, "900"),
            (ENV_MODEL, "local-model:30b"),
            (ENV_GITHUB_REPO, "example-owner/laoban-apps"),
            (ENV_GITHUB_TOKEN, "github_pat_secret_value"),
        ];
        let c = from_env_with(env(&pairs)).unwrap();
        assert_eq!(c.ai.base_url.as_deref(), Some("http://192.0.2.1:11434"));
        assert_eq!(c.settings.ai_timeout_secs, 900);
        assert!(c.anthropic_api_key.is_none());

        // A key, if given, is still passed on.
        let mut with_key = pairs.to_vec();
        with_key.push((ENV_ANTHROPIC_API_KEY, "sk-test-secret-value"));
        assert!(from_env_with(env(&with_key))
            .unwrap()
            .anthropic_api_key
            .is_some());
    }

    #[test]
    fn key_still_required_without_base_url() {
        let pairs = [
            (ENV_MODEL, "m"),
            (ENV_GITHUB_REPO, "example-owner/laoban-apps"),
            (ENV_GITHUB_TOKEN, "t"),
        ];
        assert_eq!(
            from_env_with(env(&pairs)).unwrap_err(),
            ConfigError::Missing(vec![ENV_ANTHROPIC_API_KEY])
        );
    }
}
