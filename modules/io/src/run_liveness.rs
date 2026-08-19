//! Local liveness index: a pointer index a driver upserts on start and
//! removes on clean exit, so "what is running, machine-wide" is one small
//! read instead of parsing every ledger on every store.
//!
//! Lives at `<runtime_root>/live-index.toml`, the same per-user LOCAL-disk
//! directory [`crate::run_control::runtime_root`] already uses for control
//! sockets — `flock` is reliable there (unlike NFS), a reboot clears it, and
//! a copied/moved ledger store can never inherit stale liveness. If that
//! root cannot be resolved, created, or ownership-checked, every liveness
//! answer here is [`Liveness::Unknown`] — never [`Liveness::NotRunning`] and
//! never fabricated as dead.
//!
//! **Liveness is the lock, not the row** (contract (b)): the row is a
//! breadcrumb designed to survive the process that wrote it; the kernel
//! `flock` on the sibling driver-lock file (see [`crate::run_control`]) is
//! the oracle, since the kernel drops it on `kill -9` even when nothing else
//! ran cleanup. A row present with the lock free is an orphan — the row
//! names exactly what a later cleanup (P511/P512) should look at.
//!
//! This index is a pointer index like `checkouts.toml`
//! ([`crate::state::touch_repo_index`]), never a payload cache: no session
//! status, no token counts, nothing that could drift from — and quietly
//! start lying about — the ledger it points at.

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

/// One session's pointer row in the liveness index. Flat and small by
/// design (contract: "never a payload cache").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveIndexRow {
    pub session_id: String,
    pub run_id: String,
    pub repo_key: String,
    pub repo_path: String,
    pub ledger_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_path: Option<String>,
    pub started_at_epoch: u64,
    pub pid: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct LiveIndexDocument {
    #[serde(rename = "run", default, skip_serializing_if = "Vec::is_empty")]
    runs: Vec<LiveIndexRow>,
}

/// Facts a driver supplies once it holds the ledger's driver lock, bundled
/// into a single argument for [`crate::run_control::try_acquire`] rather
/// than a growing list of independent parameters — the same precedent as
/// `DriveTerminalEvidence` (`ctx_traits_core::procedure::session`).
#[derive(Debug, Clone)]
pub struct LiveRunFacts {
    pub session_id: String,
    pub run_id: String,
    pub repo_key: String,
    pub repo_path: String,
    pub ledger_path: Utf8PathBuf,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub log_path: Option<String>,
}

/// One typed liveness answer (contract (b)'s table), resolved per row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Liveness {
    /// Row present, lock held: a live, index-described driver.
    Live { row: LiveIndexRow },
    /// Row present, lock free: the driver crashed (or was `kill -9`'d)
    /// without clearing its row. The row names exactly what to clean.
    Orphan { row: LiveIndexRow },
    /// Row absent, lock held: an externally started or row-less driver.
    /// Still genuinely live — display normally.
    Adopted,
    /// Row absent, lock free: nothing attached to this ledger.
    NotRunning,
    /// The runtime root could not be resolved, created, or ownership
    /// checked. Never collapse this to `NotRunning` — an unreadable index is
    /// not evidence of anything being dead.
    Unknown,
}

/// Environment variable a detached dashboard spawn (RESUME or
/// spawn-request) uses to hand its session-keyed log path to the child
/// driver, which reads it once into its own [`LiveRunFacts::log_path`]. Spawned
/// run log paths were previously named `dashboard-resume-<pid>.log` —
/// unresolvable from a session id — so this is the one shared mechanism both
/// dashboard spawn call sites use instead of each hand-rolling their own
/// exe/cwd/log-path construction.
pub const SPAWNED_LOG_PATH_ENV: &str = "CTX_TRAITS_SPAWNED_LOG_PATH";

fn index_path(root: &Utf8Path) -> Utf8PathBuf {
    root.join("live-index.toml")
}

