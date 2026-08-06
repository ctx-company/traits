//! Files-backed task board (0060): the first [`TaskProvider`] implementation,
//! reading and writing `.internal/tasks`-shaped board directories.
//!
//! A board directory holds `.toml` task documents as direct children (the
//! live set) plus an `archived/` subdirectory (documents closed via
//! `done`/`cancelled`). Every call re-reads the directory. The backend
//! itself carries no cache and no watcher; a persisted-cache freshness
//! layer above it (0063.7, `ctx-traits-io::task_board_cache`) decides when
//! a consumer needs to call in here again via [`board_fingerprint`].

use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};

use ctx_traits_core::digest::Digest;
use ctx_traits_core::task::graph::{self, EdgeKind};
use ctx_traits_core::task::provider::{
    self, DuplicateKey, EffectKind, EffectOutcome, EffectRecord, NewTask, ParseFailure,
    ProviderError, ResolvedTask, SyncReport, TaskProvider, TaskProviderMut, TaskSummary,
    TaskUpdate, UpdateOutcome, WriteError,
};
use ctx_traits_core::task::{Relations, TaskDocument, TaskStatus};

const ARCHIVED_DIR: &str = "archived";

/// The board-config file name, reserved among a board directory's direct
/// children: never parsed as a task document, never resolvable as one
/// (0063.6).
const BOARD_CONFIG_FILE: &str = "board.toml";

