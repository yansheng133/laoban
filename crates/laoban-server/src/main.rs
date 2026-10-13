//! `laoban-server` binary.
//!
//! Listens on `LAOBAN_LISTEN_ADDR` (default `0.0.0.0:8080`) and stops
//! cleanly on SIGTERM or SIGINT. Handling SIGTERM explicitly matters in a
//! container: as PID 1 a process without a handler ignores SIGTERM, so
//! Kubernetes would have to wait for the grace period and send SIGKILL.

use std::process::ExitCode;

use std::sync::Arc;

use laoban_ai::anthropic::{AnthropicConfig, AnthropicProvider, ApiKey};
use laoban_m0::{GithubCommitter, GithubConfig, GithubToken, M0Service};
use laoban_server::{git_sha, ApiState, Server, NAME, VERSION};
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:8080";

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
    let provider = match ApiKey::new(config.anthropic_api_key.expose())
        .and_then(|key| AnthropicProvider::new(key, AnthropicConfig::default()))
    {
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
        "/api enabled: model {}, repo {}, namespace {}",
        s.model,
        s.repo(),
        s.target_namespace
    );
    match M0Service::new(s, Box::new(provider), github) {
        Ok(service) => (ApiState::Ready(Arc::new(service)), status),
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
                 Environment:\n  LAOBAN_LISTEN_ADDR       listen address (default {DEFAULT_LISTEN_ADDR})\n  \
                 ANTHROPIC_API_KEY        Anthropic API key (required for /api)\n  \
                 LAOBAN_MODEL             Anthropic model ID (required for /api)\n  \
                 LAOBAN_TOKEN_CAP         monthly token cap (default 2000000)\n  \
                 LAOBAN_GITHUB_REPO       <owner>/<repo> (required for /api)\n  \
                 LAOBAN_GITHUB_TOKEN      GitHub token (required for /api)\n  \
                 LAOBAN_TARGET_NAMESPACE  default laoban-dev\n  \
                 LAOBAN_CLUSTER_ID        default c-m-26kbxk5f\n\n\
                 Without the required variables /healthz and /version still work and \
                 /api/* answers 500 naming the missing variables."
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
