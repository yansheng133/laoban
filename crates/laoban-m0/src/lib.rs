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
//! * [`state`]: laoban's own Kubernetes resources (rounds), or memory.
//! * [`service`]: ties it together behind `plan`, `apply` and `config`.
//! * [`config`]: environment variables.

pub mod codegen;
pub mod config;
pub mod github;
pub mod meta;
pub mod plan;
pub mod render;
pub mod rounds;
pub mod service;
pub mod state;
pub mod store;
mod time;

pub use config::{from_env_with, AiEndpoint, ConfigError, EnvConfig, M0Settings, Secret};
pub use github::{CommitFile, CommitInfo, GithubCommitter, GithubConfig, GithubError, GithubToken};
pub use rounds::Limits as RoundLimits;
pub use service::{
    AddOpinionsRequest, ApplyOptions, ApplyRequest, M0Error, M0Service, OpenRoundRequest,
    OpinionItem, OpinionRequest, PlanRequestBody, RestoreRequest, SaveFilesRequest,
    SaveLayoutRequest, SkipRequest,
};
pub use state::{MemoryStore, StateError, StateStore};

/// The AI returned something that does not match the expected format or
/// limits. The detail is for tests and debugging; it is not shown to users
/// and not logged.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid AI output: {0}")]
pub struct InvalidOutput(pub String);
