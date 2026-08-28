//! Compact center projection of a session ledger. It is derived only from an
//! already parsed session and is persisted in the center's disposable index,
//! never beside the authoritative ledger.
//! Clients keep no second memo of these rows and therefore have no disk cache
//! or invalidation protocol to coordinate with the center.

use ctx_traits_core::procedure::activity::SessionState;
use ctx_traits_core::procedure::session::{
    MergeFrame, MergeStatus, Session, Status, TraitSource, WorktreeProvenance,
};
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
    /// The session's task value. Keep the preceding center wire key while its
    /// shared disposable index is read by older binaries.
    #[serde(rename = "phase")]
    pub task_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_drive_outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narrator_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guide_tokens: Option<u64>,
    /// Whether the ledger's most recent `merge_frames` entry exists at all —
    /// lets the MERGES projection skip ledgers with nothing to show without
    /// reading the ledger.
    pub has_merge_frames: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_merge_status: Option<String>,
    /// 0151: `session::landing_state`'s projection, as a stable wire word —
    /// `"landed"`, `"not-merged"`, `"parked"`, or `"merge-failed"`. `None`
    /// mid-run, for a non-worktree run, and for a completed clean-tree run
    /// (nothing committed) — the center projection stays honest on every
    /// intermediate update, never guessing ahead of the ledger's own evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landing: Option<String>,
    /// Task provenance and terminal merge evidence let task reports use a
    /// single center snapshot rather than reopening each ledger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_terminal_merge_frame: Option<MergeFrame>,
    #[serde(default)]
    pub blocked_with_park_report: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_epoch: Option<u64>,
    /// Facts consumed by center clients' list projections. They intentionally
    /// remain compact; detail views request the cached full session separately.
    #[serde(default)]
    pub elapsed_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_sequence_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeProvenance>,
    // Keep the preceding flattened keys in the shared index. Installed v1
    // centers deserialize these fields while current clients use `worktree`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    #[serde(default)]
    pub has_commit_receipt: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_frame_kind: Option<ctx_traits_core::procedure::runtime::SequenceFrameKind>,
    #[serde(default)]
    pub interrupted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trait_source: Option<TraitSource>,
    /// A discovered ledger that cannot be parsed remains visible to center
    /// clients instead of vanishing from aggregate answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parse_error: Option<String>,
    /// 0262: facts `Stats` and `StandingWall` need that were previously read
    /// from a retained `Session`, so those handlers can answer from this
    /// projection alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<ctx_traits_core::procedure::runtime::StopReason>,
    #[serde(default)]
    pub tokens_by_model: std::collections::BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict_rounds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub park_wall_id: Option<String>,
}

