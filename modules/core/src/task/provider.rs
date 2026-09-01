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

use serde::{Deserialize, Serialize};

use super::graph::{CyclePaths, DerivedStatus, ResolvedRelations};
use super::{AutoClosePolicy, Step, TaskDocument, TaskStatus};

/// The compact Tasks-pane sections. Closed and archived rows are deliberately
/// excluded: callers still retain those served facts for sibling views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BoardSection {
    InProgress,
    Ready,
    Draft,
}

/// Run facts reduced to the board join. Repository identity is required so a
/// same-key run from another checkout cannot claim this board's task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct BoardRun {
    /// The center-supplied run identity. It lets a served board retain the
    /// concrete claiming run rather than reducing it to an anonymous boolean.
    pub run_id: String,
    /// `None` is the TUI's historical current-repository row. A named
    /// repository must match the board before it can join.
    pub repo_key: Option<String>,
    pub task_key: String,
    pub live: bool,
    pub awaiting_owner: bool,
    pub not_merged: bool,
}

/// The precedence result used by task consumers that retain the richer legacy
/// task groups. It keeps run facts and board state in one shared decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardTaskState {
    InProgress,
    Pending,
    AwaitingMerge,
    Board(DerivedStatus),
}

/// Current-repository runs historically omit repository identity; explicitly
/// identified runs must match the board's repository.
pub fn same_repository(board_repo: Option<&str>, run_repo: Option<&str>) -> bool {
    run_repo.is_none() || run_repo == board_repo
}

/// The task-group precedence shared by the TUI and served board: live, then
/// awaiting owner, then unmerged work, then the derived board status.
pub fn task_state(
    derived: DerivedStatus,
    runs: impl IntoIterator<Item = impl std::borrow::Borrow<BoardRun>>,
) -> BoardTaskState {
    let runs: Vec<_> = runs.into_iter().collect();
    if runs.iter().any(|run| run.borrow().live) {
        return BoardTaskState::InProgress;
    }
    if runs.iter().any(|run| run.borrow().awaiting_owner) {
        return BoardTaskState::Pending;
    }
    if runs.iter().any(|run| run.borrow().not_merged) {
        return BoardTaskState::AwaitingMerge;
    }
    BoardTaskState::Board(derived)
}

/// Returns only runs that belong to `repo_key` and `task_key`.
pub fn joined_runs<'a>(
    repo_key: &str,
    task_key: &str,
    runs: impl IntoIterator<Item = &'a BoardRun>,
) -> Vec<&'a BoardRun> {
    runs.into_iter()
        .filter(|run| same_repository(Some(repo_key), run.repo_key.as_deref()))
        .filter(|run| run.task_key == task_key)
        .collect()
}

/// The one central partition rule for the first Tasks pane. A joined live or
/// awaiting-owner run wins over the stored board state; all other open rows
/// are draft only when their derived state is draft.
pub fn section_of(
    derived: DerivedStatus,
    archived: bool,
    joined: impl IntoIterator<Item = impl std::borrow::Borrow<BoardRun>>,
) -> Option<BoardSection> {
    if archived || derived.is_closed() {
        return None;
    }
    let joined: Vec<_> = joined.into_iter().collect();
    if matches!(
        task_state(derived, joined.iter().map(|run| run.borrow())),
        BoardTaskState::InProgress | BoardTaskState::Pending
    ) {
        return Some(BoardSection::InProgress);
    }
    // A claimed draft that is neither live nor awaiting its owner is still a
    // claim, so it belongs with the other non-progress open work in Ready.
    if derived == DerivedStatus::Draft && joined.is_empty() {
        Some(BoardSection::Draft)
    } else {
        Some(BoardSection::Ready)
    }
}

/// Counts the open board rows and their served run state. A row contributes at
/// most once to each count, using the same precedence as [`section_of`].
pub fn board_summary_counts<'a>(
    rows: impl IntoIterator<Item = (&'a TaskSummary, Option<BoardSection>, &'a [BoardRun])>,
) -> (usize, usize, usize, usize) {
    let mut open = 0;
    let mut sections = [false; 3];
    let mut live = 0;
    let mut waiting = 0;
    for (summary, section, runs) in rows {
        let Some(section) = section else { continue };
        open += 1;
        sections[match section {
            BoardSection::InProgress => 0,
            BoardSection::Ready => 1,
            BoardSection::Draft => 2,
        }] = true;
        match task_state(summary.derived_status, runs) {
            BoardTaskState::InProgress => live += 1,
            BoardTaskState::Pending => waiting += 1,
            BoardTaskState::AwaitingMerge | BoardTaskState::Board(_) => {}
        }
    }
    (
        open,
        sections.into_iter().filter(|present| *present).count(),
        live,
        waiting,
    )
}

