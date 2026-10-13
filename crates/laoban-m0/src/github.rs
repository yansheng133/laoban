//! Commit a set of files to one branch with the GitHub Git Data API:
//! get ref → base commit → blobs → tree (with `base_tree`) → commit →
//! update ref (`force: false`).
//!
//! No retries: a failed step (including a stale ref, which GitHub reports
//! as 422 or 409 when the branch moved underneath us) is returned as an
//! error and the caller reports `github_failed`. The token is kept in a
//! wrapper whose `Debug` is redacted and is never put into an error.

use std::fmt;
use std::time::Duration;

use serde_json::{json, Value};
use thiserror::Error;

/// Environment variable holding the token.
pub const TOKEN_ENV: &str = "LAOBAN_GITHUB_TOKEN";
pub const DEFAULT_API_BASE: &str = "https://api.github.com";

/// Largest error body read, in bytes.
const MAX_ERROR_BODY: u64 = 64 * 1024;

/// A fine-grained personal access token. `Debug` never shows it.
#[derive(Clone)]
pub struct GithubToken(String);

impl GithubToken {
    pub fn new(token: impl Into<String>) -> Result<Self, GithubError> {
        let token = token.into().trim().to_string();
        if token.is_empty() || !token.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(GithubError::BadToken);
        }
        Ok(Self(token))
    }
}

impl fmt::Debug for GithubToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GithubToken([REDACTED])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubConfig {
    /// `https://api.github.com`; a local mock server in tests.
    pub api_base: String,
    pub owner: String,
    pub repo: String,
    pub branch: String,
    /// Per-call upper bound.
    pub timeout: Duration,
}

impl GithubConfig {
    pub fn new(owner: &str, repo: &str) -> Self {
        Self {
            api_base: DEFAULT_API_BASE.into(),
            owner: owner.into(),
            repo: repo.into(),
            branch: "main".into(),
            timeout: Duration::from_secs(30),
        }
    }
}

/// One file to write, path relative to the repository root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GithubError {
    #[error("GitHub token is empty or malformed")]
    BadToken,
    #[error("refusing to commit an unsafe path")]
    BadPath,
    /// `message` is GitHub's own `message` field (short, no secrets).
    #[error("GitHub {stage} failed with HTTP {status}: {message}")]
    Status {
        stage: &'static str,
        status: u16,
        message: String,
    },
    #[error("GitHub {stage} failed: transport error {kind}")]
    Transport { stage: &'static str, kind: String },
    #[error("GitHub {stage} returned an unexpected response")]
    InvalidResponse { stage: &'static str },
}

pub struct GithubCommitter {
    agent: ureq::Agent,
    config: GithubConfig,
    token: GithubToken,
}

impl fmt::Debug for GithubCommitter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GithubCommitter")
            .field("config", &self.config)
            .field("token", &self.token)
            .finish()
    }
}