fn read_document(root: &Utf8Path) -> crate::Result<LiveIndexDocument> {
    let path = index_path(root);
    match std::fs::read_to_string(path.as_std_path()) {
        Ok(text) => toml::from_str(&text).map_err(|source| {
            crate::parse::Error::TomlDecode {
                context: format!(
                    "decode liveness index at {path}; fix or remove {path} to recover"
                ),
                source,
            }
            .into()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LiveIndexDocument::default()),
        Err(source) => Err(crate::environment::Error::Filesystem {
            path: path.to_string(),
            source,
        }
        .into()),
    }
}

fn write_document(root: &Utf8Path, document: &LiveIndexDocument) -> crate::Result<()> {
    let path = index_path(root);
    let mut sorted = document.clone();
    sorted.runs.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    let text =
        toml::to_string_pretty(&sorted).map_err(|source| crate::parse::Error::TomlEncode {
            context: format!("encode liveness index at {path}"),
            source,
        })?;
    crate::run_session::write_text_atomically(&path, &text)
}

/// Rows whose ledger file no longer exists are stale breadcrumbs — a session
/// that was pruned, or whose store moved. Self-healed here (write time only;
/// see module doc / contract), never by a reader.
fn drop_rows_with_missing_ledgers(document: &mut LiveIndexDocument) {
    document
        .runs
        .retain(|row| Utf8Path::new(&row.ledger_path).is_file());
}

/// Upsert one session's row under `root`'s exclusive lock, following exactly
/// [`crate::state::touch_repo_index`]'s shape: sibling `.lock` file, blocking
/// `flock`, read-modify-write, atomic rename. Also self-heals: drops any
/// other row whose ledger has since disappeared, since this call already
/// holds the lock those removals need.
pub fn upsert_row(
    root: &Utf8Path,
    facts: &LiveRunFacts,
    pid: u32,
    started_at_epoch: u64,
) -> crate::Result<()> {
    ensure_root_writable(root)?;
    with_index_lock(root, |document| {
        drop_rows_with_missing_ledgers(document);
        let row = LiveIndexRow {
            session_id: facts.session_id.clone(),
            run_id: facts.run_id.clone(),
            repo_key: facts.repo_key.clone(),
            repo_path: facts.repo_path.clone(),
            ledger_path: facts.ledger_path.to_string(),
            worktree_path: facts.worktree_path.clone(),
            branch: facts.branch.clone(),
            log_path: facts.log_path.clone(),
            started_at_epoch,
            pid,
        };
        match document
            .runs
            .iter_mut()
            .find(|existing| existing.session_id == facts.session_id)
        {
            Some(existing) => *existing = row,
            None => document.runs.push(row),
        }
        Ok(())
    })
}

/// Remove one session's row under `root`'s exclusive lock. Best-effort by
/// design at every call site: a removal failure must never fail a clean
/// driver exit, and a row left behind by a failed removal is exactly the
/// orphan case this whole index already has to handle.
pub fn remove_row(root: &Utf8Path, session_id: &str) -> crate::Result<()> {
    ensure_root_writable(root)?;
    with_index_lock(root, |document| {
        document.runs.retain(|row| row.session_id != session_id);
        drop_rows_with_missing_ledgers(document);
        Ok(())
    })
}

/// Read every row currently in the index. A read never mutates the file —
/// self-healing only ever happens inside [`upsert_row`]/[`remove_row`],
/// which already hold the write lock.
///
/// Returns `Err` when the runtime root cannot be resolved, created, or
/// ownership-checked — a genuinely unavailable root, distinct from "root
/// available, index empty" (`Ok(Vec::new())`, the ordinary case for an
/// index that has never been written to, since [`ensure_root_writable`]
/// creates a missing root rather than failing on it). Callers that must
/// degrade to [`Liveness::Unknown`] on `Err` do so explicitly — this
/// function never collapses the distinction itself (contract (a): "no local
/// runtime path obtainable ⇒ liveness is UNKNOWN, never dead").
pub fn read_index(root: &Utf8Path) -> crate::Result<Vec<LiveIndexRow>> {
    ensure_root_writable(root)?;
    Ok(read_document(root)?.runs)
}

fn ensure_root_writable(root: &Utf8Path) -> crate::Result<()> {
    crate::run_control::ensure_runtime_root(root)
}

fn with_index_lock(
    root: &Utf8Path,
    mutate: impl FnOnce(&mut LiveIndexDocument) -> crate::Result<()>,
) -> crate::Result<()> {
    std::fs::create_dir_all(root.as_std_path()).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: root.to_string(),
            source,
        }
    })?;
    let lock_path = root.join("live-index.toml.lock");
    let lock_file = crate::file_lock::open_lock_file_no_follow(&lock_path).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source,
        }
    })?;
    crate::file_lock::lock_exclusive_blocking(&lock_file).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source,
        }
    })?;
    // `lock_file`'s exclusive flock releases when it drops at the end of
    // this function, after the atomic rename below has landed.
    let mut document = read_document(root)?;
    mutate(&mut document)?;
    write_document(root, &document)
}

