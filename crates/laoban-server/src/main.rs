//! `laoban-server` binary.
//!
//! Listens on `LAOBAN_LISTEN_ADDR` (default `0.0.0.0:8080`) and stops
//! cleanly on SIGTERM or SIGINT. Handling SIGTERM explicitly matters in a
//! container: as PID 1 a process without a handler ignores SIGTERM, so
//! Kubernetes would have to wait for the grace period and send SIGKILL.

use std::process::ExitCode;

use laoban_server::{git_sha, Server, NAME, VERSION};
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:8080";

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        None => {}
        Some("--version" | "-V") => {
            println!("{NAME} {VERSION} ({})", git_sha());
            return ExitCode::SUCCESS;
        }
        Some("--help" | "-h") => {
            println!(
                "{NAME} {VERSION}\n\nServes GET /healthz and GET /version.\n\n\
                 Environment:\n  LAOBAN_LISTEN_ADDR  listen address (default {DEFAULT_LISTEN_ADDR})"
            );
            return ExitCode::SUCCESS;
        }
        Some(other) => {
            eprintln!("{NAME}: unknown argument {other:?} (try --help)");
            return ExitCode::from(2);
        }
    }

    let addr = std::env::var("LAOBAN_LISTEN_ADDR").unwrap_or_else(|_| DEFAULT_LISTEN_ADDR.into());
    let server = match Server::bind(addr.as_str()) {
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
