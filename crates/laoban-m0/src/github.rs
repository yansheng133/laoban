//! Commit a set of files to one branch with the GitHub Git Data API:
//! get ref → base commit → blobs → tree (with `base_tree`) → commit →
//! update ref (`force: false`). Also reads directory listings and single
//! files from the branch with the contents API (raw media type, so no
//! base64 decoding is needed).
//!
//! No retries: a failed step is returned as an error. A stale ref (GitHub
//! reports 422 or 409 when the branch moved underneath us) is the
//! `update ref` stage with that status; the service reports it as a
//! version conflict, everything else as `github_failed`. The token is kept in a
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
/// Largest file read back with [`GithubCommitter::read_file`], in bytes.
pub const MAX_READ_BYTES: u64 = 1024 * 1024;

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

/// A commit from [`GithubCommitter::list_commits`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub sha: String,
    /// Committer date, RFC 3339.
    pub date: String,
    /// First line of the message.
    pub message: String,
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
        let text = self.call_text(stage, method, url, body, "application/vnd.github+json")?;
        serde_json::from_str(&text).map_err(|_| GithubError::InvalidResponse { stage })
    }

    fn call_text(
        &self,
        stage: &'static str,
        method: &str,
        url: &str,
        body: Option<&Value>,
        accept: &str,
    ) -> Result<String, GithubError> {
        let req = self
            .agent
            .request(method, url)
            .set("authorization", &format!("Bearer {}", self.token.0))
            .set("accept", accept)
            .set("x-github-api-version", "2022-11-28");
        let result = match body {
            Some(b) => req
                .set("content-type", "application/json")
                .send_string(&b.to_string()),
            None => req.call(),
        };
        match result {
            Ok(resp) => {
                let mut text = String::new();
                std::io::Read::read_to_string(
                    &mut std::io::Read::take(resp.into_reader(), MAX_READ_BYTES + 1),
                    &mut text,
                )
                .map_err(|_| GithubError::InvalidResponse { stage })?;
                if text.len() as u64 > MAX_READ_BYTES {
                    return Err(GithubError::InvalidResponse { stage });
                }
                Ok(text)
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

    fn contents_url(&self, path: &str, git_ref: &str) -> String {
        self.url(&format!("contents/{path}?ref={git_ref}"))
    }

    /// Names of the subdirectories of `path` on the branch. A missing
    /// directory is an empty list.
    pub fn list_dirs(&self, path: &str) -> Result<Vec<String>, GithubError> {
        if !safe_path(path) {
            return Err(GithubError::BadPath);
        }
        let url = self.contents_url(path, &self.config.branch);
        let v = match self.call("list directory", "GET", &url, None) {
            Ok(v) => v,
            Err(GithubError::Status { status: 404, .. }) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let items = v.as_array().ok_or(GithubError::InvalidResponse {
            stage: "list directory",
        })?;
        let mut names: Vec<String> = items
            .iter()
            .filter(|i| i["type"] == "dir")
            .filter_map(|i| i["name"].as_str().map(str::to_string))
            .collect();
        names.sort();
        Ok(names)
    }

    /// Names of the files in `path` at `git_ref` (a branch or SHA). A
    /// missing directory or branch is an empty list.
    pub fn list_files_at(&self, path: &str, git_ref: &str) -> Result<Vec<String>, GithubError> {
        if !safe_path(path) || !safe_ref(git_ref) {
            return Err(GithubError::BadPath);
        }
        let url = self.contents_url(path, git_ref);
        let v = match self.call("list directory", "GET", &url, None) {
            Ok(v) => v,
            Err(GithubError::Status { status: 404, .. }) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let items = v.as_array().ok_or(GithubError::InvalidResponse {
            stage: "list directory",
        })?;
        let mut names: Vec<String> = items
            .iter()
            .filter(|i| i["type"] == "file")
            .filter_map(|i| i["name"].as_str().map(str::to_string))
            .collect();
        names.sort();
        Ok(names)
    }

    /// A file's content on the branch, or `None` if it does not exist.
    pub fn read_file(&self, path: &str) -> Result<Option<String>, GithubError> {
        let branch = self.config.branch.clone();
        self.read_file_at(path, &branch)
    }

    /// A file's content at a commit SHA (or branch), or `None` if it does
    /// not exist there.
    pub fn read_file_at(&self, path: &str, git_ref: &str) -> Result<Option<String>, GithubError> {
        if !safe_path(path) || !safe_ref(git_ref) {
            return Err(GithubError::BadPath);
        }
        match self.call_text(
            "read file",
            "GET",
            &self.contents_url(path, git_ref),
            None,
            "application/vnd.github.raw+json",
        ) {
            Ok(text) => Ok(Some(text)),
            Err(GithubError::Status { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Commits on the branch that touched `path`, newest first, at most
    /// `max` (up to 100). A path that never existed is an empty list.
    pub fn list_commits(&self, path: &str, max: usize) -> Result<Vec<CommitInfo>, GithubError> {
        if !safe_path(path) {
            return Err(GithubError::BadPath);
        }
        let per_page = max.clamp(1, 100);
        let url = self.url(&format!(
            "commits?path={path}&sha={}&per_page={per_page}",
            self.config.branch
        ));
        let v = match self.call("list commits", "GET", &url, None) {
            Ok(v) => v,
            Err(GithubError::Status { status: 404, .. }) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let items = v.as_array().ok_or(GithubError::InvalidResponse {
            stage: "list commits",
        })?;
        let mut out = Vec::new();
        for item in items.iter().take(per_page) {
            let sha = sha_at(&item["sha"], "list commits")?;
            out.push(CommitInfo {
                sha,
                date: item["commit"]["committer"]["date"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(40)
                    .collect(),
                message: item["commit"]["message"]
                    .as_str()
                    .unwrap_or("")
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(200)
                    .collect(),
            });
        }
        Ok(out)
    }

    /// Commit `files` on top of the branch head and move the branch.
    /// Returns the new commit SHA.
    pub fn commit(&self, files: &[CommitFile], message: &str) -> Result<String, GithubError> {
        self.commit_changes(files, &[], message)
    }

    /// Like [`commit`](Self::commit), also deleting the files at `deletes`
    /// (paths that exist on the branch; GitHub refuses to delete a missing
    /// path).
    pub fn commit_changes(
        &self,
        files: &[CommitFile],
        deletes: &[String],
        message: &str,
    ) -> Result<String, GithubError> {
        let branch = self.config.branch.clone();
        self.commit_on(&branch, files, deletes, message, false)
    }

    /// Commit `files` on `branch` (another branch than the apps', such as
    /// the archive of rounds). A missing branch is created with this one
    /// commit and no history, so it shares nothing with the apps' branch
    /// and Fleet (which follows that branch) never sees it.
    pub fn commit_to_branch(
        &self,
        branch: &str,
        files: &[CommitFile],
        message: &str,
    ) -> Result<String, GithubError> {
        if !safe_ref(branch) || branch == self.config.branch {
            return Err(GithubError::BadPath);
        }
        self.commit_on(branch, files, &[], message, true)
    }

    fn commit_on(
        &self,
        branch: &str,
        files: &[CommitFile],
        deletes: &[String],
        message: &str,
        create: bool,
    ) -> Result<String, GithubError> {
        for path in files.iter().map(|f| &f.path).chain(deletes) {
            if !safe_path(path) {
                return Err(GithubError::BadPath);
            }
        }
        let head = match self.call(
            "get ref",
            "GET",
            &self.url(&format!("git/ref/heads/{branch}")),
            None,
        ) {
            Ok(head) => Some(head),
            Err(GithubError::Status { status: 404, .. }) if create => None,
            Err(e) => return Err(e),
        };
        let base = match &head {
            Some(head) => {
                let base_commit = sha_at(&head["object"]["sha"], "get ref")?;
                let commit = self.call(
                    "get commit",
                    "GET",
                    &self.url(&format!("git/commits/{base_commit}")),
                    None,
                )?;
                Some((base_commit, sha_at(&commit["tree"]["sha"], "get commit")?))
            }
            None => None,
        };

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
        // A null sha removes the path from the base tree.
        for path in deletes {
            entries.push(json!({ "path": path, "mode": "100644", "type": "blob", "sha": null }));
        }

        let tree_body = match &base {
            Some((_, base_tree)) => json!({ "base_tree": base_tree, "tree": entries }),
            None => json!({ "tree": entries }),
        };
        let tree = self.call(
            "create tree",
            "POST",
            &self.url("git/trees"),
            Some(&tree_body),
        )?;
        let tree_sha = sha_at(&tree["sha"], "create tree")?;

        let parents: Vec<&str> = base.iter().map(|(c, _)| c.as_str()).collect();
        let new_commit = self.call(
            "create commit",
            "POST",
            &self.url("git/commits"),
            Some(&json!({ "message": message, "tree": tree_sha, "parents": parents })),
        )?;
        let new_sha = sha_at(&new_commit["sha"], "create commit")?;

        if base.is_some() {
            self.call(
                "update ref",
                "PATCH",
                &self.url(&format!("git/refs/heads/{branch}")),
                Some(&json!({ "sha": new_sha, "force": false })),
            )?;
        } else {
            self.call(
                "create ref",
                "POST",
                &self.url("git/refs"),
                Some(&json!({ "ref": format!("refs/heads/{branch}"), "sha": new_sha })),
            )?;
        }
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

/// A branch name or commit SHA usable in a query string.
fn safe_ref(r: &str) -> bool {
    !r.is_empty()
        && r.len() <= 100
        && r.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
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
