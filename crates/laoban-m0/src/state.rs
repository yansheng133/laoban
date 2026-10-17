//! Where laoban keeps its own state between restarts: one Kubernetes
//! resource per object in the backend's namespace (`laoban-system`), group
//! `laoban.yansheng133.github.io`. The backend may read and write only
//! these resources there (its Role); users cannot reach them.
//!
//! Every write names the version it read (`resourceVersion`), so two
//! writers never overwrite each other: the second gets
//! [`StateError::Conflict`], reads again and decides. This is the shared
//! layer for rounds now and for the shared drafts later (code, opinion
//! drafts, plan review).
//!
//! [`MemoryStore`] behaves the same in memory, for tests and for running
//! outside a cluster.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use serde_json::Value;

/// Largest object written, in bytes of JSON. Kubernetes refuses objects
/// over about 1.5 MB; this leaves room.
pub const MAX_OBJECT_BYTES: usize = 512 * 1024;

pub const API_GROUP: &str = "laoban.yansheng133.github.io";
pub const API_VERSION: &str = "v1alpha1";
/// Label naming the app an object belongs to.
pub const APP_LABEL: &str = "laoban.yansheng133.github.io/app";

/// A kind of stored object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kind {
    /// `rounds`
    pub plural: &'static str,
    /// `Round`
    pub kind: &'static str,
}

pub const ROUNDS: Kind = Kind {
    plural: "rounds",
    kind: "Round",
};

/// A plan waiting to be confirmed (outside a round), kept across restarts.
pub const PLANS: Kind = Kind {
    plural: "plans",
    kind: "Plan",
};

/// The settled AI tokens of one month, kept across restarts.
pub const TOKEN_USAGES: Kind = Kind {
    plural: "tokenusages",
    kind: "TokenUsage",
};

/// One stored object: its name, version, labels and `spec`.
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub name: String,
    pub version: String,
    pub labels: BTreeMap<String, String>,
    pub spec: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StateError {
    /// Changed since it was read (or deleted).
    #[error("changed by another writer")]
    Conflict,
    #[error("already exists")]
    Exists,
    #[error("not found")]
    NotFound,
    /// The resource type is not installed (the CRD is missing).
    #[error("the {0} resource is not installed")]
    NotInstalled(&'static str),
    #[error("object is larger than {MAX_OBJECT_BYTES} bytes")]
    TooLarge,
    /// Anything else (no access, cannot reach the API server). The detail
    /// is for the server log; it never contains the token or user text.
    #[error("state store: {0}")]
    Other(String),
}

pub trait StateStore: Send + Sync {
    fn list(&self, kind: Kind) -> Result<Vec<Object>, StateError>;
    fn get(&self, kind: Kind, name: &str) -> Result<Option<Object>, StateError>;
    /// Create; returns the new version.
    fn create(
        &self,
        kind: Kind,
        name: &str,
        labels: &BTreeMap<String, String>,
        spec: &Value,
    ) -> Result<String, StateError>;
    /// Replace the spec if the object is still at `version`; returns the
    /// new version.
    fn update(
        &self,
        kind: Kind,
        name: &str,
        version: &str,
        spec: &Value,
    ) -> Result<String, StateError>;
    /// Delete; a missing object is not an error.
    fn delete(&self, kind: Kind, name: &str) -> Result<(), StateError>;
}

/// Refuse objects the API server would refuse (or that would crowd etcd).
pub fn check_size(spec: &Value) -> Result<(), StateError> {
    if spec.to_string().len() > MAX_OBJECT_BYTES {
        return Err(StateError::TooLarge);
    }
    Ok(())
}

/// A valid object name (RFC 1123 subdomain, at most 253 characters).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        && name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && name
            .bytes()
            .last()
            .is_some_and(|b| b.is_ascii_alphanumeric())
}

