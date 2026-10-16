//! `laoban-server` binary.
//!
//! Listens on `LAOBAN_LISTEN_ADDR` (default `0.0.0.0:8080`) for the probes
//! and, when `LAOBAN_TLS_CERT_FILE` and `LAOBAN_TLS_KEY_FILE` are set, on
//! `LAOBAN_TLS_LISTEN_ADDR` (default `0.0.0.0:8443`) for the API as a
//! Kubernetes aggregated API (see `laoban_server::identity`). Stops
//! cleanly on SIGTERM or SIGINT. Handling SIGTERM explicitly matters in a
//! container: as PID 1 a process without a handler ignores SIGTERM, so
//! Kubernetes would have to wait for the grace period and send SIGKILL.

use std::process::ExitCode;

use std::sync::Arc;
use std::time::Duration;

use laoban_ai::anthropic::{AnthropicConfig, AnthropicProvider, ApiKey, DEFAULT_BASE_URL};
use laoban_m0::{GithubCommitter, GithubConfig, GithubToken, M0Service};
use laoban_server::{git_sha, ApiState, Server, NAME, VERSION};
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:8080";
const DEFAULT_TLS_LISTEN_ADDR: &str = "0.0.0.0:8443";

/// Build the M0 API from the environment, plus a status line for the log.
/// Problems are reported by variable name only and leave the probes
/// working.
fn api_from_env() -> (ApiState, String) {
    let config = match laoban_m0::from_env_with(|k| std::env::var(k).ok()) {
        Ok(c) => c,
        Err(e) => {
            return (
                ApiState::Unconfigured(e.user_message()),
                format!("/api disabled: {e}"),
            )
        }
    };
    let invalid = |var: &str| {
        (
            ApiState::Unconfigured(format!("後端設定錯誤：環境變數 {var} 的值不正確。")),
            format!("/api disabled: environment variable {var} has an invalid value"),
        )
    };
    let ai_config = AnthropicConfig {
        base_url: config
            .ai
            .base_url
            .clone()
            .unwrap_or_else(|| DEFAULT_BASE_URL.into()),
        request_timeout: Duration::from_secs(config.settings.ai_timeout_secs),
        ..AnthropicConfig::default()
    };
    let provider = match &config.anthropic_api_key {
        Some(key) => ApiKey::new(key.expose()).and_then(|k| AnthropicProvider::new(k, ai_config)),
        None => AnthropicProvider::without_key(ai_config),
    };
    let provider = match provider {
        Ok(p) => p,
        Err(_) => return invalid("ANTHROPIC_API_KEY"),
    };
    let token = match GithubToken::new(config.github_token.expose()) {
        Ok(t) => t,
        Err(_) => return invalid("LAOBAN_GITHUB_TOKEN"),
    };
    let s = config.settings;
    let github = GithubCommitter::new(GithubConfig::new(&s.repo_owner, &s.repo_name), token);
    let status = format!(
        "/api enabled: model {} at {}, repo {}, namespace {}",
        s.model,
        config.ai.base_url.as_deref().unwrap_or(DEFAULT_BASE_URL),
        s.repo(),
        s.target_namespace
    );
    let round_limits = config.round_limits;
    match M0Service::new(s, Box::new(provider), github) {
        Ok(mut service) => {
            service.set_round_limits(round_limits);
            // Rounds are kept in Round resources in the backend's own
            // namespace. Outside a cluster (local runs) they stay in memory.
            let store_status = if std::env::var("KUBERNETES_SERVICE_HOST").is_ok() {
                match laoban_server::kube::KubeStore::in_cluster() {
                    Ok(store) => {
                        let line = format!("rounds stored in namespace {}", store.namespace());
                        service.use_state_store(Arc::new(store));
                        line
                    }
                    Err(e) => {
                        service.use_state_store(Arc::new(laoban_server::kube::UnavailableStore(
                            e.clone(),
                        )));
                        format!("rounds unavailable: {e}")
                    }
                }
            } else {
                "rounds kept in memory (not in a cluster)".to_string()
            };
            let status = format!("{status}; {store_status}");
            let service = Arc::new(service);
            // Rounds of 意見收斂 converge at their deadline.
            service.start_round_timer();
            (ApiState::Ready(service), status)
        }
        Err(e) => (
            ApiState::Unconfigured("後端初始化失敗。".into()),
            format!("/api disabled: {e}"),
        ),
    }
}

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        None => {}
        Some("--version" | "-V") => {
            println!("{NAME} {VERSION} ({})", git_sha());
            return ExitCode::SUCCESS;
        }
        Some("--help" | "-h") => {
            println!(
                "{NAME} {VERSION}\n\nServes GET /healthz, GET /version, POST /api/plan, \
                 POST /api/apply and GET /api/config.\n\n\
                 Environment:\n  LAOBAN_LISTEN_ADDR       probes (default {DEFAULT_LISTEN_ADDR})\n  \
                 LAOBAN_TLS_LISTEN_ADDR   aggregated API (default {DEFAULT_TLS_LISTEN_ADDR})\n  \
                 LAOBAN_TLS_CERT_FILE     server certificate (PEM) for the API port\n  \
                 LAOBAN_TLS_KEY_FILE      its key (PEM)\n  \
                 ANTHROPIC_API_KEY        Anthropic API key (required for /api unless\n                           LAOBAN_AI_BASE_URL is set)\n  \
                 LAOBAN_MODEL             model ID (required for /api)\n  \
                 LAOBAN_AI_BASE_URL       Anthropic-compatible endpoint, e.g. a local\n                           Ollama server (default the Anthropic API)\n  \
                 LAOBAN_AI_TIMEOUT_SECONDS  limit for one AI call (default 300)\n  \
                 LAOBAN_TOKEN_CAP         monthly token cap (default 2000000)\n  \
                 LAOBAN_GITHUB_REPO       <owner>/<repo> (required for /api)\n  \
                 LAOBAN_GITHUB_TOKEN      GitHub token (required for /api)\n  \
                 LAOBAN_TARGET_NAMESPACE  default laoban-dev\n  \
                 LAOBAN_CLUSTER_ID        default c-m-26kbxk5f\n  \
                 LAOBAN_STATE_NAMESPACE   where rounds are stored (default the pod's)\n  \
                 LAOBAN_ROUND_MAX_*       lower the size limits of rounds\n\n\
                 Without the required variables /healthz and /version still work and \
                 the API answers 500 naming the missing variables. Without the TLS \
                 variables only /healthz and /version are served."
            );
            return ExitCode::SUCCESS;
        }
        Some(other) => {
            eprintln!("{NAME}: unknown argument {other:?} (try --help)");
            return ExitCode::from(2);
        }
    }

    let addr = std::env::var("LAOBAN_LISTEN_ADDR").unwrap_or_else(|_| DEFAULT_LISTEN_ADDR.into());
    let (api, api_status) = api_from_env();
    let server = match Server::bind_with_api(addr.as_str(), api) {
        Ok(server) => server,
        Err(e) => {
            eprintln!("{NAME}: cannot listen on {addr}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let tls_status;
    let server = match (
        std::env::var("LAOBAN_TLS_CERT_FILE"),
        std::env::var("LAOBAN_TLS_KEY_FILE"),
    ) {
        (Ok(cert_file), Ok(key_file)) => {
            let tls_addr = std::env::var("LAOBAN_TLS_LISTEN_ADDR")
                .unwrap_or_else(|_| DEFAULT_TLS_LISTEN_ADDR.into());
            let (cert, key) = match (std::fs::read(&cert_file), std::fs::read(&key_file)) {
                (Ok(c), Ok(k)) => (c, k),
                _ => {
                    eprintln!("{NAME}: cannot read LAOBAN_TLS_CERT_FILE or LAOBAN_TLS_KEY_FILE");
                    return ExitCode::FAILURE;
                }
            };
            let auth = match laoban_server::kube::load_auth_config() {
                Ok(a) => a,
                Err(e) => {
                    eprintln!(
                        "{NAME}: cannot read kube-system/extension-apiserver-authentication: {e}"
                    );
                    return ExitCode::FAILURE;
                }
            };
            let names = if auth.allowed_names.is_empty() {
                "any".to_string()
            } else {
                auth.allowed_names.join(",")
            };
            match server.with_tls(tls_addr.as_str(), &cert, &key, auth) {
                Ok(s) => {
                    tls_status = format!(
                        "aggregated API on {} (proxy client names: {names})",
                        s.tls_addr().map(|a| a.to_string()).unwrap_or_default()
                    );
                    s
                }
                Err(e) => {
                    eprintln!("{NAME}: cannot serve TLS on {tls_addr}: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
        _ => {
            tls_status =
                "LAOBAN_TLS_CERT_FILE/LAOBAN_TLS_KEY_FILE not set: API disabled, probes only"
                    .to_string();
            server
        }
    };

    let mut signals = match Signals::new([SIGTERM, SIGINT]) {
        Ok(signals) => signals,
        Err(e) => {
            eprintln!("{NAME}: cannot install signal handlers: {e}");
            return ExitCode::FAILURE;
        }
    };
    let handle = server.shutdown_handle();
    std::thread::spawn(move || {
        if let Some(sig) = signals.forever().next() {
            eprintln!("{NAME}: received signal {sig}, shutting down");
            handle.shutdown();
        }
    });

    eprintln!(
        "{NAME} {VERSION} ({}) listening on {}",
        git_sha(),
        server.local_addr()
    );
    // After the "listening on" line, which tools and tests read first.
    eprintln!("{NAME}: {api_status}");
    eprintln!("{NAME}: {tls_status}");
    match server.run() {
        Ok(()) => {
            eprintln!("{NAME}: stopped");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{NAME}: {e}");
            ExitCode::FAILURE
        }
    }
}
