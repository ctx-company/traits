//! `<ledger>.summary.json` sidecar: a small, mtime+size-gated projection
//! of a session ledger carrying the handful of facts a dashboard tick needs
//! (status, phase, elapsed, exit code, worktree identity) without parsing
//! the ledger's full frame history.
//!
//! Derived evidence only, never authority: a missing or unparseable sidecar,
//! or one whose mtime trails its ledger, always falls back to parsing the
//! ledger itself (see [`read_summary_or_ledger`]). Written inside the same
//! atomic-rename boundary as the ledger write, immediately after it, so a
//! sidecar's mtime is never older than its ledger's except across a crash
//! between the two writes — exactly the case the freshness rule exists for.

use camino::{Utf8Path, Utf8PathBuf};
use ctx_traits_core::procedure::activity::SessionState;
use ctx_traits_core::procedure::session::{MergeStatus, Session, Status};
use serde::{Deserialize, Serialize};

/// `MergeStatus`'s own `#[serde(rename_all = "kebab-case")]` name, not a
/// second hand-rolled status table — the MERGES pane's Watch clause (P506)
/// bans baking `Debug` formatting into a persisted or displayed artifact.
fn merge_status_str(status: MergeStatus) -> String {
    match serde_json::to_value(status) {
        Ok(serde_json::Value::String(s)) => s,
        _ => unreachable!("MergeStatus serializes to a plain string"),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSummary {
    pub session_id: String,
    pub run_id: String,
    pub trait_id: String,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_state: Option<SessionState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_drive_outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narrator_tokens: Option<u64>,
    /// Whether the ledger's most recent `merge_frames` entry exists at all —
    /// lets the MERGES projection skip ledgers with nothing to show without
    /// reading the ledger.
    pub has_merge_frames: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_merge_status: Option<String>,
}

impl RunSummary {
    /// Project a summary from an already-parsed session. Pure — no IO.
    pub fn from_session(session: &Session) -> RunSummary {
        let last_merge = session.provenance.merge_frames.last();
        RunSummary {
            session_id: session.session_id.as_str().to_string(),
            run_id: session.run_id.as_str().to_string(),
            trait_id: session.trait_id.clone(),
            status: session.status.clone(),
            session_state: Some(SessionState::derive(
                &session.status,
                session
                    .last_drive_outcome
                    .as_ref()
                    .map(|outcome| &outcome.outcome),
                false,
            )),
            phase: crate::run_session::session_phase(session),
            started_at_epoch: session.provenance.started_at_epoch,
            exit_code: session
                .last_drive_outcome
                .as_ref()
                .and_then(|outcome| outcome.exit_code),
            worktree_id: session.provenance.worktree.as_ref().map(|w| w.id.clone()),
            worktree_branch: session
                .provenance
                .worktree
                .as_ref()
                .map(|w| w.branch.clone()),
            worktree_path: session
                .provenance
                .worktree
                .as_ref()
                .and_then(|w| w.path.clone()),
            last_drive_outcome: session
                .last_drive_outcome
                .as_ref()
                .map(|outcome| outcome.outcome.as_str().to_string()),
            work_tokens: session
                .last_drive_outcome
                .as_ref()
                .and_then(|outcome| outcome.token_usage.as_ref())
                .and_then(|usage| usage.work_tokens),
            narrator_tokens: session
                .last_drive_outcome
                .as_ref()
                .and_then(|outcome| outcome.token_usage.as_ref())
                .and_then(|usage| usage.narrator_tokens),
            has_merge_frames: last_merge.is_some(),
            last_merge_status: last_merge.map(|frame| merge_status_str(frame.status)),
        }
    }
}

/// Sidecar path for a ledger: `<ledger>.json` -> `<ledger>.json.summary.json`.
/// Deliberately matches the contract's naming exactly (`<ledger>.summary.json`)
/// so [`crate::run_session::session_ledger_names`]'s explicit skip filter for
/// this suffix stays correct — see that function's doc comment for why the
/// filter must exist at all.
pub fn summary_path(ledger_path: &Utf8Path) -> Utf8PathBuf {
    let mut path = ledger_path.as_str().to_string();
    path.push_str(".summary.json");
    Utf8PathBuf::from(path)
}

/// Write a summary sidecar, atomically, best-effort: a write failure here is
/// derived-evidence loss, never a ledger-write failure, so callers swallow
/// the error (matching contract §3.4's "written... a write failure is
/// swallowed").
pub fn write_summary(ledger_path: &Utf8Path, session: &Session) -> crate::Result<()> {
    let summary = RunSummary::from_session(session);
    let text = serde_json::to_string_pretty(&summary).map_err(|source| {
        crate::parse::Error::JsonSerialize {
            context: format!("serialize run summary for {ledger_path}"),
            source,
        }
    })?;
    crate::run_session::write_text_atomically(&summary_path(ledger_path), &format!("{text}\n"))
}

fn read_summary(ledger_path: &Utf8Path) -> Option<RunSummary> {
    let text = std::fs::read_to_string(summary_path(ledger_path).as_std_path()).ok()?;
    serde_json::from_str(&text).ok()
}

fn mtime_epoch(path: &Utf8Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path.as_std_path()).ok()?.modified().ok()
}

/// A sidecar is usable iff it exists, parses, and is at least as fresh as
/// its ledger (`sidecar.mtime >= ledger.mtime`) — both are written by the
/// same call, in order, so an out-of-date sidecar means either a crash
/// between the two writes or a ledger mutated by an older binary that never
/// wrote a sidecar at all. Either way, falling back to the ledger is always
/// correct; the sidecar is derived evidence, never authority.
pub fn summary_is_fresh(ledger_path: &Utf8Path) -> bool {
    let (Some(ledger_mtime), Some(sidecar_mtime)) = (
        mtime_epoch(ledger_path),
        mtime_epoch(&summary_path(ledger_path)),
    ) else {
        return false;
    };
    sidecar_mtime >= ledger_mtime
}

/// Resolve a summary for `ledger_path`: the fresh sidecar if one parses,
/// otherwise the ledger itself, projected. Two `stat`s in the fast path, no
/// digest, no parse of the ledger's frame history.
pub fn read_summary_or_ledger(ledger_path: &Utf8Path) -> crate::Result<RunSummary> {
    if summary_is_fresh(ledger_path) {
        if let Some(summary) = read_summary(ledger_path) {
            return Ok(summary);
        }
    }
    let session = crate::run_session::read_run_session(ledger_path)?;
    Ok(RunSummary::from_session(&session))
}

/// Best-effort removal of a ledger's summary sidecar, sibling to
/// `run_liveness::remove_row` — called wherever a ledger itself is deleted
/// (P512), never grown into its own sweep here.
pub fn remove_summary_for_ledger(ledger_path: &Utf8Path) {
    let _ = std::fs::remove_file(summary_path(ledger_path).as_std_path());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(name: &str) -> Utf8PathBuf {
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("temp dir is UTF-8")
            .join(format!(
                "ctx-run-summary-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
        let _ = std::fs::remove_dir_all(dir.as_std_path());
        std::fs::create_dir_all(dir.as_std_path()).expect("create scratch dir");
        dir
    }

    /// Every field left out below has a `#[serde(default)]` on `Session`,
    /// `Provenance`, or `State`, so this fixture only needs to supply the
    /// handful of genuinely required fields.
    fn fixture_session() -> Session {
        serde_json::from_value(serde_json::json!({
            "schema-version": "0.1.0",
            "session-id": "session-fixture",
            "run-id": "run-fixture",
            "trait-id": "fixture-trait",
            "current-run-index": 0,
            "status": "awaiting-agent-output",
            "provenance": {
                "started-by": {"surface": "test", "caller": "run-summary-fixture"},
                "state-source": "test",
                "started-at-epoch": 1000,
            },
            "ledger": {
                "run-id": "run-fixture",
                "trait-id": "fixture-trait",
                "current-run-index": 0,
                "final-state": "running",
            },
            "state-digest": "sha256:fixture",
        }))
        .expect("fixture session deserializes")
    }

    #[test]
    fn from_session_round_trips() {
        let session = fixture_session();
        let summary = RunSummary::from_session(&session);
        assert_eq!(summary.session_id, "session-fixture");
        assert_eq!(summary.started_at_epoch, Some(1000));
        let text = serde_json::to_string(&summary).expect("serialize");
        let round_tripped: RunSummary = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(round_tripped, summary);
    }

    #[test]
    fn stale_sidecar_falls_back_to_ledger() {
        let dir = scratch_dir("stale");
        let ledger_path = dir.join("session-fixture.json");
        let session = fixture_session();
        crate::run_session::write_run_session(&ledger_path, &session).expect("write ledger");
        write_summary(&ledger_path, &session).expect("write summary");
        // Age the sidecar's mtime behind the ledger's: touch the ledger file
        // directly (rather than through `write_run_session`, which would
        // regenerate the sidecar too and keep it fresh).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let text = std::fs::read_to_string(ledger_path.as_std_path()).expect("read ledger");
        std::fs::write(ledger_path.as_std_path(), text).expect("touch ledger");
        assert!(!summary_is_fresh(&ledger_path));
        let resolved = read_summary_or_ledger(&ledger_path).expect("resolve");
        assert_eq!(resolved.session_id, "session-fixture");
    }

    #[test]
    fn unparseable_sidecar_falls_back_to_ledger_without_error() {
        let dir = scratch_dir("corrupt");
        let ledger_path = dir.join("session-fixture.json");
        let session = fixture_session();
        crate::run_session::write_run_session(&ledger_path, &session).expect("write ledger");
        std::fs::write(summary_path(&ledger_path).as_std_path(), "not json")
            .expect("corrupt sidecar");
        let resolved =
            read_summary_or_ledger(&ledger_path).expect("resolve despite corrupt sidecar");
        assert_eq!(resolved.session_id, "session-fixture");
    }
}
