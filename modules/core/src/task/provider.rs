//! The task provider interface (0060): the verb set a task-board backend
//! implements, split into a read-only capability and a read-write one as
//! separate traits.
//!
//! [`TaskProvider`] is the read surface every consumer — including a run —
//! may hold. [`TaskProviderMut`] adds the write verbs and is extended only
//! to the CLI and dashboard. This split is enforced at the API level, not
//! the filesystem level: a value typed as `&dyn TaskProvider` has no write
//! method to call, but the same process can still reach the board's files
//! directly through other means (shell access, a different crate). The
//! guarantee is "the provider interface cannot be misused by accident," not
//! a permission boundary.
//!
//! Every type here is backend-agnostic — no filesystem, no board-directory
//! shape — so the files backend ([`crate` consumer `ctx-traits-io`]) is one
//! implementation among possible future others (a hosted tracker, Linear).

use std::collections::BTreeMap;

use serde::Serialize;

use super::graph::{CyclePaths, DerivedStatus, ResolvedRelations};
use super::{Step, TaskDocument, TaskStatus};

/// A task reduced to what a list view needs: identity, title, and both the
/// stored and derived status (they can differ — a `Ready`-stored task with
/// an unmet dependency derives to `Blocked`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TaskSummary {
    pub key: String,
    pub title: String,
    pub stored_status: Option<TaskStatus>,
    pub derived_status: DerivedStatus,
    pub archived: bool,
}

/// A single task fully resolved: its document plus every relation edge
/// resolved to the other side's key, title and current status — a consumer
/// never needs a second lookup to render an edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ResolvedTask {
    pub document: TaskDocument,
    pub derived_status: DerivedStatus,
    pub relations: ResolvedRelations,
    pub archived: bool,
    /// Derived from `document.steps` (0062): steps not yet marked `done`,
    /// in document order. Not stored — recomputed on every resolve so it
    /// never drifts from the document it was derived from.
    pub open_steps: Vec<Step>,
}

/// What `sync` reports: what the backend's own re-read says, never a
/// verification of the repository beyond the board itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct SyncReport {
    pub dangling_edges: Vec<super::graph::DanglingEdge>,
    pub parse_failures: Vec<ParseFailure>,
    pub duplicate_keys: Vec<DuplicateKey>,
}

/// A file the backend could not parse into a [`TaskDocument`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ParseFailure {
    pub location: String,
    pub reason: String,
}

/// More than one file in the board declares the same `key`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct DuplicateKey {
    pub key: String,
    pub locations: Vec<String>,
}

/// Fields a `create` call supplies; the provider assigns the key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewTask {
    pub title: String,
    pub content: String,
    pub status: Option<TaskStatus>,
    pub depends_on: Vec<String>,
    /// A child key (`NNNN.M`) is assigned when this names a parent.
    pub parent: Option<String>,
}

/// A partial update: only the fields set to `Some` (or non-empty) change.
/// `set_parent` distinguishes "leave unchanged" (`None`) from "clear the
/// parent" (`Some(None)`) from "set the parent" (`Some(Some(key))`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskUpdate {
    pub status: Option<TaskStatus>,
    pub content: Option<String>,
    pub add_depends_on: Vec<String>,
    pub remove_depends_on: Vec<String>,
    pub set_parent: Option<Option<String>>,
}

/// A backend-level failure common to both reads and writes (an unreadable
/// board directory, an I/O error) — never raised for "no match", which
/// reads answer with `None`/an empty list instead.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("backend failure: {0}")]
pub struct ProviderError(pub String);

/// A write the provider refused rather than perform.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WriteError {
    #[error("no task {0:?} in the board")]
    NotFound(String),
    #[error(
        "writing this relation would create a cycle: existing path {path_a:?}, new edge {path_b:?}"
    )]
    CycleRefused {
        path_a: Vec<String>,
        path_b: Vec<String>,
    },
    #[error(
        "key {0:?} is ambiguous — more than one document declares it; resolve the duplicate before writing"
    )]
    AmbiguousKey(String),
    #[error(transparent)]
    Backend(#[from] ProviderError),
}

impl From<CyclePaths> for WriteError {
    fn from(paths: CyclePaths) -> Self {
        WriteError::CycleRefused {
            path_a: paths.path_a,
            path_b: paths.path_b,
        }
    }
}

/// The read-only capability: `resolve`, `get`, `list`, `sync`. A run holds
/// this — never [`TaskProviderMut`].
pub trait TaskProvider {
    /// Resolve `task_value` (a bare key, a dotted child key, a filename, or
    /// a stem) to the canonical key of the task it names, or `None` if
    /// nothing matches. Resolution rules are the backend's own — the files
    /// backend follows `task_file_name_in_dir`'s chain.
    fn resolve(&self, task_value: &str) -> Result<Option<String>, ProviderError>;

