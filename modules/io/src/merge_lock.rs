//! Cross-process lock serializing `ctx traits merge` at the repository
//! boundary. Several `just implement`/`just implement-lean` batches can
//! finish independent runs at nearly the same moment; without mutual
//! exclusion, two concurrent merges race `main` and the second either parks
//! on a moved base or trips a raw Git `index.lock`. This module turns that
//! race into a bounded wait.
//!
//! Locking is a kernel `flock(2)` held on an opened `merge.lock` file inside
//! the invocation repository's global per-repository runs root
//! (`~/.config/ctx/runs/<repo-key>/`, P426), not create/delete locking:
//! kernel ownership makes acquisition atomic
//! and automatically releases a crashed holder's lock the moment its process
//! exits, with no unsafe stale-file unlink race. The file's path stays stable
//! across acquisitions — only the holder metadata inside it is cleared on
//! release — so every contender flocks the same inode.
//!
//! Holder metadata (pid + run-id) is best-effort operational evidence, not an
//! ownership mechanism: the kernel flock is the sole authority for who holds
//! the lock. A dead holder's leftover metadata (crash before its own release
//! ran) is reclaimed with a warning once *this* process has genuinely
//! acquired the kernel lock — never by inspecting or removing the file
//! up front.

use camino::{Utf8Path, Utf8PathBuf};
use std::time::{Duration, Instant};

const POLL_INTERVAL_MS: u64 = 250;

/// How long [`acquire`] is willing to poll for a contended lock. The bound
/// itself is the caller's responsibility to size: `modules/io` has no
/// visibility into merger/gate call budgets, so it must not hard-code a
/// timeout that could silently drift from them — the caller derives a value
/// that actually covers its own worst-case holder lifetime.
pub enum Wait {
    /// Fail fast (`Busy`) instead of polling.
    NoWait,
    /// Poll until acquired or this bound elapses (`TimedOut`). Never
    /// infinite — a wedged holder must not deadlock every other batch.
    Bounded(Duration),
}

/// Observed holder of the lock: either the current holder of a contended
/// lock (best-effort, read without holding the lock) or a prior holder whose
/// metadata was left behind by a crash and reclaimed on acquisition.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HolderInfo {
    pub pid: u32,
    pub run_id: String,
}

/// Operational evidence for a successful acquisition: wait duration, the
/// holder observed while queued behind a contended lock (if any wait
/// happened), and, when the previous holder's metadata was left behind by a
/// crash, that holder's identity. Never canonical/digested state — see
/// [`ctx_traits_core::procedure::session::MergeStatus::LockAcquired`].
#[derive(Debug, Clone)]
pub struct LockEvidence {
    pub wait_ms: u64,
    pub queued_behind: Option<HolderInfo>,
    pub stale_holder_reclaimed: Option<HolderInfo>,
}

/// Outcome of [`acquire`].
pub enum AcquireOutcome {
    Acquired {
        guard: MergeLockGuard,
        evidence: LockEvidence,
    },
    /// `wait = false` and the lock was already held.
    Busy { holder: Option<HolderInfo> },
    /// `wait = true` and the bounded default timeout elapsed without
    /// acquiring the lock.
    TimedOut {
        waited_ms: u64,
        holder: Option<HolderInfo>,
    },
}

/// RAII guard for a held merge lock. Holder metadata is cleared (while still
/// locked) on drop; the lock file itself is left in place so every future
/// contender flocks the same stable inode. Covers every exit path of the
/// caller — success, park, timeout/error propagation, and cleanup failure —
/// since `Drop` runs regardless of how the caller's function returns.
pub struct MergeLockGuard {
    file: std::fs::File,
}

impl Drop for MergeLockGuard {
    fn drop(&mut self) {
        // Best effort: a Drop impl cannot propagate failure, and the kernel
        // flock is released on fd close regardless of whether the metadata
        // clear below succeeds.
        let _ = crate::file_lock::clear_lock_metadata(&mut self.file);
    }
}

fn lock_path(repo_root: &Utf8Path) -> crate::Result<Utf8PathBuf> {
    let key = crate::state::repo_key(&crate::state::canonical_repo_root(repo_root)?);
    Ok(crate::state::global_runs_root(&key)?.join("merge.lock"))
}

/// Acquire the cross-process merge lock. [`Wait::NoWait`] fails fast
/// (`Busy`) instead of polling; [`Wait::Bounded`] polls at a fixed short
/// interval up to the given bound and returns `TimedOut` if it is exhausted.
pub fn acquire(
    repo_root: &Utf8Path,
    holder_run_id: &str,
    wait: Wait,
) -> crate::Result<AcquireOutcome> {
    let path = lock_path(repo_root)?;
    let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| crate::environment::Error::Filesystem {
        path: parent.to_string(),
        source: e,
    })?;
    let mut file = crate::file_lock::open_lock_file_no_follow(&path).map_err(|e| {
        crate::environment::Error::Filesystem {
            path: path.to_string(),
            source: e,
        }
    })?;

    let start = Instant::now();
    // Last holder observed while contended, so a successful queued
    // acquisition can report who it waited behind — the prior holder clears
    // its metadata under lock on a clean release, so this must be captured
    // during the wait itself, not read back afterward.
    let mut queued_behind: Option<HolderInfo> = None;
    loop {
        match crate::file_lock::try_lock_exclusive(&file) {
            Ok(true) => break,
            Ok(false) => {
                if let Some(holder) = crate::file_lock::read_lock_metadata(&mut file) {
                    queued_behind = Some(holder);
                }
                let timeout = match wait {
                    Wait::NoWait => {
                        return Ok(AcquireOutcome::Busy {
                            holder: queued_behind,
                        });
                    }
                    Wait::Bounded(timeout) => timeout,
                };
                let waited = start.elapsed();
                if waited >= timeout {
                    return Ok(AcquireOutcome::TimedOut {
                        waited_ms: waited.as_millis() as u64,
                        holder: queued_behind,
                    });
                }
                std::thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
            }
            Err(e) => {
                return Err(crate::environment::Error::Filesystem {
                    path: path.to_string(),
                    source: e,
                }
                .into());
            }
        }
    }

    // The kernel flock is now held exclusively: any metadata still present is
    // either empty (clean prior release) or was left behind by a holder that
    // crashed before its own `Drop` ran. Treat the latter as stale once
    // `kill(pid, 0)` confirms the pid is dead — the kernel lock, not the
    // metadata, is the authority on ownership; liveness only classifies
    // leftover metadata for reporting.
    let leftover: Option<HolderInfo> = crate::file_lock::read_lock_metadata(&mut file);
    let stale_holder_reclaimed =
        leftover.filter(|holder| !crate::file_lock::pid_is_alive(holder.pid));
    let wait_ms = start.elapsed().as_millis() as u64;

    crate::file_lock::write_lock_metadata(
        &mut file,
        &HolderInfo {
            pid: std::process::id(),
            run_id: holder_run_id.to_string(),
        },
    )
    .map_err(|e| crate::environment::Error::Filesystem {
        path: path.to_string(),
        source: e,
    })?;

    Ok(AcquireOutcome::Acquired {
        guard: MergeLockGuard { file },
        evidence: LockEvidence {
            wait_ms,
            queued_behind,
            stale_holder_reclaimed,
        },
    })
}
