//! Persisted dashboard SESSIONS and MERGES rows.
//!
//! This is derived evidence for a warm first frame, never authority: any
//! unreadable or unsafe cache file is a silent miss and live data replaces a
//! valid record on the next worker snapshot.

use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};

use super::{MergeRow, SessionRow};

const SCHEMA_VERSION: u32 = 1;
const SNAPSHOT_FILENAME: &str = "sessions-snapshot.json";

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct SessionsSnapshotRecord {
    pub(super) schema_version: u32,
    /// Unix epoch seconds, retained across dashboard restarts.
    pub(super) captured_at: u64,
    pub(super) repo_key: String,
    pub(super) all_repos: bool,
    pub(super) sessions: Vec<SessionRow>,
    pub(super) merges: Vec<MergeRow>,
}

impl SessionsSnapshotRecord {
    pub(super) fn new(
        captured_at: u64,
        repo_key: String,
        all_repos: bool,
        sessions: Vec<SessionRow>,
        merges: Vec<MergeRow>,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            captured_at,
            repo_key,
            all_repos,
            sessions,
            merges,
        }
    }
}

pub(super) fn snapshot_path(cache_root: &Utf8Path) -> Utf8PathBuf {
    cache_root.join(SNAPSHOT_FILENAME)
}

/// Returns `None` when the cache cannot be trusted. The caller has no reason
/// to distinguish absent, stale-schema, malformed, unreadable, or unsafe data.
pub(super) fn read_snapshot(cache_root: &Utf8Path) -> Option<SessionsSnapshotRecord> {
    let path = snapshot_path(cache_root);
    let parent = path.parent()?;
    ctx_traits_io::path_safety::ensure_no_symlink_ancestors(
        parent,
        "sessions snapshot cache ancestor",
    )
    .ok()?;
    if !ctx_traits_io::path_safety::ensure_leaf_is_regular_file_or_absent(
        &path,
        "sessions snapshot",
    )
    .ok()?
    {
        return None;
    }
    let text = ctx_traits_io::read::read_text(&path).ok()?;
    let record = serde_json::from_str::<SessionsSnapshotRecord>(&text).ok()?;
    (record.schema_version == SCHEMA_VERSION).then_some(record)
}

pub(super) fn write_snapshot(
    cache_root: &Utf8Path,
    record: &SessionsSnapshotRecord,
) -> ctx_traits_io::Result<()> {
    let path = snapshot_path(cache_root);
    let parent = path.parent().expect("snapshot cache path has parent");
    ctx_traits_io::path_safety::create_dir_all_no_symlinks(
        parent,
        "sessions snapshot cache ancestor",
    )?;
    ctx_traits_io::path_safety::ensure_leaf_is_regular_file_or_absent(&path, "sessions snapshot")?;
    let json = serde_json::to_string_pretty(record).map_err(|source| {
        ctx_traits_io::parse::Error::JsonSerialize {
            context: format!("serialize sessions snapshot at {path}"),
            source,
        }
    })?;
    ctx_traits_io::write::write_bytes_atomically(&path, json.as_bytes())
}

/// Pure minimum-interval policy for worker-owned cache writes.
pub(super) struct WriteDebounce {
    minimum_interval: Duration,
    last_write: Option<Instant>,
}

impl WriteDebounce {
    pub(super) fn new(minimum_interval: Duration) -> Self {
        Self {
            minimum_interval,
            last_write: None,
        }
    }

    pub(super) fn admits(&mut self, now: Instant, changed: bool) -> bool {
        let within_interval = self.last_write.is_some_and(|last| {
            now.checked_duration_since(last)
                .is_none_or(|elapsed| elapsed < self.minimum_interval)
        });
        if !changed || within_interval {
            return false;
        }
        self.last_write = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::{MergeClass, SessionClass};
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn scratch_cache_root() -> Utf8PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sessions-cache-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        Utf8PathBuf::from_path_buf(dir).expect("temp dir path is utf8")
    }

    fn record() -> SessionsSnapshotRecord {
        SessionsSnapshotRecord::new(
            1_000,
            "repo-a".to_string(),
            false,
            vec![super::super::tests::row(SessionClass::Resumable)],
            vec![super::super::tests::merges_test_row(
                "s1",
                MergeClass::Mergeable,
            )],
        )
    }

    #[test]
    fn round_trips_non_empty_rows() {
        let root = scratch_cache_root();
        let written = record();
        write_snapshot(&root, &written).unwrap();

        let read = read_snapshot(&root).expect("round trip hit");
        assert_eq!(read.captured_at, written.captured_at);
        assert_eq!(read.repo_key, written.repo_key);
        assert!(!read.sessions.is_empty());
        assert!(!read.merges.is_empty());
        assert_eq!(read.sessions[0].session_id, "s1");
        assert_eq!(read.merges[0].session_id, "s1");
    }

    #[test]
    fn absent_corrupt_and_wrong_schema_are_misses() {
        let root = scratch_cache_root();
        assert!(read_snapshot(&root).is_none());

        let path = snapshot_path(&root);
        std::fs::create_dir_all(path.parent().unwrap().as_std_path()).unwrap();
        std::fs::write(path.as_std_path(), b"not json").unwrap();
        assert!(read_snapshot(&root).is_none());

        let mut stale = record();
        stale.schema_version += 1;
        write_snapshot(&root, &stale).unwrap();
        assert!(read_snapshot(&root).is_none());
    }

    #[test]
    fn non_regular_leaf_is_a_miss() {
        let root = scratch_cache_root();
        let path = snapshot_path(&root);
        std::fs::create_dir_all(path.as_std_path()).unwrap();
        assert!(read_snapshot(&root).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_snapshot_is_a_miss() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch_cache_root();
        write_snapshot(&root, &record()).unwrap();
        let path = snapshot_path(&root);
        std::fs::set_permissions(path.as_std_path(), std::fs::Permissions::from_mode(0o000))
            .unwrap();
        assert!(read_snapshot(&root).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_snapshot_is_a_miss() {
        use std::os::unix::fs::symlink;

        let root = scratch_cache_root();
        let path = snapshot_path(&root);
        std::fs::create_dir_all(path.parent().unwrap().as_std_path()).unwrap();
        let target = root.join("target.json");
        std::fs::write(target.as_std_path(), b"{}").unwrap();
        symlink(target.as_std_path(), path.as_std_path()).unwrap();
        assert!(read_snapshot(&root).is_none());
    }

    #[test]
    fn debounce_requires_changed_rows_and_minimum_interval() {
        let start = Instant::now();
        let mut debounce = WriteDebounce::new(Duration::from_secs(2));
        assert!(!debounce.admits(start, false));
        assert!(debounce.admits(start, true));
        assert!(!debounce.admits(start + Duration::from_secs(1), true));
        assert!(debounce.admits(start + Duration::from_secs(2), true));
    }
}