/// Classify liveness for one row (or its absence) against the driver lock
/// sibling to `ledger_path`. `existing_row` is `None` when the index has no
/// row for this session — the caller (a full sweep) probes the lock anyway
/// to catch a row-less held lock (adoption, contract (b)).
pub fn classify(existing_row: Option<LiveIndexRow>, ledger_path: &Utf8Path) -> Liveness {
    let held = matches!(
        crate::run_control::probe(ledger_path),
        Ok(crate::run_control::DriverProbe::Held(_))
    );
    match (existing_row, held) {
        (Some(row), true) => Liveness::Live { row },
        (Some(row), false) => Liveness::Orphan { row },
        (None, true) => Liveness::Adopted,
        (None, false) => Liveness::NotRunning,
    }
}

/// Resolve liveness for every row currently in the index under the default
/// runtime root. An unavailable root reports as the single sentinel entry
/// `(None, Liveness::Unknown)` — never as an empty `Vec`, which is
/// indistinguishable from "root available, nothing running" and is exactly
/// the false-dead answer contract (a) forbids. Callers must check for that
/// sentinel before treating an empty-looking report as "no rows".
pub fn liveness_report(root: &Utf8Path) -> Vec<(Option<LiveIndexRow>, Liveness)> {
    let rows = match read_index(root) {
        Ok(rows) => rows,
        Err(_) => return vec![(None, Liveness::Unknown)],
    };
    rows.into_iter()
        .map(|row| {
            let ledger_path = Utf8PathBuf::from(row.ledger_path.clone());
            let liveness = classify(Some(row.clone()), &ledger_path);
            (Some(row), liveness)
        })
        .collect()
}

/// One sighting of a session that appears to be running the exact bytes
/// under `trust --blocked` review — read-only, best-effort evidence rendered in
/// the block report (P534 review blocker 2), never authority: it never
/// gates or stops the block write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedDigestSighting {
    pub session_id: String,
    pub run_id: String,
}

/// Sessions that appear live under the default runtime root
/// ([`crate::run_control::runtime_root`]) whose ledger's start-time trust
/// pin (`Provenance.trust_approval.canonical_digest`) matches `digest`. An
/// unreadable ledger, an unresolvable runtime root, or a session with no
/// pin is silently skipped — this reports sightings, never authority, and
/// must never fail the `trust --blocked` write it accompanies.
pub fn live_sessions_pinned_to_digest(digest: &str) -> Vec<BlockedDigestSighting> {
    let root = crate::run_control::runtime_root();
    liveness_report(&root)
        .into_iter()
        .filter_map(|(row, liveness)| {
            if !matches!(liveness, Liveness::Live { .. }) {
                return None;
            }
            let row = row?;
            let session =
                crate::run_session::read_run_session(&Utf8PathBuf::from(row.ledger_path.clone()))
                    .ok()?;
            let pinned = session.provenance.trust_approval.as_ref()?;
            if pinned.canonical_digest.as_str() != digest {
                return None;
            }
            Some(BlockedDigestSighting {
                session_id: row.session_id,
                run_id: row.run_id,
            })
        })
        .collect()
}