/// A task reduced to what a list view needs: identity, title, and both the
/// stored and derived status (they can differ — a `Ready`-stored task with
/// an unmet dependency derives to `Blocked`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TaskSummary {
    pub key: String,
    pub title: String,
    pub stored_status: Option<TaskStatus>,
    pub derived_status: DerivedStatus,
    pub archived: bool,
}

/// The task a run claims, reduced to what a face renders without a second
/// lookup. `description` is `TaskDocument.content` verbatim — there is no
/// stored `description` field, and no consumer derives a second one.
/// `stored_status` is the document's own optional status, never a derived
/// board state. `auto_close` is the document's override only; resolving it
/// against `[tasks]` config belongs to the consumer that needs the
/// effective policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ClaimedTask {
    pub key: String,
    pub title: String,
    pub description: String,
    pub stored_status: Option<TaskStatus>,
    pub auto_close: Option<AutoClosePolicy>,
}

/// The full board lede and its compact list form. Both are projections of the
/// one canonical `content` field; layout, truncation, and ellipsis belong to
/// consumers rather than this shared data boundary.
pub fn content_lede(document: &TaskDocument) -> &str {
    &document.content
}

pub fn content_short(document: &TaskDocument) -> &str {
    document
        .content
        .split("\n\n")
        .next()
        .unwrap_or("")
        .trim_end()
}

impl ClaimedTask {
    pub fn from_document(document: &TaskDocument) -> Self {
        Self {
            key: document.key.clone(),
            title: document.title.clone(),
            description: document.content.clone(),
            stored_status: document.status,
            auto_close: document.auto_close,
        }
    }
}

/// A single task fully resolved: its document plus every relation edge
/// resolved to the other side's key, title and current status — a consumer
/// never needs a second lookup to render an edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// `sha256:<hex>` of the stored document's exact source text (0063.5).
    /// A caller that later writes a `TaskUpdate` passes this back as
    /// `expected_digest` to refuse a write against a document that changed
    /// since it was read.
    pub digest: String,
}

/// What `sync` reports: what the backend's own re-read says, never a
/// verification of the repository beyond the board itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SyncReport {
    pub dangling_edges: Vec<super::graph::DanglingEdge>,
    pub parse_failures: Vec<ParseFailure>,
    pub duplicate_keys: Vec<DuplicateKey>,
}

/// A file the backend could not parse into a [`TaskDocument`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ParseFailure {
    pub location: String,
    pub reason: String,
}

/// More than one file in the board declares the same `key`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// 0064: a split child's done-when, carried from the park-report
    /// blocker it was proposed from (`ParkBlocker::done_when`). Empty for
    /// every other caller, matching every `TaskDocument::validation` that
    /// was never set at creation.
    pub validation: String,
    /// 0064: a split child's operational steps, mapped from the park-report
    /// blocker's own `steps` list one-for-one. Empty for every other
    /// caller.
    pub steps: Vec<Step>,
}

/// A partial update: only the fields set to `Some` (or non-empty) change.
/// `set_parent` distinguishes "leave unchanged" (`None`) from "clear the
/// parent" (`Some(None)`) from "set the parent" (`Some(Some(key))`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskUpdate {
    pub title: Option<String>,
    pub status: Option<TaskStatus>,
    pub content: Option<String>,
    pub scope: Option<String>,
    pub validation: Option<String>,
    pub add_depends_on: Vec<String>,
    pub remove_depends_on: Vec<String>,
    pub set_parent: Option<Option<String>>,
    pub set_wall: Option<Option<String>>,
    pub set_origin: Option<Option<String>>,
    /// 0144: recorded proof of how this write closed the task. Refused with
    /// [`WriteError::InvalidField`] unless this same update also sets a
    /// closing status — a closure record with nothing it closed is
    /// meaningless.
    pub set_closure: Option<super::Closure>,
    /// `(step id, new done value)` pairs, applied in order. The step stays
    /// inside its owning document — this never adds, removes, or reorders
    /// steps (0059's line: standalone work is a child task).
    pub set_steps_done: Vec<(String, bool)>,
    /// When `Some`, the write refuses with [`WriteError::StaleWrite`] unless
    /// it matches the stored document's current [`ResolvedTask::digest`].
    /// `None` bypasses the check.
    pub expected_digest: Option<String>,
    /// The explicit ask (0063.6) for the dependents-sweep effect: when
    /// `true` and this update sets a closing status, every task that
    /// directly `depends-on` this one has that edge removed in the same
    /// write. Refused with [`WriteError::InvalidField`] when set without a
    /// closing status — there is nothing to release a dependent from
    /// otherwise. Default `false`: the sweep never runs silently.
    pub release_dependents: bool,
}