impl GithubCommitter {
    pub fn new(config: GithubConfig, token: GithubToken) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout(config.timeout)
            .redirects(0)
            .user_agent(concat!("laoban/", env!("CARGO_PKG_VERSION")))
            .build();
        Self {
            agent,
            config,
            token,
        }
    }

    pub fn config(&self) -> &GithubConfig {
        &self.config
    }

    fn url(&self, rest: &str) -> String {
        format!(
            "{}/repos/{}/{}/{}",
            self.config.api_base.trim_end_matches('/'),
            self.config.owner,
            self.config.repo,
            rest
        )
    }

    fn call(
        &self,
        stage: &'static str,
        method: &str,
        url: &str,
        body: Option<&Value>,
    ) -> Result<Value, GithubError> {
        let req = self
            .agent
            .request(method, url)
            .set("authorization", &format!("Bearer {}", self.token.0))
            .set("accept", "application/vnd.github+json")
            .set("x-github-api-version", "2022-11-28");
        let result = match body {
            Some(b) => req
                .set("content-type", "application/json")
                .send_string(&b.to_string()),
            None => req.call(),
        };
        match result {
            Ok(resp) => {
                let text = resp
                    .into_string()
                    .map_err(|_| GithubError::InvalidResponse { stage })?;
                serde_json::from_str(&text).map_err(|_| GithubError::InvalidResponse { stage })
            }
            Err(ureq::Error::Status(status, resp)) => {
                let mut text = String::new();
                let _ = std::io::Read::read_to_string(
                    &mut std::io::Read::take(resp.into_reader(), MAX_ERROR_BODY),
                    &mut text,
                );
                let message = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| v["message"].as_str().map(str::to_string))
                    .unwrap_or_default()
                    .chars()
                    .take(200)
                    .collect();
                Err(GithubError::Status {
                    stage,
                    status,
                    message,
                })
            }
            Err(ureq::Error::Transport(t)) => Err(GithubError::Transport {
                stage,
                kind: format!("{:?}", t.kind()),
            }),
        }
    }

    /// Commit `files` on top of the branch head and move the branch.
    /// Returns the new commit SHA.
    pub fn commit(&self, files: &[CommitFile], message: &str) -> Result<String, GithubError> {
        for f in files {
            if !safe_path(&f.path) {
                return Err(GithubError::BadPath);
            }
        }
        let branch = &self.config.branch;
        let head = self.call(
            "get ref",
            "GET",
            &self.url(&format!("git/ref/heads/{branch}")),
            None,
        )?;
        let base_commit = sha_at(&head["object"]["sha"], "get ref")?;

        let commit = self.call(
            "get commit",
            "GET",
            &self.url(&format!("git/commits/{base_commit}")),
            None,
        )?;
        let base_tree = sha_at(&commit["tree"]["sha"], "get commit")?;

        let mut entries = Vec::with_capacity(files.len());
        for f in files {
            let blob = self.call(
                "create blob",
                "POST",
                &self.url("git/blobs"),
                Some(&json!({ "content": f.content, "encoding": "utf-8" })),
            )?;
            let sha = sha_at(&blob["sha"], "create blob")?;
            entries.push(json!({ "path": f.path, "mode": "100644", "type": "blob", "sha": sha }));
        }

        let tree = self.call(
            "create tree",
            "POST",
            &self.url("git/trees"),
            Some(&json!({ "base_tree": base_tree, "tree": entries })),
        )?;
        let tree_sha = sha_at(&tree["sha"], "create tree")?;

        let new_commit = self.call(
            "create commit",
            "POST",
            &self.url("git/commits"),
            Some(&json!({ "message": message, "tree": tree_sha, "parents": [base_commit] })),
        )?;
        let new_sha = sha_at(&new_commit["sha"], "create commit")?;

        self.call(
            "update ref",
            "PATCH",
            &self.url(&format!("git/refs/heads/{branch}")),
            Some(&json!({ "sha": new_sha, "force": false })),
        )?;
        Ok(new_sha)
    }
}

/// A git object SHA: 40 (SHA-1) or 64 (SHA-256) lowercase hex digits.
fn sha_at(v: &Value, stage: &'static str) -> Result<String, GithubError> {
    match v.as_str() {
        Some(s)
            if (s.len() == 40 || s.len() == 64)
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) =>
        {
            Ok(s.to_string())
        }
        _ => Err(GithubError::InvalidResponse { stage }),
    }
}

fn safe_path(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('/')
        && !p.contains('\\')
        && p.split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert!(safe_path("apps/demo/web.yaml"));
        for bad in ["", "/etc/x", "apps/../x", "apps//x", "a\\b", "./a"] {
            assert!(!safe_path(bad), "{bad}");
        }
    }

    #[test]
    fn token_debug_is_redacted() {
        let t = GithubToken::new("github_pat_test_0001").unwrap();
        assert!(!format!("{t:?}").contains("github_pat"));
    }
}