impl RunSummary {
    /// Project a summary from an already-parsed session. Pure — no IO.
    pub fn from_session(session: &Session) -> RunSummary {
        let last_merge = session.provenance.merge_frames.last();
        let last_terminal_merge_frame = session
            .provenance
            .merge_frames
            .iter()
            .rev()
            .find(|frame| frame.status.is_terminal())
            .cloned();
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
            task_value: crate::run_session::session_task(session),
            started_at_epoch: session.provenance.started_at_epoch,
            exit_code: session
                .last_drive_outcome
                .as_ref()
                .and_then(|outcome| outcome.exit_code),
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
            guide_tokens: session
                .last_drive_outcome
                .as_ref()
                .and_then(|outcome| outcome.token_usage.as_ref())
                .and_then(|usage| usage.guide_tokens),
            has_merge_frames: last_merge.is_some(),
            last_merge_status: last_merge.map(|frame| merge_status_str(frame.status)),
            landing: ctx_traits_core::procedure::session::landing_state(session).map(|landing| {
                match landing {
                    ctx_traits_core::procedure::session::LandingState::Landed { .. } => {
                        "landed".to_string()
                    }
                    ctx_traits_core::procedure::session::LandingState::NotMerged => {
                        "not-merged".to_string()
                    }
                    ctx_traits_core::procedure::session::LandingState::Parked => {
                        "parked".to_string()
                    }
                    ctx_traits_core::procedure::session::LandingState::MergeFailed => {
                        "merge-failed".to_string()
                    }
                }
            }),
            task_key: session.provenance.task_key.clone(),
            task_digest: session
                .provenance
                .task_digest
                .as_ref()
                .map(ToString::to_string),
            last_terminal_merge_frame,
            blocked_with_park_report: session.status == Status::Blocked
                && crate::run_session::session_park_report(session).is_some(),
            terminal_epoch: session
                .last_drive_outcome
                .as_ref()
                .map(|outcome| outcome.recorded_at_epoch),
            elapsed_seconds: session.ledger.elapsed_seconds,
            current_sequence_title: session.current_sequence_title.clone(),
            canonical_digest: session.canonical_digest.as_ref().map(ToString::to_string),
            source_digest: session.source_digest.as_ref().map(ToString::to_string),
            title: session
                .provenance
                .session_title
                .as_ref()
                .and_then(|title| title.resolved_title())
                .map(str::to_string),
            worktree: session.provenance.worktree.clone(),
            worktree_id: session
                .provenance
                .worktree
                .as_ref()
                .map(|worktree| worktree.id.clone()),
            worktree_branch: session
                .provenance
                .worktree
                .as_ref()
                .map(|worktree| worktree.branch.clone()),
            worktree_path: session
                .provenance
                .worktree
                .as_ref()
                .and_then(|worktree| worktree.path.clone()),
            has_commit_receipt: ctx_traits_core::procedure::session::commit_receipt(
                &session.ledger,
            )
            .is_some(),
            next_frame_kind: session.next_frame.as_ref().map(|frame| frame.kind.clone()),
            interrupted: session.last_drive_outcome.as_ref().is_some_and(|outcome| {
                outcome.outcome
                    == ctx_traits_core::procedure::session::DriveOutcomeKind::Interrupted
            }),
            trait_source: session.provenance.trait_source.clone(),
            parse_error: None,
            stop_reason: session.stop_reason.clone(),
            tokens_by_model: session
                .last_drive_outcome
                .as_ref()
                .and_then(|outcome| outcome.tokens_by_model.clone())
                .unwrap_or_default(),
            verdict_rounds: ctx_traits_core::procedure::stats::verdict_slot_rounds(
                &session.slot_revisions,
            ),
            park_wall_id: crate::run_session::session_park_report(session).and_then(|report| {
                report
                    .get("wall-id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            }),
        }
    }

    pub fn unreadable(session_id: String, error: String) -> RunSummary {
        RunSummary {
            session_id,
            run_id: String::new(),
            trait_id: String::new(),
            status: Status::Failed,
            session_state: None,
            task_value: None,
            started_at_epoch: None,
            exit_code: None,
            last_drive_outcome: None,
            work_tokens: None,
            narrator_tokens: None,
            guide_tokens: None,
            has_merge_frames: false,
            last_merge_status: None,
            landing: None,
            task_key: None,
            task_digest: None,
            last_terminal_merge_frame: None,
            blocked_with_park_report: false,
            terminal_epoch: None,
            elapsed_seconds: 0,
            current_sequence_title: None,
            canonical_digest: None,
            source_digest: None,
            title: None,
            worktree: None,
            worktree_id: None,
            worktree_branch: None,
            worktree_path: None,
            has_commit_receipt: false,
            next_frame_kind: None,
            interrupted: false,
            trait_source: None,
            parse_error: Some(error),
            stop_reason: None,
            tokens_by_model: std::collections::BTreeMap::new(),
            verdict_rounds: None,
            park_wall_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(!text.contains("\"task_value\":"));
        let round_tripped: RunSummary = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(round_tripped, summary);
    }

    #[test]
    fn previous_center_summary_json_uses_defaults_for_added_projection_facts() {
        let summary: RunSummary = serde_json::from_value(serde_json::json!({
            "session_id": "session-fixture",
            "run_id": "run-fixture",
            "trait_id": "fixture-trait",
            "status": "completed",
            "phase": "preceding-task-value",
            "has_merge_frames": false,
        }))
        .expect("previous summary remains readable");
        assert!(summary.task_key.is_none());
        assert_eq!(summary.task_value.as_deref(), Some("preceding-task-value"));
        assert!(summary.last_terminal_merge_frame.is_none());
        assert!(!summary.blocked_with_park_report);
        assert!(summary.stop_reason.is_none());
        assert!(summary.tokens_by_model.is_empty());
        assert!(summary.verdict_rounds.is_none());
        assert!(summary.park_wall_id.is_none());
    }

    #[test]
    fn current_projection_keeps_v1_flattened_worktree_facts() {
        let mut session = fixture_session();
        session.provenance.worktree = Some(WorktreeProvenance {
            id: "worktree-id".to_string(),
            branch: "ctx/run/worktree-id".to_string(),
            seed_snapshots: Vec::new(),
            path: Some("/tmp/worktree-id".to_string()),
        });

        let summary = RunSummary::from_session(&session);
        let json = serde_json::to_value(summary).expect("serialize summary");
        assert_eq!(json["worktree_id"], "worktree-id");
        assert_eq!(json["worktree_branch"], "ctx/run/worktree-id");
        assert_eq!(json["worktree_path"], "/tmp/worktree-id");
    }
}