/// The closed set of follow-ups an `update` can declare (0063.6). Adding a
/// new kind is a deliberate, reviewed change — this is not an extension
/// point for backend-specific or scripted effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EffectKind {
    /// The files backend's close/reopen archive move (default-on, declared
    /// per board in `board.toml`'s `[effects]` table).
    ArchivePlacement,
    /// The dependents sweep: every direct `depends-on` edge onto the
    /// updated task is removed, one hop, only when
    /// [`TaskUpdate::release_dependents`] asked for it.
    ReleaseDependents,
}

/// What happened when a declared effect ran. `Failed` never rolls back the
/// primary field write — an effect is best-effort by ruling, not a
/// condition on the update succeeding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "outcome")]
pub enum EffectOutcome {
    Applied,
    Failed { reason: String },
}

/// One declared effect's execution, named so nothing an `update` did beyond
/// the field write happens silently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct EffectRecord {
    pub effect: EffectKind,
    #[serde(flatten)]
    pub outcome: EffectOutcome,
    /// The document(s) the effect touched or attempted to touch, by key —
    /// for `ArchivePlacement` the updated task itself (named by the target
    /// path in `reason` on failure); for `ReleaseDependents` every
    /// dependent the sweep visited, applied or failed alike.
    pub documents: Vec<String>,
}

/// `update`'s result: the same [`TaskSummary`] plus every declared effect
/// that ran, in the order it ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct UpdateOutcome {
    pub summary: TaskSummary,
    pub effects: Vec<EffectRecord>,
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
    #[error("invalid {field}: {reason}")]
    InvalidField { field: &'static str, reason: String },
    #[error("task {key:?} has no step {step_id:?}")]
    UnknownStep { key: String, step_id: String },
    #[error("{key:?} changed since you looked — sync and retry")]
    StaleWrite { key: String },
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
    /// archived task un-archives it. Declared effects (0063.6) run after
    /// the field write and are recorded in the returned
    /// [`UpdateOutcome::effects`] — an effect failing never rolls back the
    /// field write itself.
    fn update(&self, key: &str, update: TaskUpdate) -> Result<UpdateOutcome, WriteError>;
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
/// archived and the digest of its stored source text. Same invariant as
/// [`summarize`]: `key` must be present.
pub fn resolve_task(
    documents: &BTreeMap<String, TaskDocument>,
    key: &str,
    archived: bool,
    digest: String,
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
        digest,
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
            checks: Vec::new(),
            auto_close: None,
            closure: None,
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

    #[test]
    fn content_projection_uses_the_first_paragraph_without_truncation() {
        let mut document = doc("0001", Some(TaskStatus::Ready));
        document.content =
            "A deliberately long first paragraph.\nStill first.\n\nSecond paragraph.".to_string();
        assert_eq!(content_lede(&document), document.content);
        assert_eq!(
            content_short(&document),
            "A deliberately long first paragraph.\nStill first."
        );
    }

    #[test]
    fn content_projection_preserves_empty_and_single_paragraph_content() {
        let mut document = doc("0001", Some(TaskStatus::Ready));
        assert_eq!(content_lede(&document), "");
        assert_eq!(content_short(&document), "");
        document.content = "Only paragraph\n".to_string();
        assert_eq!(content_short(&document), "Only paragraph");
    }

    #[test]
    fn sections_are_a_repository_scoped_partition_of_open_tasks() {
        let foreign = BoardRun {
            run_id: "foreign-run".to_string(),
            repo_key: Some("other".to_string()),
            task_key: "0001".to_string(),
            live: true,
            awaiting_owner: false,
            not_merged: false,
        };
        let local = BoardRun {
            run_id: "local-run".to_string(),
            repo_key: Some("repo".to_string()),
            ..foreign.clone()
        };
        let joined = joined_runs("repo", "0001", [&foreign, &local]);
        assert_eq!(joined.len(), 1);
        assert_eq!(
            section_of(DerivedStatus::Ready, false, joined),
            Some(BoardSection::InProgress)
        );
        assert_eq!(
            section_of(DerivedStatus::Draft, false, std::iter::empty::<&BoardRun>()),
            Some(BoardSection::Draft)
        );
        let claimed_draft = BoardRun {
            run_id: "settled-run".to_string(),
            repo_key: Some("repo".to_string()),
            task_key: "0002".to_string(),
            live: false,
            awaiting_owner: false,
            not_merged: true,
        };
        assert_eq!(
            section_of(DerivedStatus::Draft, false, [&claimed_draft]),
            Some(BoardSection::Ready)
        );
        assert_eq!(
            section_of(
                DerivedStatus::Blocked,
                false,
                std::iter::empty::<&BoardRun>()
            ),
            Some(BoardSection::Ready)
        );
        assert_eq!(
            section_of(DerivedStatus::Done, false, std::iter::empty::<&BoardRun>()),
            None
        );
    }
}
