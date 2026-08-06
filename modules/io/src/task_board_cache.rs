//! Persisted task-board snapshot cache (0063.7): the dashboard's TASKS
//! screen opens populated by reading this file rather than showing an empty
//! screen until the first `s` keypress or tick sweep.
//!
//! Derived evidence only, never authority — a record that fails to parse,
//! carries the wrong schema version, or names a different `board_dir` than
//! the caller asked for is discarded (`None`) rather than surfaced as an
//! error. The backend re-read that follows a discard is what makes the
//! cache trustworthy again, not anything this module validates about its
//! contents.

use camino::Utf8Path;
use std::collections::BTreeMap;

use ctx_traits_core::task::provider::{ResolvedTask, SyncReport, TaskSummary};

use crate::task_files::BoardFingerprint;

/// Bumped only when the record shape changes; a stored record at any other
/// version is discarded rather than partially trusted.
const SCHEMA_VERSION: u32 = 1;

const SNAPSHOT_FILENAME: &str = "task-board-snapshot.json";

/// The persisted form of the dashboard's `TasksBoardSnapshot`, plus the
/// fingerprint the tick path compares against and the `board_dir` the
/// record was captured from — a cache for a different board is a miss, not
/// a stale hit.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct BoardSnapshotRecord {
    pub schema_version: u32,
    pub board_dir: String,
    /// Unix epoch seconds — wall-clock, not [`std::time::Instant`], so the
    /// age survives a dashboard restart.
    pub captured_at: u64,
    pub summaries: Vec<TaskSummary>,
    pub resolved: BTreeMap<String, ResolvedTask>,
    pub sync_report: SyncReport,
    pub fingerprint: BoardFingerprint,
}

impl BoardSnapshotRecord {
    pub fn new(
        board_dir: &Utf8Path,
        captured_at: u64,
        summaries: Vec<TaskSummary>,
        resolved: BTreeMap<String, ResolvedTask>,
        sync_report: SyncReport,
        fingerprint: BoardFingerprint,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            board_dir: board_dir.to_string(),
            captured_at,
            summaries,
            resolved,
            sync_report,
            fingerprint,
        }
    }
}

/// Path to the snapshot file under a given cache root. Production callers
/// derive `cache_root` from [`crate::state::global_cache_root`]; tests point
/// this at a scratch directory instead, so no test touches the real config
/// home.
pub fn snapshot_path(cache_root: &Utf8Path) -> camino::Utf8PathBuf {
    cache_root.join(SNAPSHOT_FILENAME)
}

/// Read the persisted snapshot for `board_dir` from `cache_root`, or `None`
/// on anything that makes it untrustworthy: no file, unreadable,
/// unparseable, a schema version mismatch, or a `board_dir` recorded for a
/// different board.
pub fn read_snapshot(cache_root: &Utf8Path, board_dir: &Utf8Path) -> Option<BoardSnapshotRecord> {
    let path = snapshot_path(cache_root);
    let parent = path.parent()?;
    crate::path_safety::ensure_no_symlink_ancestors(parent, "task board snapshot cache ancestor")
        .ok()?;
    if !crate::path_safety::ensure_leaf_is_regular_file_or_absent(&path, "task board snapshot")
        .ok()?
    {
        return None;
    }
    let text = crate::read::read_text(&path).ok()?;
    let record: BoardSnapshotRecord = serde_json::from_str(&text).ok()?;
    if record.schema_version != SCHEMA_VERSION || record.board_dir != board_dir.as_str() {
        return None;
    }
    Some(record)
}

/// Persist `record` atomically (tmp + rename) under `cache_root` so a
/// concurrent dashboard process never reads a torn file. Last-writer-wins is
/// fine: the cache is derived evidence, never authority.
pub fn write_snapshot(cache_root: &Utf8Path, record: &BoardSnapshotRecord) -> crate::Result<()> {
    let path = snapshot_path(cache_root);
    let parent = path.parent().expect("snapshot cache path has parent");
    crate::path_safety::create_dir_all_no_symlinks(parent, "task board snapshot cache ancestor")?;
    crate::path_safety::ensure_leaf_is_regular_file_or_absent(&path, "task board snapshot")?;
    let json = serde_json::to_string_pretty(record).map_err(|source| {
        crate::parse::Error::JsonSerialize {
            context: format!("serialize task board snapshot at {path}"),
            source,
        }
    })?;
    crate::write::write_bytes_atomically(&path, json.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;

    fn scratch_cache_root() -> Utf8PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "task-board-cache-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        Utf8PathBuf::from_path_buf(dir).expect("temp dir path is utf8")
    }

    fn record(board_dir: &Utf8Path) -> BoardSnapshotRecord {
        BoardSnapshotRecord::new(
            board_dir,
            1_000,
            Vec::new(),
            BTreeMap::new(),
            SyncReport::default(),
            BoardFingerprint::default(),
        )
    }

    #[test]
    fn round_trips() {
        let root = scratch_cache_root();
        let board_dir = Utf8PathBuf::from("/tmp/board-a");
        let written = record(&board_dir);
        write_snapshot(&root, &written).unwrap();
        let read = read_snapshot(&root, &board_dir).expect("round trip hit");
        assert_eq!(read, written);
    }

    #[test]
    fn missing_file_is_none() {
        let root = scratch_cache_root();
        let board_dir = Utf8PathBuf::from("/tmp/board-a");
        assert!(read_snapshot(&root, &board_dir).is_none());
    }

    #[test]
    fn corrupt_json_is_none() {
        let root = scratch_cache_root();
        let board_dir = Utf8PathBuf::from("/tmp/board-a");
        let path = snapshot_path(&root);
        std::fs::create_dir_all(path.parent().unwrap().as_std_path()).unwrap();
        std::fs::write(path.as_std_path(), b"not json").unwrap();
        assert!(read_snapshot(&root, &board_dir).is_none());
    }

    #[test]
    fn mismatched_board_dir_is_none() {
        let root = scratch_cache_root();
        let board_dir = Utf8PathBuf::from("/tmp/board-a");
        let other_dir = Utf8PathBuf::from("/tmp/board-b");
        write_snapshot(&root, &record(&board_dir)).unwrap();
        assert!(read_snapshot(&root, &other_dir).is_none());
    }

    #[test]
    fn mismatched_schema_version_is_none() {
        let root = scratch_cache_root();
        let board_dir = Utf8PathBuf::from("/tmp/board-a");
        let mut stale = record(&board_dir);
        stale.schema_version = SCHEMA_VERSION + 1;
        write_snapshot(&root, &stale).unwrap();
        assert!(read_snapshot(&root, &board_dir).is_none());
    }
}