type Key = (&'static str, String);

/// In-memory store with the same version checks as Kubernetes.
#[derive(Default)]
pub struct MemoryStore {
    objects: Mutex<BTreeMap<Key, Object>>,
    counter: AtomicU64,
    /// Tests: act as if the resource type were not installed.
    missing: AtomicBool,
    /// Tests: fail the next writes with `Other`.
    failing_writes: AtomicU64,
    writes: AtomicU64,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Tests: pretend the CRD is (not) installed.
    pub fn set_installed(&self, installed: bool) {
        self.missing.store(!installed, Ordering::SeqCst);
    }

    /// Tests: make the next `n` writes fail.
    pub fn fail_next_writes(&self, n: u64) {
        self.failing_writes.store(n, Ordering::SeqCst);
    }

    /// Writes done so far (create, update, delete).
    pub fn write_count(&self) -> u64 {
        self.writes.load(Ordering::SeqCst)
    }

    /// Tests: change an object behind the writer's back (another replica,
    /// or someone with kubectl).
    pub fn touch(&self, kind: Kind, name: &str, f: impl FnOnce(&mut Value)) {
        let mut objects = self.objects.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(o) = objects.get_mut(&(kind.plural, name.to_string())) {
            f(&mut o.spec);
            o.version = self.next_version();
        }
    }

    /// Tests: delete an object behind the writer's back.
    pub fn remove(&self, kind: Kind, name: &str) {
        self.lock().remove(&(kind.plural, name.to_string()));
    }

    /// Tests: delete everything (as reinstalling the CRD does).
    pub fn clear(&self) {
        self.lock().clear();
    }

    fn next_version(&self) -> String {
        (self.counter.fetch_add(1, Ordering::SeqCst) + 1).to_string()
    }

    fn check(&self, kind: Kind) -> Result<(), StateError> {
        if self.missing.load(Ordering::SeqCst) {
            return Err(StateError::NotInstalled(kind.plural));
        }
        Ok(())
    }

    fn check_write(&self, kind: Kind) -> Result<(), StateError> {
        self.check(kind)?;
        let failing = self.failing_writes.load(Ordering::SeqCst);
        if failing > 0 {
            self.failing_writes.store(failing - 1, Ordering::SeqCst);
            return Err(StateError::Other("simulated failure".into()));
        }
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<Key, Object>> {
        self.objects.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl StateStore for MemoryStore {
    fn list(&self, kind: Kind) -> Result<Vec<Object>, StateError> {
        self.check(kind)?;
        Ok(self
            .lock()
            .iter()
            .filter(|((p, _), _)| *p == kind.plural)
            .map(|(_, o)| o.clone())
            .collect())
    }

    fn get(&self, kind: Kind, name: &str) -> Result<Option<Object>, StateError> {
        self.check(kind)?;
        Ok(self.lock().get(&(kind.plural, name.to_string())).cloned())
    }

    fn create(
        &self,
        kind: Kind,
        name: &str,
        labels: &BTreeMap<String, String>,
        spec: &Value,
    ) -> Result<String, StateError> {
        check_size(spec)?;
        if !valid_name(name) {
            return Err(StateError::Other("invalid name".into()));
        }
        self.check_write(kind)?;
        let mut objects = self.lock();
        let key = (kind.plural, name.to_string());
        if objects.contains_key(&key) {
            return Err(StateError::Exists);
        }
        let version = self.next_version();
        objects.insert(
            key,
            Object {
                name: name.to_string(),
                version: version.clone(),
                labels: labels.clone(),
                spec: spec.clone(),
            },
        );
        Ok(version)
    }

    fn update(
        &self,
        kind: Kind,
        name: &str,
        version: &str,
        spec: &Value,
    ) -> Result<String, StateError> {
        check_size(spec)?;
        self.check_write(kind)?;
        let mut objects = self.lock();
        let o = objects
            .get_mut(&(kind.plural, name.to_string()))
            .ok_or(StateError::Conflict)?;
        if o.version != version {
            return Err(StateError::Conflict);
        }
        o.version = self.next_version();
        o.spec = spec.clone();
        Ok(o.version.clone())
    }

    fn delete(&self, kind: Kind, name: &str) -> Result<(), StateError> {
        self.check_write(kind)?;
        self.lock().remove(&(kind.plural, name.to_string()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn versions_guard_writes() {
        let s = MemoryStore::new();
        let labels = BTreeMap::new();
        let v1 = s
            .create(ROUNDS, "todo-r-1", &labels, &json!({"a": 1}))
            .unwrap();
        assert_eq!(
            s.create(ROUNDS, "todo-r-1", &labels, &json!({})),
            Err(StateError::Exists)
        );
        let v2 = s.update(ROUNDS, "todo-r-1", &v1, &json!({"a": 2})).unwrap();
        assert_eq!(
            s.update(ROUNDS, "todo-r-1", &v1, &json!({"a": 3})),
            Err(StateError::Conflict),
            "a stale version never overwrites"
        );
        s.touch(ROUNDS, "todo-r-1", |v| v["a"] = json!(9));
        assert_eq!(
            s.update(ROUNDS, "todo-r-1", &v2, &json!({"a": 4})),
            Err(StateError::Conflict)
        );
        let o = s.get(ROUNDS, "todo-r-1").unwrap().unwrap();
        assert_eq!(o.spec["a"], 9);
        s.delete(ROUNDS, "todo-r-1").unwrap();
        s.delete(ROUNDS, "todo-r-1").unwrap();
        assert!(s.list(ROUNDS).unwrap().is_empty());
    }

    #[test]
    fn size_names_and_missing_crd() {
        let s = MemoryStore::new();
        let big = json!({ "x": "a".repeat(MAX_OBJECT_BYTES) });
        assert_eq!(
            s.create(ROUNDS, "a", &BTreeMap::new(), &big),
            Err(StateError::TooLarge)
        );
        assert!(valid_name("todo-r-20261016-0a1b2c"));
        assert!(!valid_name("Todo"));
        assert!(!valid_name("-a"));
        assert!(!valid_name("a_b"));
        s.set_installed(false);
        assert_eq!(s.list(ROUNDS), Err(StateError::NotInstalled("rounds")));
        s.set_installed(true);
        s.fail_next_writes(1);
        assert!(matches!(
            s.create(ROUNDS, "a", &BTreeMap::new(), &json!({})),
            Err(StateError::Other(_))
        ));
        assert!(s.create(ROUNDS, "a", &BTreeMap::new(), &json!({})).is_ok());
    }
}