/// Per-board effect declarations (0063.6), loaded from an optional
/// `board.toml` in the board directory. `deny_unknown_fields` on both
/// levels makes an undeclared effect name a load error rather than a
/// silently ignored key — the closed set of two named effects is the whole
/// surface, not an extension point for scripted ones.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
struct BoardConfig {
    effects: EffectsConfig,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
struct EffectsConfig {
    #[serde(rename = "archive-on-close")]
    archive_on_close: bool,
}

impl Default for EffectsConfig {
    fn default() -> Self {
        Self {
            archive_on_close: true,
        }
    }
}

/// Load `board_dir`'s `board.toml`, or the all-defaults config when the
/// file is absent. A present-but-unparseable or unknown-key file is a
/// `ProviderError` — validated when the config loads, not deferred to the
/// first effect that would have used it.
fn load_board_config(board_dir: &Utf8Path) -> Result<BoardConfig, ProviderError> {
    let path = board_dir.join(BOARD_CONFIG_FILE);
    match std::fs::read_to_string(path.as_std_path()) {
        Ok(text) => toml::from_str(&text)
            .map_err(|e| ProviderError(format!("{path}: invalid board config: {e}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BoardConfig::default()),
        Err(e) => Err(provider_error(&path, e)),
    }
}

/// Resolve `task_value` to a file name among `dir`'s direct children — the
/// exact filename, the exact stem, or (for a bare `NNNN[.M...]`-shaped key)
/// the `<task_value>-` prefix. Subdirectories never match. Candidates are
/// sorted so a prefix match is deterministic even if numbers ever collide.
///
/// Shared by [`crate::dispatch_preflight`] (live board only) and
/// [`Board::resolve`] below (live, then archived).
pub(crate) fn task_file_name_in_dir(dir: &Utf8Path, task_value: &str) -> Option<String> {
    let entries = std::fs::read_dir(dir.as_std_path()).ok()?;
    let key_like = is_key_like(task_value);
    let prefix = format!("{task_value}-");
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_file())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| name.ends_with(".toml") && name != BOARD_CONFIG_FILE)
        .collect();
    names.sort();
    names.into_iter().find(|name| {
        let stem = &name[..name.len() - 5];
        name == task_value || stem == task_value || (key_like && stem.starts_with(&prefix))
    })
}

/// Whether `s` is shaped like a bare task key: one or more dot-separated
/// groups of ASCII digits (`0010`, `0010.1`). The pre-0060 check
/// (`is_ascii_digit` over the whole value) rejected dotted child keys
/// outright, so a dotted key never prefix-matched its archived file.
fn is_key_like(s: &str) -> bool {
    !s.is_empty()
        && s.split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

fn provider_error(context: impl std::fmt::Display, source: std::io::Error) -> ProviderError {
    ProviderError(format!("{context}: {source}"))
}

/// The snapshot a `load()` call assembles from disk: the deterministic
/// per-key document map (first-wins by sorted path, live before archived),
/// every path that declared each key (duplicates included), which keys
/// resolved from `archived/`, and any file that failed to parse.
struct LoadedBoard {
    documents: BTreeMap<String, TaskDocument>,
    locations: BTreeMap<String, Vec<Utf8PathBuf>>,
    archived_keys: BTreeSet<String>,
    parse_failures: Vec<ParseFailure>,
    /// `sha256:<hex>` of the first-wins document's exact stored source text,
    /// per key (0063.5) — what a caller's `expected_digest` is checked
    /// against.
    digests: BTreeMap<String, String>,
}

/// One board directory's stat-sweep signature (0063.7): every direct
/// `*.toml` child of `board_dir` (including `board.toml`) and of
/// `board_dir/archived`, as (archived, name, mtime-seconds, len), sorted. No
/// parsing — cheap enough to run on every dashboard tick. A file appearing,
/// disappearing, or moving between the two directories changes the
/// fingerprint because names participate in it, not just mtimes. A missing
/// `archived/` fingerprints as empty, not an error.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BoardFingerprint(Vec<(bool, String, i64, u64)>);

/// Sweep `board_dir` for [`BoardFingerprint`] without reading any file's
/// contents.
pub fn board_fingerprint(board_dir: &Utf8Path) -> Result<BoardFingerprint, ProviderError> {
    let mut entries = Vec::new();
    for (dir, archived) in [
        (board_dir.to_path_buf(), false),
        (board_dir.join(ARCHIVED_DIR), true),
    ] {
        let read_dir = match std::fs::read_dir(dir.as_std_path()) {
            Ok(read_dir) => read_dir,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(provider_error(&dir, e)),
        };
        for entry in read_dir {
            let entry = entry.map_err(|e| provider_error(&dir, e))?;
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let metadata = entry.metadata().map_err(|e| provider_error(&dir, e))?;
            let modified = metadata.modified().map_err(|e| provider_error(&dir, e))?;
            let secs = modified
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            entries.push((archived, name.to_string(), secs, metadata.len()));
        }
    }
    entries.sort();
    Ok(BoardFingerprint(entries))
}

/// The private filesystem shell shared by both the read-only and
/// read-write wrapper types below.
struct Board {
    board_dir: Utf8PathBuf,
}

impl Board {
    fn new(board_dir: impl Into<Utf8PathBuf>) -> Self {
        Self {
            board_dir: board_dir.into(),
        }
    }

    fn archived_dir(&self) -> Utf8PathBuf {
        self.board_dir.join(ARCHIVED_DIR)
    }

    /// Load every `.toml` document from the live directory then
    /// `archived/`, in that order, so a duplicate key's first occurrence
    /// (and therefore the one `documents`/`archived_keys` carries) is
    /// always the live copy when one exists.
    fn load(&self) -> Result<LoadedBoard, ProviderError> {
        let mut documents = BTreeMap::new();
        let mut locations: BTreeMap<String, Vec<Utf8PathBuf>> = BTreeMap::new();
        let mut archived_keys = BTreeSet::new();
        let mut parse_failures = Vec::new();
        let mut digests: BTreeMap<String, String> = BTreeMap::new();

        for (dir, archived) in [(self.board_dir.clone(), false), (self.archived_dir(), true)] {
            let Ok(entries) = std::fs::read_dir(dir.as_std_path()) else {
                continue;
            };
            let mut paths: Vec<Utf8PathBuf> = entries
                .flatten()
                .filter(|entry| entry.path().is_file())
                .filter_map(|entry| Utf8PathBuf::from_path_buf(entry.path()).ok())
                .filter(|path| path.extension() == Some("toml"))
                .filter(|path| archived || path.file_name() != Some(BOARD_CONFIG_FILE))
                .collect();
            paths.sort();
            for path in paths {
                let text = match std::fs::read_to_string(path.as_std_path()) {
                    Ok(text) => text,
                    Err(e) => {
                        parse_failures.push(ParseFailure {
                            location: path.to_string(),
                            reason: e.to_string(),
                        });
                        continue;
                    }
                };
                match ctx_traits_core::task::parse(&text) {
                    Ok(document) => {
                        let key = document.key.clone();
                        locations.entry(key.clone()).or_default().push(path.clone());
                        if let std::collections::btree_map::Entry::Vacant(slot) =
                            documents.entry(key.clone())
                        {
                            digests.insert(key.clone(), Digest::source(&text).as_str().to_string());
                            slot.insert(document);
                            if archived {
                                archived_keys.insert(key);
                            }
                        }
                    }
                    Err(e) => {
                        parse_failures.push(ParseFailure {
                            location: path.to_string(),
                            reason: e.to_string(),
                        });
                    }
                }
            }
        }

        Ok(LoadedBoard {
            documents,
            locations,
            archived_keys,
            parse_failures,
            digests,
        })
    }

    fn resolve(&self, task_value: &str) -> Result<Option<String>, ProviderError> {
        if let Some(name) = task_file_name_in_dir(&self.board_dir, task_value) {
            return self.key_for_file(&self.board_dir.join(name));
        }
        if let Some(name) = task_file_name_in_dir(&self.archived_dir(), task_value) {
            return self.key_for_file(&self.archived_dir().join(name));
        }
        Ok(None)
    }

    fn key_for_file(&self, path: &Utf8Path) -> Result<Option<String>, ProviderError> {
        let text =
            std::fs::read_to_string(path.as_std_path()).map_err(|e| provider_error(path, e))?;
        Ok(ctx_traits_core::task::parse(&text).ok().map(|doc| doc.key))
    }

    fn get(&self, key: &str) -> Result<Option<ResolvedTask>, ProviderError> {
        let loaded = self.load()?;
        if !loaded.documents.contains_key(key) {
            return Ok(None);
        }
        let archived = loaded.archived_keys.contains(key);
        let digest = loaded.digests.get(key).cloned().unwrap_or_default();
        Ok(Some(provider::resolve_task(
            &loaded.documents,
            key,
            archived,
            digest,
        )))
    }

    fn list(&self, include_archived: bool) -> Result<Vec<TaskSummary>, ProviderError> {
        let loaded = self.load()?;
        Ok(loaded
            .documents
            .keys()
            .filter(|key| include_archived || !loaded.archived_keys.contains(*key))
            .map(|key| {
                provider::summarize(&loaded.documents, key, loaded.archived_keys.contains(key))
            })
            .collect())
    }

    fn sync(&self) -> Result<SyncReport, ProviderError> {
        let loaded = self.load()?;
        let dangling_edges = graph::dangling_edges(&loaded.documents);
        let duplicate_keys = loaded
            .locations
            .into_iter()
            .filter(|(_, paths)| paths.len() > 1)
            .map(|(key, paths)| DuplicateKey {
                key,
                locations: paths.into_iter().map(|p| p.to_string()).collect(),
            })
            .collect();
        Ok(SyncReport {
            dangling_edges,
            parse_failures: loaded.parse_failures,
            duplicate_keys,
        })
    }

    /// Single, unambiguous location for `key`, or the write refusal the
    /// task carries: absent, or claimed by more than one file.
    fn single_location<'a>(
        loaded: &'a LoadedBoard,
        key: &str,
    ) -> Result<&'a Utf8PathBuf, WriteError> {
        let locations = loaded
            .locations
            .get(key)
            .ok_or_else(|| WriteError::NotFound(key.to_string()))?;
        match locations.as_slice() {
            [path] => Ok(path),
            _ => Err(WriteError::AmbiguousKey(key.to_string())),
        }
    }

    fn write_document(
        &self,
        dir: &Utf8Path,
        file_name: &str,
        document: &TaskDocument,
    ) -> Result<(), WriteError> {
        let text = ctx_traits_core::task::serialize(document)
            .map_err(|e| WriteError::from(ProviderError(e.to_string())))?;
        std::fs::create_dir_all(dir.as_std_path()).map_err(|e| provider_error(dir, e))?;
        std::fs::write(dir.join(file_name).as_std_path(), text)
            .map_err(|e| provider_error(dir, e))?;
        Ok(())
    }

    fn create(&self, new_task: NewTask) -> Result<TaskSummary, WriteError> {
        let loaded = self.load()?;
        let key = match &new_task.parent {
            None => next_top_level_key(&loaded),
            Some(parent) => {
                Self::single_location(&loaded, parent)?;
                next_child_key(&loaded, parent)
            }
        };
        let archived = matches!(
            new_task.status,
            Some(TaskStatus::Done) | Some(TaskStatus::Cancelled)
        );
        let document = TaskDocument {
            schema_version: ctx_traits_core::task::SCHEMA_VERSION.to_string(),
            key: key.clone(),
            title: new_task.title,
            status: new_task.status,
            raised: None,
            closed: None,
            wall: None,
            origin: None,
            content: new_task.content,
            scope: String::new(),
            validation: new_task.validation,
            relations: Relations {
                depends_on: new_task.depends_on,
                parent: new_task.parent,
            },
            steps: new_task.steps,
        };
        let file_name = format!("{key}-{}.toml", slugify(&document.title));
        let dir = if archived {
            self.archived_dir()
        } else {
            self.board_dir.clone()
        };
        self.write_document(&dir, &file_name, &document)?;

        let reloaded = self.load()?;
        Ok(provider::summarize(&reloaded.documents, &key, archived))
    }

    fn update(&self, key: &str, update: TaskUpdate) -> Result<UpdateOutcome, WriteError> {
        let loaded = self.load()?;
        let current_path = Self::single_location(&loaded, key)?.clone();
        let mut document = loaded
            .documents
            .get(key)
            .cloned()
            .ok_or_else(|| WriteError::NotFound(key.to_string()))?;
        let was_archived = loaded.archived_keys.contains(key);
        let closing = matches!(
            update.status,
            Some(TaskStatus::Done) | Some(TaskStatus::Cancelled)
        );

        if let Some(expected) = &update.expected_digest
            && loaded.digests.get(key) != Some(expected)
        {
            return Err(WriteError::StaleWrite {
                key: key.to_string(),
            });
        }

        if let Some(title) = &update.title
            && title.trim().is_empty()
        {
            return Err(WriteError::InvalidField {
                field: "title",
                reason: "title must not be empty or whitespace-only".to_string(),
            });
        }

        if update.release_dependents && !closing {
            return Err(WriteError::InvalidField {
                field: "release_dependents",
                reason: "releasing dependents requires this update to set a closing status \
                         (done or cancelled)"
                    .to_string(),
            });
        }

        for (step_id, _) in &update.set_steps_done {
            if !document.steps.iter().any(|step| &step.id == step_id) {
                return Err(WriteError::UnknownStep {
                    key: key.to_string(),
                    step_id: step_id.clone(),
                });
            }
        }

        // Cycle checks run against a clone of `loaded.documents` with this
        // write's own removals already applied — a re-point (remove+add in
        // one call) must not see the edge it is deleting as still closing a
        // loop, so removals apply to the clone before the added edges are
        // checked against it.
        let mut post_removal = loaded.documents.clone();
        if let Some(clone_doc) = post_removal.get_mut(key) {
            clone_doc
                .relations
                .depends_on
                .retain(|dep| !update.remove_depends_on.contains(dep));
            if let Some(new_parent) = &update.set_parent {
                clone_doc.relations.parent = new_parent.clone();
            }
        }
        for dep in &update.add_depends_on {
            if let Some(cycle) =
                graph::would_create_cycle(&post_removal, EdgeKind::DependsOn, key, dep)
            {
                return Err(cycle.into());
            }
        }
        if let Some(Some(new_parent)) = &update.set_parent
            && let Some(cycle) =
                graph::would_create_cycle(&post_removal, EdgeKind::Parent, key, new_parent)
        {
            return Err(cycle.into());
        }

        if let Some(title) = update.title {
            document.title = title;
        }
        for dep in &update.add_depends_on {
            if !document.relations.depends_on.contains(dep) {
                document.relations.depends_on.push(dep.clone());
            }
        }
        document
            .relations
            .depends_on
            .retain(|dep| !update.remove_depends_on.contains(dep));
        if let Some(parent) = update.set_parent {
            document.relations.parent = parent;
        }
        if let Some(content) = update.content {
            document.content = content;
        }
        if let Some(scope) = update.scope {
            document.scope = scope;
        }
        if let Some(validation) = update.validation {
            document.validation = validation;
        }
        if let Some(wall) = update.set_wall {
            document.wall = wall;
        }
        if let Some(origin) = update.set_origin {
            document.origin = origin;
        }
        for (step_id, done) in &update.set_steps_done {
            if let Some(step) = document.steps.iter_mut().find(|step| &step.id == step_id) {
                step.done = *done;
            }
        }
        let effects_config = load_board_config(&self.board_dir)?.effects;
        let mut archive_target = was_archived;
        if let Some(status) = update.status {
            document.status = Some(status);
            if closing && !was_archived {
                document.closed = Some(crate::audit_journal::today_date_utc());
            } else if !closing && was_archived {
                document.closed = None;
            }
            if effects_config.archive_on_close {
                archive_target = closing;
            }
        }

        let file_name = current_path
            .file_name()
            .ok_or_else(|| {
                WriteError::from(ProviderError(format!("{current_path}: no file name")))
            })?
            .to_string();
        let target_dir = if archive_target {
            self.archived_dir()
        } else {
            self.board_dir.clone()
        };
        self.write_document(&target_dir, &file_name, &document)?;
        let target_path = target_dir.join(&file_name);
        if target_path != current_path {
            std::fs::remove_file(current_path.as_std_path())
                .map_err(|e| provider_error(&current_path, e))?;
        }

        let mut effects = Vec::new();
        if archive_target != was_archived {
            effects.push(EffectRecord {
                effect: EffectKind::ArchivePlacement,
                outcome: EffectOutcome::Applied,
                documents: vec![target_path.to_string()],
            });
        }

        if update.release_dependents {
            effects.extend(self.sweep_dependents(&loaded, key));
        }

        let reloaded = self.load()?;
        Ok(UpdateOutcome {
            summary: provider::summarize(&reloaded.documents, key, archive_target),
            effects,
        })
    }

    /// The dependents sweep (0063.6): every task that directly
    /// `depends-on` `key`, per the pre-write snapshot `loaded`, has that
    /// edge removed by a direct document write — never a recursive
    /// `self.update()`, so the sweep is structurally one hop and triggers
    /// no further effects. Each dependent is re-read and its digest
    /// compared against `loaded.digests` before it is touched (0063.5's
    /// per-document stale refusal); a stale or otherwise failed dependent
    /// is recorded and skipped, never a reason to fail the whole sweep or
    /// roll back the primary write already applied by the caller.
    fn sweep_dependents(&self, loaded: &LoadedBoard, key: &str) -> Vec<EffectRecord> {
        let dependents = graph::blockers_of(&loaded.documents, key);
        let mut applied = Vec::new();
        let mut failed: Vec<(String, String)> = Vec::new();
        for dependent in dependents {
            let dep_key = dependent.key.clone();
            match self.release_one_dependent(loaded, &dep_key, key) {
                Ok(()) => applied.push(dep_key),
                Err(e) => failed.push((dep_key, e.to_string())),
            }
        }
        let mut records = Vec::new();
        if !applied.is_empty() {
            records.push(EffectRecord {
                effect: EffectKind::ReleaseDependents,
                outcome: EffectOutcome::Applied,
                documents: applied,
            });
        }
        for (dep_key, reason) in failed {
            records.push(EffectRecord {
                effect: EffectKind::ReleaseDependents,
                outcome: EffectOutcome::Failed { reason },
                documents: vec![dep_key],
            });
        }
        records
    }

    /// Remove the `depends_on` edge naming `released` from `dep_key`'s
    /// document, refusing (without touching disk) if `dep_key` changed
    /// since `loaded` was taken.
    fn release_one_dependent(
        &self,
        loaded: &LoadedBoard,
        dep_key: &str,
        released: &str,
    ) -> Result<(), WriteError> {
        let path = Self::single_location(loaded, dep_key)?.clone();
        let text =
            std::fs::read_to_string(path.as_std_path()).map_err(|e| provider_error(&path, e))?;
        let current_digest = Digest::source(&text).as_str().to_string();
        if loaded.digests.get(dep_key) != Some(&current_digest) {
            return Err(WriteError::StaleWrite {
                key: dep_key.to_string(),
            });
        }
        let mut document = ctx_traits_core::task::parse(&text)
            .map_err(|e| WriteError::from(ProviderError(e.to_string())))?;
        document.relations.depends_on.retain(|dep| dep != released);

        let dir = path
            .parent()
            .map(Utf8PathBuf::from)
            .ok_or_else(|| WriteError::from(ProviderError(format!("{path}: no parent dir"))))?;
        let file_name = path
            .file_name()
            .ok_or_else(|| WriteError::from(ProviderError(format!("{path}: no file name"))))?
            .to_string();
        self.write_document(&dir, &file_name, &document)
    }
}

/// The next top-level `NNNN` key: one past the highest leading numeric
/// segment among every live-or-archived key (a dotted child key's leading
/// segment counts toward its parent's number, not a new one).
fn next_top_level_key(loaded: &LoadedBoard) -> String {
    let max = loaded
        .documents
        .keys()
        .filter_map(|key| key.split('.').next())
        .filter_map(|segment| segment.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    format!("{:04}", max + 1)
}

/// The next `parent.M` child key: one past the highest sibling ordinal
/// among `parent`'s direct children.
fn next_child_key(loaded: &LoadedBoard, parent: &str) -> String {
    let max = loaded
        .documents
        .values()
        .filter(|doc| doc.relations.parent.as_deref() == Some(parent))
        .filter_map(|doc| doc.key.strip_prefix(parent))
        .filter_map(|rest| rest.strip_prefix('.'))
        .filter_map(|ordinal| ordinal.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    format!("{parent}.{}", max + 1)
}

/// A lowercase, dash-separated slug for a task title, for the filename
/// convention `NNNN-<slug>.toml`. Never empty — an all-punctuation title
/// falls back to `task` rather than producing a bare `NNNN-.toml`.
fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            out.push(ch.to_ascii_lowercase());
            pending_dash = false;
        } else {
            pending_dash = true;
        }
    }
    if out.is_empty() {
        out.push_str("task");
    }
    out
}