/// Session ids the index currently has a row for, under the default runtime
/// root. `None` when the root cannot be resolved — distinct from `Some`
/// with an empty set, since a caller that bounds its probing to this set
/// must fail OPEN (probe everything, as if every tick were a full sweep) on
/// `None` rather than fail closed on an empty set, which would silently
/// render every live session as not-live. Used by the dashboard to bound
/// its per-tick probe set to "the index rows only" (§3.3): a row-less held
/// lock (adoption) is instead caught by a slower-cadence full sweep the
/// caller drives itself, not by this function.
pub fn indexed_session_ids(root: &Utf8Path) -> Option<std::collections::HashSet<String>> {
    read_index(root)
        .ok()
        .map(|rows| rows.into_iter().map(|row| row.session_id).collect())
}

pub(crate) fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_root(name: &str) -> Utf8PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("temp dir is UTF-8")
            .join(format!(
                "ctx-run-liveness-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
        let _ = std::fs::remove_dir_all(dir.as_std_path());
        std::fs::create_dir_all(dir.as_std_path()).expect("create scratch root");
        std::fs::set_permissions(dir.as_std_path(), std::fs::Permissions::from_mode(0o700))
            .expect("restrict scratch root to its owner");
        dir
    }

    fn write_fake_ledger(dir: &Utf8Path, session_id: &str) -> Utf8PathBuf {
        let path = dir.join(format!("{session_id}.json"));
        std::fs::write(path.as_std_path(), "{}").expect("write fake ledger");
        path
    }

    fn facts(session_id: &str, ledger_path: Utf8PathBuf) -> LiveRunFacts {
        LiveRunFacts {
            session_id: session_id.to_string(),
            run_id: format!("run-{session_id}"),
            repo_key: "repo-key".to_string(),
            repo_path: "/repo".to_string(),
            ledger_path,
            worktree_path: Some("/repo/.worktrees/wt".to_string()),
            branch: Some("ctx/run/wt".to_string()),
            log_path: Some("/tmp/log".to_string()),
        }
    }

    #[test]
    fn round_trip_upsert_read_remove() {
        let root = scratch_root("round-trip");
        let ledgers = scratch_root("round-trip-ledgers");
        let path_a = write_fake_ledger(&ledgers, "session-a");
        let path_b = write_fake_ledger(&ledgers, "session-b");

        upsert_row(&root, &facts("session-a", path_a.clone()), 111, 1000).expect("upsert a");
        upsert_row(&root, &facts("session-b", path_b.clone()), 222, 2000).expect("upsert b");
        let rows = read_index(&root).expect("read");
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .any(|r| r.session_id == "session-a" && r.pid == 111)
        );
        assert!(
            rows.iter()
                .any(|r| r.session_id == "session-b" && r.pid == 222)
        );

        remove_row(&root, "session-a").expect("remove a");
        let rows = read_index(&root).expect("read after remove");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "session-b");
    }

    #[test]
    fn upsert_self_heals_rows_with_missing_ledgers() {
        let root = scratch_root("self-heal");
        let ledgers = scratch_root("self-heal-ledgers");
        let path_stale = ledgers.join("session-stale.json");
        // Never written: this row's ledger never existed.
        upsert_row(&root, &facts("session-stale", path_stale), 111, 1000)
            .expect("upsert stale row directly (ledger absent)");
        let path_fresh = write_fake_ledger(&ledgers, "session-fresh");
        upsert_row(&root, &facts("session-fresh", path_fresh), 222, 2000)
            .expect("upsert fresh row triggers self-heal");
        let rows = read_index(&root).expect("read");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "session-fresh");
    }

    #[test]
    fn read_never_mutates_the_index_file() {
        let root = scratch_root("read-only");
        let ledgers = scratch_root("read-only-ledgers");
        let path = write_fake_ledger(&ledgers, "session-a");
        upsert_row(&root, &facts("session-a", path), 111, 1000).expect("upsert");
        let index_file = index_path(&root);
        let before = std::fs::read(index_file.as_std_path()).expect("read bytes before");
        let before_mtime = std::fs::metadata(index_file.as_std_path())
            .expect("metadata before")
            .modified()
            .expect("mtime before");
        let _ = read_index(&root).expect("read");
        let _ = read_index(&root).expect("read again");
        let after = std::fs::read(index_file.as_std_path()).expect("read bytes after");
        let after_mtime = std::fs::metadata(index_file.as_std_path())
            .expect("metadata after")
            .modified()
            .expect("mtime after");
        assert_eq!(before, after, "reading must not change index file bytes");
        assert_eq!(
            before_mtime, after_mtime,
            "reading must not touch index file mtime"
        );
    }

    #[test]
    fn classify_covers_the_four_row_lock_combinations() {
        let ledgers = scratch_root("classify-ledgers");
        let ledger_path = write_fake_ledger(&ledgers, "session-a");
        let row = LiveIndexRow {
            session_id: "session-a".to_string(),
            run_id: "run-a".to_string(),
            repo_key: "repo-key".to_string(),
            repo_path: "/repo".to_string(),
            ledger_path: ledger_path.to_string(),
            worktree_path: None,
            branch: None,
            log_path: None,
            started_at_epoch: 1000,
            pid: 111,
        };
        // No driver lock file exists yet: lock is free (`Unheld`).
        assert!(matches!(
            classify(Some(row.clone()), &ledger_path),
            Liveness::Orphan { .. }
        ));
        assert!(matches!(classify(None, &ledger_path), Liveness::NotRunning));

        let held = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let guard = crate::run_control::try_acquire(
            &facts("session-a", ledger_path.clone()),
            std::sync::Arc::new({
                let held = held.clone();
                move || held.store(true, std::sync::atomic::Ordering::SeqCst)
            }),
        )
        .expect("acquire driver lock")
        .expect("lock uncontended");

        assert!(matches!(
            classify(Some(row.clone()), &ledger_path),
            Liveness::Live { .. }
        ));
        assert!(matches!(classify(None, &ledger_path), Liveness::Adopted));

        drop(guard);
    }

    // An unavailable runtime root must report `Unknown`,
    // never a fabricated `NotRunning`/dead answer collapsed from an empty
    // collection.
    #[test]
    fn unavailable_root_reports_unknown_not_empty() {
        let root = scratch_root("unavailable");
        // A plain file where the runtime root should be a directory fails
        // `ensure_runtime_root`'s ownership/type check every time.
        std::fs::remove_dir_all(root.as_std_path()).expect("clear scratch dir");
        std::fs::write(root.as_std_path(), b"not a directory").expect("plant a blocking file");

        assert!(
            read_index(&root).is_err(),
            "an unavailable root is an error, not Ok(vec![])"
        );
        assert_eq!(
            liveness_report(&root),
            vec![(None, Liveness::Unknown)],
            "the report sentinel must be the Unknown variant, not an empty Vec"
        );
        assert_eq!(
            indexed_session_ids(&root),
            None,
            "indexed_session_ids must be None (fail open), not Some(empty set) (fail closed)"
        );
    }

    #[test]
    fn group_or_other_readable_root_is_unavailable() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch_root("insecure-permissions");
        std::fs::set_permissions(root.as_std_path(), std::fs::Permissions::from_mode(0o755))
            .expect("make root group and other readable");

        assert!(read_index(&root).is_err());
        assert_eq!(
            liveness_report(&root),
            vec![(None, Liveness::Unknown)],
            "an insecure runtime root must never answer not-running"
        );
    }
}
