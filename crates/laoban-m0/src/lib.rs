//! laoban M0 (試吃版) pipeline. See `docs/m0-design.md` and this crate's
//! README.
//!
//! * [`plan`]: the plan JSON the AI returns and its conversion to the
//!   viewer's architecture graph.
//! * [`codegen`]: the four generated files and their limits.
//! * [`render`]: fixed Kubernetes and Fleet templates, `server.py`,
//!   `main.py`, README.
//! * [`github`]: commit a directory with the Git Data API.
//! * [`store`]: in-memory plan store.
//! * [`service`]: ties it together behind `plan`, `apply` and `config`.
//! * [`config`]: environment variables.

pub mod codegen;
pub mod config;
pub mod github;
pub mod plan;
pub mod render;
pub mod service;
pub mod store;
mod time;

pub use config::{from_env_with, AiEndpoint, ConfigError, EnvConfig, M0Settings, Secret};
pub use github::{CommitFile, GithubCommitter, GithubConfig, GithubError, GithubToken};
pub use service::{ApplyRequest, M0Error, M0Service, PlanRequestBody};

/// The AI returned something that does not match the expected format or
/// limits. The detail is for tests and debugging; it is not shown to users
/// and not logged.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid AI output: {0}")]
pub struct InvalidOutput(pub String);