/// A read-only handle over a board directory — the only capability a run
/// may hold. See [`FilesTaskBoard::open_read`].
pub struct ReadOnlyBoard(Board);

/// A read-write handle over a board directory — the CLI and dashboard
/// only. See [`FilesTaskBoard::open_read_write`].
pub struct ReadWriteBoard(Board);

impl TaskProvider for ReadOnlyBoard {
    fn resolve(&self, task_value: &str) -> Result<Option<String>, ProviderError> {
        self.0.resolve(task_value)
    }

    fn get(&self, key: &str) -> Result<Option<ResolvedTask>, ProviderError> {
        self.0.get(key)
    }

    fn list(&self, include_archived: bool) -> Result<Vec<TaskSummary>, ProviderError> {
        self.0.list(include_archived)
    }

    fn sync(&self) -> Result<SyncReport, ProviderError> {
        self.0.sync()
    }
}

impl TaskProvider for ReadWriteBoard {
    fn resolve(&self, task_value: &str) -> Result<Option<String>, ProviderError> {
        self.0.resolve(task_value)
    }

    fn get(&self, key: &str) -> Result<Option<ResolvedTask>, ProviderError> {
        self.0.get(key)
    }

    fn list(&self, include_archived: bool) -> Result<Vec<TaskSummary>, ProviderError> {
        self.0.list(include_archived)
    }