    /// The task named by `key`, fully resolved, or `None` if no task has
    /// that exact key. Resolves both live and archived tasks — an inbound
    /// edge from a live task to an archived one must still resolve.
    fn get(&self, key: &str) -> Result<Option<ResolvedTask>, ProviderError>;

    /// Every task the backend knows about. `include_archived` false (the
    /// default) hides archived tasks; true includes them.
    fn list(&self, include_archived: bool) -> Result<Vec<TaskSummary>, ProviderError>;

    /// Re-read the backend and report what it says about its own state —
    /// dangling edges, unparseable documents, duplicate keys. Never
    /// verifies anything about the repository beyond the board itself, and
    /// never runs implicitly.
    fn sync(&self) -> Result<SyncReport, ProviderError>;
}

/// The read-write capability. Only the CLI and dashboard hold this.
pub trait TaskProviderMut: TaskProvider {
    /// Create a task. The provider assigns the key: the next top-level
    /// `NNNN` when `new_task.parent` is `None`, or the next child ordinal
    /// (`parent.M`) when it names an existing task.
    fn create(&self, new_task: NewTask) -> Result<TaskSummary, WriteError>;

    /// Apply a partial update to the task named by `key`. A relation write
    /// that would create a cycle is refused (naming both paths) before
    /// anything is written. A status write that closes the task
    /// (`done`/`cancelled`) archives it; a status write that reopens an
    /// archived task un-archives it.
    fn update(&self, key: &str, update: TaskUpdate) -> Result<TaskSummary, WriteError>;
}

/// Build a [`TaskSummary`] for `key` from a snapshot, given whether it is
/// archived — the one place summary construction happens, so every backend
/// derives the same status the same way. `documents` must already contain
/// `key`, else this panics; callers own that invariant.
pub fn summarize(
    documents: &BTreeMap<String, TaskDocument>,
    key: &str,
    archived: bool,
) -> TaskSummary {
    let doc = documents
        .get(key)
        .expect("summarize called with a key absent from the snapshot");
    TaskSummary {
        key: doc.key.clone(),
        title: doc.title.clone(),
        stored_status: doc.status,
        derived_status: super::graph::derived_status(documents, key),
        archived,
    }
}

/// Build a [`ResolvedTask`] for `key` from a snapshot, given whether it is
/// archived. Same invariant as [`summarize`]: `key` must be present.
pub fn resolve_task(
    documents: &BTreeMap<String, TaskDocument>,
    key: &str,
    archived: bool,
) -> ResolvedTask {
    let doc = documents
        .get(key)
        .expect("resolve_task called with a key absent from the snapshot")
        .clone();
    ResolvedTask {
        derived_status: super::graph::derived_status(documents, key),
        relations: super::graph::resolved_relations(documents, key),
        open_steps: doc.open_steps().into_iter().cloned().collect(),
        document: doc,
        archived,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Relations;

    fn doc(key: &str, status: Option<TaskStatus>) -> TaskDocument {
        TaskDocument {
            schema_version: crate::task::SCHEMA_VERSION.to_string(),
            key: key.to_string(),
            title: format!("title {key}"),
            status,
            raised: None,
            closed: None,
            wall: None,
            origin: None,
            content: String::new(),
            scope: String::new(),
            validation: String::new(),
            relations: Relations::default(),
            steps: Vec::new(),
        }
    }

    #[test]
    fn summarize_carries_stored_and_derived_status_separately() {
        let dependency = doc("0001", Some(TaskStatus::Ready));
        let mut dependent = doc("0002", Some(TaskStatus::Ready));
        dependent.relations.depends_on = vec!["0001".to_string()];
        let documents: BTreeMap<_, _> = [dependency, dependent]
            .into_iter()
            .map(|d| (d.key.clone(), d))
            .collect();

        let summary = summarize(&documents, "0002", false);
        assert_eq!(summary.stored_status, Some(TaskStatus::Ready));
        assert_eq!(summary.derived_status, DerivedStatus::Blocked);
    }

    #[test]
    fn write_error_from_cycle_paths_names_both() {
        let paths = CyclePaths {
            path_a: vec!["0001".to_string()],
            path_b: vec!["0002".to_string()],
        };
        let error: WriteError = paths.into();
        match error {
            WriteError::CycleRefused { path_a, path_b } => {
                assert_eq!(path_a, vec!["0001".to_string()]);
                assert_eq!(path_b, vec!["0002".to_string()]);
            }
            other => panic!("expected CycleRefused, got {other:?}"),
        }
    }
}
