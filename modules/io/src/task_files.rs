//! Files-backed task board (0060): the first [`TaskProvider`] implementation,
//! reading and writing `.internal/tasks`-shaped board directories.
//!
//! A board directory holds `.toml` task documents as direct children (the
//! live set) plus an `archived/` subdirectory (documents closed via
//! `done`/`cancelled`). Every call re-reads the directory — no cache, no
//! watcher — since `sync` is manual only and boards are small.

use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};

use ctx_traits_core::task::graph::{self, EdgeKind};
use ctx_traits_core::task::provider::{
    self, DuplicateKey, NewTask, ParseFailure, ProviderError, ResolvedTask, SyncReport,
    TaskProvider, TaskProviderMut, TaskSummary, TaskUpdate, WriteError,
};
use ctx_traits_core::task::{Relations, TaskDocument, TaskStatus};

const ARCHIVED_DIR: &str = "archived";

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
        .filter(|name| name.ends_with(".toml"))
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

        for (dir, archived) in [(self.board_dir.clone(), false), (self.archived_dir(), true)] {
            let Ok(entries) = std::fs::read_dir(dir.as_std_path()) else {
                continue;
            };
            let mut paths: Vec<Utf8PathBuf> = entries
                .flatten()
                .filter(|entry| entry.path().is_file())
                .filter_map(|entry| Utf8PathBuf::from_path_buf(entry.path()).ok())
                .filter(|path| path.extension() == Some("toml"))
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
        Ok(Some(provider::resolve_task(
            &loaded.documents,
            key,
            archived,
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
            content: new_task.content,
            relations: Relations {
                depends_on: new_task.depends_on,
                parent: new_task.parent,
            },
            steps: Vec::new(),
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

    fn update(&self, key: &str, update: TaskUpdate) -> Result<TaskSummary, WriteError> {
        let loaded = self.load()?;
        let current_path = Self::single_location(&loaded, key)?.clone();
        let mut document = loaded
            .documents
            .get(key)
            .cloned()
            .ok_or_else(|| WriteError::NotFound(key.to_string()))?;
        let was_archived = loaded.archived_keys.contains(key);

        // Cycle checks run against `loaded.documents`, which does not yet
        // carry any of this write's new edges — exactly what
        // `would_create_cycle` needs to answer "would the edge close a
        // loop", and this refuses before anything is written.
        for dep in &update.add_depends_on {
            if let Some(cycle) =
                graph::would_create_cycle(&loaded.documents, EdgeKind::DependsOn, key, dep)
            {
                return Err(cycle.into());
            }
        }
        if let Some(Some(new_parent)) = &update.set_parent
            && let Some(cycle) =
                graph::would_create_cycle(&loaded.documents, EdgeKind::Parent, key, new_parent)
        {
            return Err(cycle.into());
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
        let mut archive_target = was_archived;
        if let Some(status) = update.status {
            document.status = Some(status);
            archive_target = matches!(status, TaskStatus::Done | TaskStatus::Cancelled);
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

        let reloaded = self.load()?;
        Ok(provider::summarize(
            &reloaded.documents,
            key,
            archive_target,
        ))
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

    fn update(&self, key: &str, update: TaskUpdate) -> Result<TaskSummary, WriteError> {
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
        "schema-version = \"0.1\"\nkey = \"0001\"\ntitle = \"First\"\nstatus = \"ready\"\n";

    #[test]
    fn list_hides_archived_by_default_and_get_still_resolves_it() {
        let board_dir = tempdir();
        write_task(&board_dir, "0001-first.toml", TASK_0001);
        std::fs::create_dir_all(board_dir.join(ARCHIVED_DIR).as_std_path()).unwrap();
        write_task(
            &board_dir.join(ARCHIVED_DIR),
            "0002-second.toml",
            "schema-version = \"0.1\"\nkey = \"0002\"\ntitle = \"Second\"\nstatus = \"done\"\n",
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
        let summary = write
            .update(
                "0001",
                TaskUpdate {
                    status: Some(TaskStatus::Done),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(summary.archived);
        assert!(!board_dir.join("0001-first.toml").exists());
        assert!(
            board_dir
                .join(ARCHIVED_DIR)
                .join("0001-first.toml")
                .exists()
        );

        let list = write.list(false).unwrap();
        assert!(list.is_empty());
        let fetched = write.get("0001").unwrap().expect("still resolves archived");
        assert!(fetched.archived);

        // Reopening moves it back.
        let summary = write
            .update(
                "0001",
                TaskUpdate {
                    status: Some(TaskStatus::Ready),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(!summary.archived);
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
            "schema-version = \"0.1\"\nkey = \"0005\"\ntitle = \"Existing\"\nstatus = \"ready\"\n",
        );

        let write = FilesTaskBoard::open_read_write(board_dir.clone());
        let created = write
            .create(NewTask {
                title: "A New Task".to_string(),
                content: String::new(),
                status: None,
                depends_on: Vec::new(),
                parent: None,
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
            })
            .unwrap();
        assert_eq!(second_child.key, "0005.2");
    }

    #[test]
    fn dotted_child_key_resolves_from_archived() {
        let board_dir = tempdir();
        std::fs::create_dir_all(board_dir.join(ARCHIVED_DIR).as_std_path()).unwrap();
        write_task(
            &board_dir.join(ARCHIVED_DIR),
            "0010.1-leaf.toml",
            "schema-version = \"0.1\"\nkey = \"0010.1\"\ntitle = \"Leaf\"\nstatus = \"done\"\n",
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
            "schema-version = \"0.1\"\nkey = \"0110\"\ntitle = \"A\"\nstatus = \"ready\"\n",
        );
        write_task(
            &board_dir,
            "0110-b.toml",
            "schema-version = \"0.1\"\nkey = \"0110\"\ntitle = \"B\"\nstatus = \"ready\"\n",
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
            "schema-version = \"0.1\"\nkey = \"0001\"\ntitle = \"A\"\nstatus = \"ready\"\n",
        );
        write_task(
            &board_dir,
            "0002-b.toml",
            "schema-version = \"0.1\"\nkey = \"0002\"\ntitle = \"B\"\nstatus = \"ready\"\nrelations.depends-on = [\"0001\"]\n",
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
}