    fn sync(&self) -> Result<SyncReport, ProviderError> {
        self.0.sync()
    }
}

impl TaskProviderMut for ReadWriteBoard {
    fn create(&self, new_task: NewTask) -> Result<TaskSummary, WriteError> {
        self.0.create(new_task)
    }

    fn update(&self, key: &str, update: TaskUpdate) -> Result<UpdateOutcome, WriteError> {
        self.0.update(key, update)
    }
}

/// The files-backed board entry point: two constructors selecting the
/// capability the caller gets back at the type level. Neither reads the
/// board eagerly — every verb call re-reads it.
pub struct FilesTaskBoard;

impl FilesTaskBoard {
    /// Open `board_dir` for reads only.
    pub fn open_read(board_dir: impl Into<Utf8PathBuf>) -> ReadOnlyBoard {
        ReadOnlyBoard(Board::new(board_dir))
    }

    /// Open `board_dir` for reads and writes.
    pub fn open_read_write(board_dir: impl Into<Utf8PathBuf>) -> ReadWriteBoard {
        ReadWriteBoard(Board::new(board_dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_core::task::graph::DerivedStatus;

    fn tempdir() -> Utf8PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "task-files-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Utf8PathBuf::from_path_buf(dir).unwrap()
    }

    fn write_task(dir: &Utf8Path, file_name: &str, toml: &str) {
        std::fs::write(dir.join(file_name).as_std_path(), toml).unwrap();
    }

    const TASK_0001: &str =
        "schema-version = \"0.2\"\nkey = \"0001\"\ntitle = \"First\"\nstatus = \"ready\"\n";

    #[test]
    fn list_hides_archived_by_default_and_get_still_resolves_it() {
        let board_dir = tempdir();
        write_task(&board_dir, "0001-first.toml", TASK_0001);
        std::fs::create_dir_all(board_dir.join(ARCHIVED_DIR).as_std_path()).unwrap();
        write_task(
            &board_dir.join(ARCHIVED_DIR),
            "0002-second.toml",
            "schema-version = \"0.2\"\nkey = \"0002\"\ntitle = \"Second\"\nstatus = \"done\"\n",
        );

        let read = FilesTaskBoard::open_read(board_dir.clone());
        let list = read.list(false).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].key, "0001");

        let list_all = read.list(true).unwrap();
        assert_eq!(list_all.len(), 2);

        let archived_task = read.get("0002").unwrap().expect("archived task resolves");
        assert!(archived_task.archived);
        assert_eq!(archived_task.derived_status, DerivedStatus::Done);
    }

    #[test]
    fn update_to_done_moves_the_file_to_archived_and_get_still_resolves_it() {
        let board_dir = tempdir();
        write_task(&board_dir, "0001-first.toml", TASK_0001);

        let write = FilesTaskBoard::open_read_write(board_dir.clone());
        let outcome = write
            .update(
                "0001",
                TaskUpdate {
                    status: Some(TaskStatus::Done),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(outcome.summary.archived);
        assert!(!board_dir.join("0001-first.toml").exists());
        assert!(
            board_dir
                .join(ARCHIVED_DIR)
                .join("0001-first.toml")
                .exists()
        );
        assert_eq!(outcome.effects.len(), 1);
        assert_eq!(outcome.effects[0].effect, EffectKind::ArchivePlacement);
        assert_eq!(outcome.effects[0].outcome, EffectOutcome::Applied);
        assert_eq!(
            outcome.effects[0].documents,
            vec![
                board_dir
                    .join(ARCHIVED_DIR)
                    .join("0001-first.toml")
                    .to_string()
            ]
        );

        let list = write.list(false).unwrap();
        assert!(list.is_empty());
        let fetched = write.get("0001").unwrap().expect("still resolves archived");
        assert!(fetched.archived);

        // Reopening moves it back and records the move too.
        let outcome = write
            .update(
                "0001",
                TaskUpdate {
                    status: Some(TaskStatus::Ready),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(!outcome.summary.archived);
        assert_eq!(outcome.effects.len(), 1);
        assert_eq!(outcome.effects[0].effect, EffectKind::ArchivePlacement);
        assert!(board_dir.join("0001-first.toml").exists());
        assert!(
            !board_dir
                .join(ARCHIVED_DIR)
                .join("0001-first.toml")
                .exists()
        );
    }

    #[test]
    fn create_assigns_top_level_and_child_keys() {
        let board_dir = tempdir();
        write_task(
            &board_dir,
            "0005-existing.toml",
            "schema-version = \"0.2\"\nkey = \"0005\"\ntitle = \"Existing\"\nstatus = \"ready\"\n",
        );

        let write = FilesTaskBoard::open_read_write(board_dir.clone());
        let created = write
            .create(NewTask {
                title: "A New Task".to_string(),
                content: String::new(),
                status: None,
                depends_on: Vec::new(),
                parent: None,
                validation: String::new(),
                steps: Vec::new(),
            })
            .unwrap();
        assert_eq!(created.key, "0006");
        assert!(board_dir.join("0006-a-new-task.toml").exists());

        let child = write
            .create(NewTask {
                title: "Child One".to_string(),
                content: String::new(),
                status: None,
                depends_on: Vec::new(),
                parent: Some("0005".to_string()),
                validation: String::new(),
                steps: Vec::new(),
            })
            .unwrap();
        assert_eq!(child.key, "0005.1");

        let second_child = write
            .create(NewTask {
                title: "Child Two".to_string(),
                content: String::new(),
                status: None,
                depends_on: Vec::new(),
                parent: Some("0005".to_string()),
                validation: String::new(),
                steps: Vec::new(),
            })
            .unwrap();
        assert_eq!(second_child.key, "0005.2");
    }

    #[test]
    fn create_round_trips_validation_and_steps() {
        let board_dir = tempdir();
        let write = FilesTaskBoard::open_read_write(board_dir.clone());
        let steps = vec![ctx_traits_core::task::Step {
            id: "step-1".to_string(),
            title: "do the thing".to_string(),
            done: false,
            content: "operational detail".to_string(),
        }];
        let created = write
            .create(NewTask {
                title: "Split Child".to_string(),
                content: "why this exists".to_string(),
                status: None,
                depends_on: Vec::new(),
                parent: None,
                validation: "done when the thing is done".to_string(),
                steps: steps.clone(),
            })
            .unwrap();
        let resolved = write.get(&created.key).unwrap().unwrap();
        assert_eq!(resolved.document.validation, "done when the thing is done");
        assert_eq!(resolved.document.steps, steps);
        assert_eq!(resolved.open_steps, steps);
    }

    #[test]
    fn dotted_child_key_resolves_from_archived() {
        let board_dir = tempdir();
        std::fs::create_dir_all(board_dir.join(ARCHIVED_DIR).as_std_path()).unwrap();
        write_task(
            &board_dir.join(ARCHIVED_DIR),
            "0010.1-leaf.toml",
            "schema-version = \"0.2\"\nkey = \"0010.1\"\ntitle = \"Leaf\"\nstatus = \"done\"\n",
        );

        let read = FilesTaskBoard::open_read(board_dir);
        assert_eq!(read.resolve("0010.1").unwrap(), Some("0010.1".to_string()));
    }

    #[test]
    fn duplicate_key_board_reports_and_refuses_writes() {
        let board_dir = tempdir();
        write_task(
            &board_dir,
            "0110-a.toml",
            "schema-version = \"0.2\"\nkey = \"0110\"\ntitle = \"A\"\nstatus = \"ready\"\n",
        );
        write_task(
            &board_dir,
            "0110-b.toml",
            "schema-version = \"0.2\"\nkey = \"0110\"\ntitle = \"B\"\nstatus = \"ready\"\n",
        );

        let write = FilesTaskBoard::open_read_write(board_dir);
        let report = write.sync().unwrap();
        assert_eq!(report.duplicate_keys.len(), 1);
        assert_eq!(report.duplicate_keys[0].key, "0110");
        assert_eq!(report.duplicate_keys[0].locations.len(), 2);

        let err = write
            .update(
                "0110",
                TaskUpdate {
                    status: Some(TaskStatus::Done),
                    title: Some("Retitled".to_string()),
                    scope: Some("new scope".to_string()),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(matches!(err, WriteError::AmbiguousKey(key) if key == "0110"));
    }

    #[test]
    fn write_refuses_a_cycle_naming_both_paths() {
        let board_dir = tempdir();
        write_task(
            &board_dir,
            "0001-a.toml",
            "schema-version = \"0.2\"\nkey = \"0001\"\ntitle = \"A\"\nstatus = \"ready\"\n",
        );
        write_task(
            &board_dir,
            "0002-b.toml",
            "schema-version = \"0.2\"\nkey = \"0002\"\ntitle = \"B\"\nstatus = \"ready\"\nrelations.depends-on = [\"0001\"]\n",
        );

        let write = FilesTaskBoard::open_read_write(board_dir);
        let err = write
            .update(
                "0001",
                TaskUpdate {
                    add_depends_on: vec!["0002".to_string()],
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(matches!(err, WriteError::CycleRefused { .. }));
    }

    #[test]
    fn update_names_every_new_field_and_get_reflects_them() {
        let board_dir = tempdir();
        write_task(
            &board_dir,
            "0001-a.toml",
            "schema-version = \"0.2\"\nkey = \"0001\"\ntitle = \"A\"\nstatus = \"ready\"\n\n[[steps]]\nid = \"s1\"\ntitle = \"first\"\ndone = false\n",
        );

        let write = FilesTaskBoard::open_read_write(board_dir);
        write
            .update(
                "0001",
                TaskUpdate {
                    title: Some("Retitled A".to_string()),
                    scope: Some("the scope".to_string()),
                    validation: Some("the validation".to_string()),
                    set_wall: Some(Some("wall-1".to_string())),
                    set_origin: Some(Some("run-1".to_string())),
                    set_steps_done: vec![("s1".to_string(), true)],
                    ..Default::default()
                },
            )
            .unwrap();

        let fetched = write.get("0001").unwrap().unwrap();
        assert_eq!(fetched.document.title, "Retitled A");
        assert_eq!(fetched.document.scope, "the scope");
        assert_eq!(fetched.document.validation, "the validation");
        assert_eq!(fetched.document.wall, Some("wall-1".to_string()));
        assert_eq!(fetched.document.origin, Some("run-1".to_string()));
        assert!(fetched.document.steps[0].done);
        assert!(fetched.open_steps.is_empty());
    }

    #[test]
    fn untouched_fields_survive_a_content_only_update_byte_stable() {
        let board_dir = tempdir();
        let document = TaskDocument {
            schema_version: ctx_traits_core::task::SCHEMA_VERSION.to_string(),
            key: "0001".to_string(),
            title: "A".to_string(),
            status: Some(TaskStatus::Ready),
            raised: None,
            closed: None,
            wall: Some("wall-1".to_string()),
            origin: Some("run-1".to_string()),
            content: "old content".to_string(),
            scope: "the scope".to_string(),
            validation: "the validation".to_string(),
            relations: Relations::default(),
            steps: Vec::new(),
        };
        let canonical = ctx_traits_core::task::serialize(&document).unwrap();
        write_task(&board_dir, "0001-a.toml", &canonical);

        let write = FilesTaskBoard::open_read_write(board_dir.clone());
        write
            .update(
                "0001",
                TaskUpdate {
                    content: Some("new content".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

        let mut expected = document;
        expected.content = "new content".to_string();
        let expected_text = ctx_traits_core::task::serialize(&expected).unwrap();
        let actual_text =
            std::fs::read_to_string(board_dir.join("0001-a.toml").as_std_path()).unwrap();
        assert_eq!(actual_text, expected_text);
    }

    #[test]
    fn stale_write_refuses_naming_staleness_fresh_digest_succeeds_none_bypasses() {
        let board_dir = tempdir();
        write_task(&board_dir, "0001-a.toml", TASK_0001);

        let write = FilesTaskBoard::open_read_write(board_dir.clone());
        let fetched = write.get("0001").unwrap().unwrap();
        let stale_digest = fetched.digest.clone();

        // The document changes on disk after the caller's snapshot.
        write_task(
            &board_dir,
            "0001-a.toml",
            "schema-version = \"0.2\"\nkey = \"0001\"\ntitle = \"Changed\"\nstatus = \"ready\"\n",
        );

        let err = write
            .update(
                "0001",
                TaskUpdate {
                    expected_digest: Some(stale_digest),
                    content: Some("attempted".to_string()),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(matches!(err, WriteError::StaleWrite { key } if key == "0001"));

        let fresh_digest = write.get("0001").unwrap().unwrap().digest;
        write
            .update(
                "0001",
                TaskUpdate {
                    expected_digest: Some(fresh_digest),
                    content: Some("landed".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            write.get("0001").unwrap().unwrap().document.content,
            "landed"
        );

        // `expected_digest: None` bypasses the staleness check entirely.
        write
            .update(
                "0001",
                TaskUpdate {
                    content: Some("bypassed".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            write.get("0001").unwrap().unwrap().document.content,
            "bypassed"
        );
    }

    #[test]
    fn repoint_in_one_call_is_cycle_checked_against_the_post_removal_graph() {
        let board_dir = tempdir();
        write_task(
            &board_dir,
            "0001-a.toml",
            "schema-version = \"0.2\"\nkey = \"0001\"\ntitle = \"A\"\nstatus = \"ready\"\n",
        );
        write_task(
            &board_dir,
            "0002-b.toml",
            "schema-version = \"0.2\"\nkey = \"0002\"\ntitle = \"B\"\nstatus = \"ready\"\nrelations.depends-on = [\"0001\"]\n",
        );
        write_task(
            &board_dir,
            "0003-c.toml",
            "schema-version = \"0.2\"\nkey = \"0003\"\ntitle = \"C\"\nstatus = \"ready\"\n",
        );

        let write = FilesTaskBoard::open_read_write(board_dir);

        // 0001 depends on 0003 today. Re-pointing it in one call — remove
        // the 0003 edge, add a 0002 edge — still names a real cycle (0002
        // already depends on 0001), and must refuse even though the add is
        // paired with a remove in the same `TaskUpdate`.
        write
            .update(
                "0001",
                TaskUpdate {
                    add_depends_on: vec!["0003".to_string()],
                    ..Default::default()
                },
            )
            .unwrap();

        let err = write
            .update(
                "0001",
                TaskUpdate {
                    remove_depends_on: vec!["0003".to_string()],
                    add_depends_on: vec!["0002".to_string()],
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(matches!(err, WriteError::CycleRefused { .. }));
    }

    #[test]
    fn step_flip_mutates_in_place_and_unknown_step_id_refuses() {
        let board_dir = tempdir();
        write_task(
            &board_dir,
            "0001-a.toml",
            "schema-version = \"0.2\"\nkey = \"0001\"\ntitle = \"A\"\nstatus = \"ready\"\n\n[[steps]]\nid = \"s1\"\ntitle = \"first\"\ndone = false\n\n[[steps]]\nid = \"s2\"\ntitle = \"second\"\ndone = false\n",
        );

        let write = FilesTaskBoard::open_read_write(board_dir);
        write
            .update(
                "0001",
                TaskUpdate {
                    set_steps_done: vec![("s1".to_string(), true)],
                    ..Default::default()
                },
            )
            .unwrap();
        let fetched = write.get("0001").unwrap().unwrap();
        assert_eq!(fetched.document.steps.len(), 2);
        assert!(fetched.document.steps[0].done);
        assert!(!fetched.document.steps[1].done);

        let err = write
            .update(
                "0001",
                TaskUpdate {
                    set_steps_done: vec![("no-such-step".to_string(), true)],
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(
            matches!(err, WriteError::UnknownStep { key, step_id } if key == "0001" && step_id == "no-such-step")
        );
    }

    #[test]
    fn empty_title_refused() {
        let board_dir = tempdir();
        write_task(&board_dir, "0001-a.toml", TASK_0001);

        let write = FilesTaskBoard::open_read_write(board_dir);
        let err = write
            .update(
                "0001",
                TaskUpdate {
                    title: Some("   ".to_string()),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(matches!(
            err,
            WriteError::InvalidField { field: "title", .. }
        ));
    }

    #[test]
    fn closed_is_cleared_on_reopen() {
        let board_dir = tempdir();
        write_task(&board_dir, "0001-a.toml", TASK_0001);

        let write = FilesTaskBoard::open_read_write(board_dir);
        write
            .update(
                "0001",
                TaskUpdate {
                    status: Some(TaskStatus::Done),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            write
                .get("0001")
                .unwrap()
                .unwrap()
                .document
                .closed
                .is_some()
        );

        write
            .update(
                "0001",
                TaskUpdate {
                    status: Some(TaskStatus::Ready),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(write.get("0001").unwrap().unwrap().document.closed, None);
    }

    #[test]
    fn closing_a_dependency_unblocks_the_dependent_with_no_write_to_its_file() {
        let board_dir = tempdir();
        write_task(
            &board_dir,
            "0001-a.toml",
            "schema-version = \"0.2\"\nkey = \"0001\"\ntitle = \"A\"\nstatus = \"ready\"\n",
        );
        write_task(
            &board_dir,
            "0002-b.toml",
            "schema-version = \"0.2\"\nkey = \"0002\"\ntitle = \"B\"\nstatus = \"ready\"\nrelations.depends-on = [\"0001\"]\n",
        );
        let dependent_before =
            std::fs::read_to_string(board_dir.join("0002-b.toml").as_std_path()).unwrap();

        let write = FilesTaskBoard::open_read_write(board_dir.clone());
        assert_eq!(
            write.get("0002").unwrap().unwrap().derived_status,
            DerivedStatus::Blocked
        );

        write
            .update(
                "0001",
                TaskUpdate {
                    status: Some(TaskStatus::Done),
                    ..Default::default()
                },
            )
            .unwrap();

        assert_eq!(
            write.get("0002").unwrap().unwrap().derived_status,
            DerivedStatus::Ready
        );
        let dependent_after =
            std::fs::read_to_string(board_dir.join("0002-b.toml").as_std_path()).unwrap();
        assert_eq!(dependent_before, dependent_after);
    }

    #[test]
    fn archive_on_close_false_skips_the_move_and_records_no_effect() {
        let board_dir = tempdir();
        write_task(&board_dir, "0001-a.toml", TASK_0001);
        write_task(
            &board_dir,
            BOARD_CONFIG_FILE,
            "[effects]\narchive-on-close = false\n",
        );

        let write = FilesTaskBoard::open_read_write(board_dir.clone());
        let outcome = write
            .update(
                "0001",
                TaskUpdate {
                    status: Some(TaskStatus::Done),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(!outcome.summary.archived);
        assert!(outcome.effects.is_empty());
        assert!(board_dir.join("0001-a.toml").exists());
        assert!(!board_dir.join(ARCHIVED_DIR).join("0001-a.toml").exists());
    }

    #[test]
    fn unknown_effects_key_is_a_load_error() {
        let board_dir = tempdir();
        write_task(&board_dir, "0001-a.toml", TASK_0001);
        write_task(
            &board_dir,
            BOARD_CONFIG_FILE,
            "[effects]\narchive-on-scripting = true\n",
        );

        let write = FilesTaskBoard::open_read_write(board_dir);
        let err = write
            .update(
                "0001",
                TaskUpdate {
                    status: Some(TaskStatus::Done),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(matches!(err, WriteError::Backend(_)));
    }

    #[test]
    fn board_toml_never_surfaces_as_a_task_or_a_parse_failure() {
        let board_dir = tempdir();
        write_task(&board_dir, "0001-a.toml", TASK_0001);
        write_task(
            &board_dir,
            BOARD_CONFIG_FILE,
            "[effects]\narchive-on-close = true\n",
        );

        let read = FilesTaskBoard::open_read(board_dir.clone());
        let list = read.list(true).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].key, "0001");

        let report = read.sync().unwrap();
        assert!(report.parse_failures.is_empty());

        assert_eq!(read.resolve("board").unwrap(), None);
    }

    #[test]
    fn release_dependents_removes_both_edges_lists_both_documents_and_stale_one_fails() {
        let board_dir = tempdir();
        write_task(
            &board_dir,
            "0001-a.toml",
            "schema-version = \"0.2\"\nkey = \"0001\"\ntitle = \"A\"\nstatus = \"ready\"\n",
        );
        write_task(
            &board_dir,
            "0002-b.toml",
            "schema-version = \"0.2\"\nkey = \"0002\"\ntitle = \"B\"\nstatus = \"ready\"\nrelations.depends-on = [\"0001\"]\n",
        );
        write_task(
            &board_dir,
            "0003-c.toml",
            "schema-version = \"0.2\"\nkey = \"0003\"\ntitle = \"C\"\nstatus = \"ready\"\nrelations.depends-on = [\"0001\"]\n",
        );

        let write = FilesTaskBoard::open_read_write(board_dir.clone());
        let outcome = write
            .update(
                "0001",
                TaskUpdate {
                    status: Some(TaskStatus::Done),
                    release_dependents: true,
                    ..Default::default()
                },
            )
            .unwrap();

        let sweep = outcome
            .effects
            .iter()
            .find(|e| e.effect == EffectKind::ReleaseDependents)
            .expect("release-dependents effect recorded");
        assert_eq!(sweep.outcome, EffectOutcome::Applied);
        let mut documents = sweep.documents.clone();
        documents.sort();
        assert_eq!(documents, vec!["0002".to_string(), "0003".to_string()]);

        assert!(
            write
                .get("0002")
                .unwrap()
                .unwrap()
                .document
                .relations
                .depends_on
                .is_empty()
        );
        assert!(
            write
                .get("0003")
                .unwrap()
                .unwrap()
                .document
                .relations
                .depends_on
                .is_empty()
        );
    }

    #[test]
    fn release_dependents_without_a_closing_status_is_refused() {
        let board_dir = tempdir();
        write_task(
            &board_dir,
            "0001-a.toml",
            "schema-version = \"0.2\"\nkey = \"0001\"\ntitle = \"A\"\nstatus = \"ready\"\n",
        );
        write_task(
            &board_dir,
            "0002-b.toml",
            "schema-version = \"0.2\"\nkey = \"0002\"\ntitle = \"B\"\nstatus = \"ready\"\nrelations.depends-on = [\"0001\"]\n",
        );

        let write = FilesTaskBoard::open_read_write(board_dir);
        let err = write
            .update(
                "0001",
                TaskUpdate {
                    release_dependents: true,
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(matches!(
            err,
            WriteError::InvalidField {
                field: "release_dependents",
                ..
            }
        ));
    }

    #[test]
    fn a_dependent_that_changed_since_the_snapshot_fails_its_own_entry_and_keeps_its_file() {
        let board_dir = tempdir();
        write_task(
            &board_dir,
            "0001-a.toml",
            "schema-version = \"0.2\"\nkey = \"0001\"\ntitle = \"A\"\nstatus = \"ready\"\n",
        );
        write_task(
            &board_dir,
            "0002-b.toml",
            "schema-version = \"0.2\"\nkey = \"0002\"\ntitle = \"B\"\nstatus = \"ready\"\nrelations.depends-on = [\"0001\"]\n",
        );
        write_task(
            &board_dir,
            "0003-c.toml",
            "schema-version = \"0.2\"\nkey = \"0003\"\ntitle = \"C\"\nstatus = \"ready\"\nrelations.depends-on = [\"0001\"]\n",
        );

        let write = FilesTaskBoard::open_read_write(board_dir.clone());
        let loaded = write.0.load().unwrap();

        // 0002 changes on disk after the snapshot `sweep_dependents` below
        // is handed — the exact race 0063.5's per-document refusal guards.
        let rewritten_0002 = "schema-version = \"0.2\"\nkey = \"0002\"\ntitle = \"B renamed\"\nstatus = \"ready\"\nrelations.depends-on = [\"0001\"]\n";
        write_task(&board_dir, "0002-b.toml", rewritten_0002);

        let records = write.0.sweep_dependents(&loaded, "0001");
        let applied: Vec<_> = records
            .iter()
            .filter(|r| r.outcome == EffectOutcome::Applied)
            .flat_map(|r| r.documents.clone())
            .collect();
        let failed: Vec<_> = records
            .iter()
            .filter(|r| matches!(r.outcome, EffectOutcome::Failed { .. }))
            .flat_map(|r| r.documents.clone())
            .collect();
        assert_eq!(applied, vec!["0003".to_string()]);
        assert_eq!(failed, vec!["0002".to_string()]);

        // The stale dependent's file is untouched...
        let actual_0002 =
            std::fs::read_to_string(board_dir.join("0002-b.toml").as_std_path()).unwrap();
        assert_eq!(actual_0002, rewritten_0002);
        // ...while the fresh one had its edge removed.
        assert!(
            write
                .get("0003")
                .unwrap()
                .unwrap()
                .document
                .relations
                .depends_on
                .is_empty()
        );
    }

    fn set_mtime(path: &Utf8Path, secs_from_epoch: u64) {
        let file = std::fs::File::options()
            .write(true)
            .open(path.as_std_path())
            .unwrap();
        file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs_from_epoch))
            .unwrap();
    }

    #[test]
    fn fingerprint_is_stable_over_an_unchanged_directory() {
        let board_dir = tempdir();
        write_task(&board_dir, "0001-first.toml", TASK_0001);

        let a = board_fingerprint(&board_dir).unwrap();
        let b = board_fingerprint(&board_dir).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn fingerprint_changes_on_added_removed_and_moved_files() {
        let board_dir = tempdir();
        write_task(&board_dir, "0001-first.toml", TASK_0001);
        let baseline = board_fingerprint(&board_dir).unwrap();

        write_task(&board_dir, "0002-second.toml", TASK_0001);
        let added = board_fingerprint(&board_dir).unwrap();
        assert_ne!(baseline, added);

        std::fs::remove_file(board_dir.join("0002-second.toml").as_std_path()).unwrap();
        let removed = board_fingerprint(&board_dir).unwrap();
        assert_eq!(removed, baseline);

        std::fs::create_dir_all(board_dir.join(ARCHIVED_DIR).as_std_path()).unwrap();
        std::fs::rename(
            board_dir.join("0001-first.toml").as_std_path(),
            board_dir
                .join(ARCHIVED_DIR)
                .join("0001-first.toml")
                .as_std_path(),
        )
        .unwrap();
        let moved = board_fingerprint(&board_dir).unwrap();
        assert_ne!(moved, baseline);
    }

    #[test]
    fn fingerprint_tolerates_missing_archived_dir() {
        let board_dir = tempdir();
        write_task(&board_dir, "0001-first.toml", TASK_0001);
        assert!(!board_dir.join(ARCHIVED_DIR).as_std_path().exists());
        board_fingerprint(&board_dir).unwrap();
    }

    #[test]
    fn fingerprint_changes_on_mtime_or_length_bump_with_same_name() {
        let board_dir = tempdir();
        write_task(&board_dir, "0001-first.toml", TASK_0001);
        set_mtime(&board_dir.join("0001-first.toml"), 1_000_000);
        let baseline = board_fingerprint(&board_dir).unwrap();

        set_mtime(&board_dir.join("0001-first.toml"), 1_000_100);
        let mtime_bumped = board_fingerprint(&board_dir).unwrap();
        assert_ne!(baseline, mtime_bumped);

        write_task(
            &board_dir,
            "0001-first.toml",
            &format!("{TASK_0001}extra = 1\n"),
        );
        set_mtime(&board_dir.join("0001-first.toml"), 1_000_000);
        let length_bumped = board_fingerprint(&board_dir).unwrap();
        assert_ne!(baseline, length_bumped);
    }
}
