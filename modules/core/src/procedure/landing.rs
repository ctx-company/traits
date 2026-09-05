//! 0265.15: the pure line projection behind the Sessions preview's `landing`
//! block — grammar rule 13's three fixed `→` consequence lines (worktree,
//! merge, task close). Every string here is either fixed text this module
//! binds verbatim or a value the caller already resolved (a worktree id, a
//! merge revision, a claimed task's key); nothing here reads a session,
//! provider or config document directly, so a desktop-only or CLI-only
//! caller can compose the same three lines from whatever evidence it already
//! has in hand.

use super::session::{
    MergeFrame, MergeRung, MergeStatus, WorktreeProvenance, merge_frame_revision,
};
use crate::task::{AutoClosePolicy, Closure, TaskStatus};

/// The tone one landing line renders in — deliberately not gpui's own state
/// role vocabulary, so this crate never depends on a UI toolkit; a desktop
/// caller maps this onto its own tone type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineTone {
    Neutral,
    Ok,
    Warn,
    Danger,
}

/// One `→`-prefixed landing line, fixed text or a value substitution, with
/// its tone. The `→ ` prefix is part of `text` — this is the one place that
/// prefix is written, so every line's fixed text has one source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandingLine {
    pub text: String,
    pub tone: LineTone,
}

impl LandingLine {
    fn neutral(text: impl Into<String>) -> Self {
        Self {
            text: format!("\u{2192} {}", text.into()),
            tone: LineTone::Neutral,
        }
    }

    fn toned(text: impl Into<String>, tone: LineTone) -> Self {
        Self {
            text: format!("\u{2192} {}", text.into()),
            tone,
        }
    }
}

/// The three lines the `landing` block always renders, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandingLines {
    pub worktree: LandingLine,
    pub merge: LandingLine,
    pub close: LandingLine,
}

/// Goal 2: `Provenance.worktree` verbatim, or the truthful absence form —
/// `None` covers both a run started without `--worktree` and every legacy
/// ledger, so this never claims "started without a worktree".
pub fn worktree_line(worktree: Option<&WorktreeProvenance>) -> LandingLine {
    match worktree {
        Some(worktree) => LandingLine::neutral(format!("Runs in worktree {}", worktree.id)),
        None => LandingLine::neutral("No worktree recorded for this run"),
    }
}

/// Goal 3: the configured `merge_intent` until the **last** terminal frame in
/// `merge_frames` order supersedes it entirely; a nonterminal frame
/// (`LockAcquired`/`GatesPassed`/`Reconciled`) changes nothing. Never names a
/// branch — `MergeRung` carries none.
pub fn merge_line(merge_intent: Option<MergeRung>, merge_frames: &[MergeFrame]) -> LandingLine {
    if let Some(frame) = merge_frames
        .iter()
        .rev()
        .find(|frame| frame.status.is_terminal())
    {
        return match frame.status {
            MergeStatus::Merged => match merge_frame_revision(frame) {
                Some(revision) => LandingLine::toned(format!("Merged at {revision}"), LineTone::Ok),
                None => LandingLine::toned("Merged", LineTone::Ok),
            },
            MergeStatus::Parked => LandingLine::toned("Merge parked", LineTone::Warn),
            MergeStatus::PostMergeCleanupFailure | MergeStatus::RecoveryFailure => {
                LandingLine::toned("Merge failed", LineTone::Danger)
            }
            MergeStatus::LockAcquired | MergeStatus::GatesPassed | MergeStatus::Reconciled => {
                unreachable!("filtered to is_terminal() frames only")
            }
        };
    }
    match merge_intent {
        Some(MergeRung::Standard) => {
            LandingLine::neutral("Merges automatically when this run lands")
        }
        Some(MergeRung::Deep) => {
            LandingLine::neutral("Merges deep automatically when this run lands")
        }
        None => LandingLine::neutral("No automatic merge intent recorded"),
    }
}

/// What the claimed-task center answer resolved to, reduced to what the
/// close line needs: whether a task is even claimed, and — when one is —
/// its key plus the effective `auto-close` policy outcome. A caller (the
/// desktop) maps its own served `ClaimedTaskResult`/`ClosePolicyResolution`
/// onto this before calling [`close_line`]; this module never depends on
/// the center's wire types.
pub enum ClaimedTaskCloseOutcome<'a> {
    /// No task claimed by this run — a served `Unclaimed` answer.
    Unclaimed,
    /// A task is claimed and its close-policy resolution is known.
    Claimed {
        task_key: &'a str,
        policy: ResolvedClosePolicy,
    },
    /// The claimed-task answer itself did not resolve to a task at all
    /// (missing, ambiguous, or a request/provider error) — no key is
    /// available to name in the unresolved form.
    Unavailable,
}

/// The three-way close-policy resolution outcome, reduced for this
/// projection — the config-failure reason text (if any) is deliberately not
/// carried here; the line never renders it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedClosePolicy {
    Effective(AutoClosePolicy),
    NoneConfigured,
    Unresolved,
}

/// The selected task's served claim posture. This is deliberately independent
/// of the center wire so every consumer uses the same consequence wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectedTaskClaimState {
    None,
    Active,
    Pending,
    Terminal,
    Ambiguous,
}

fn future_close_text(task_key: &str, policy: ResolvedClosePolicy) -> String {
    match policy {
        ResolvedClosePolicy::Effective(AutoClosePolicy::Confirm) => {
            format!("Task {task_key} closes only on owner confirmation")
        }
        ResolvedClosePolicy::Effective(AutoClosePolicy::Checked) => {
            format!("Task {task_key} closes when its declared checks pass")
        }
        ResolvedClosePolicy::Effective(AutoClosePolicy::Merge) => {
            format!("Task {task_key} closes when a run lands, checks or not")
        }
        ResolvedClosePolicy::NoneConfigured => {
            format!("Task {task_key} has no auto-close policy")
        }
        ResolvedClosePolicy::Unresolved => format!("Task {task_key} close policy unresolved"),
    }
}

/// Project the one close consequence for a selected board task. Stored closure
/// facts take precedence over its current claim and configured future policy.
pub fn selected_task_close_line(
    task_key: &str,
    stored_status: Option<TaskStatus>,
    closure: Option<&Closure>,
    claim_state: SelectedTaskClaimState,
    policy: ResolvedClosePolicy,
    declared_check_count: usize,
) -> LandingLine {
    let cancelled = matches!(stored_status, Some(TaskStatus::Cancelled));
    let closed = matches!(stored_status, Some(TaskStatus::Done)) || closure.is_some();
    if cancelled {
        return LandingLine::toned(format!("Task {task_key} was cancelled"), LineTone::Warn);
    }
    if closed {
        return match closure.and_then(|closure| closure.commit.as_deref()) {
            Some(commit) => {
                LandingLine::toned(format!("Task {task_key} closed at {commit}"), LineTone::Ok)
            }
            None => LandingLine::toned(format!("Task {task_key} closed"), LineTone::Ok),
        };
    }
    if policy == ResolvedClosePolicy::Unresolved {
        return LandingLine::toned(
            format!("Task {task_key} close policy unresolved"),
            LineTone::Danger,
        );
    }
    if claim_state == SelectedTaskClaimState::Terminal {
        return LandingLine::neutral(match policy {
            ResolvedClosePolicy::Effective(AutoClosePolicy::Confirm) => {
                format!("Task {task_key} did not close without owner confirmation")
            }
            ResolvedClosePolicy::Effective(AutoClosePolicy::Checked) => {
                format!("Task {task_key} did not close because its checks did not pass")
            }
            ResolvedClosePolicy::Effective(AutoClosePolicy::Merge) => {
                format!("Task {task_key} did not close because its run did not land")
            }
            ResolvedClosePolicy::NoneConfigured => {
                format!("Task {task_key} had no auto-close policy")
            }
            ResolvedClosePolicy::Unresolved => unreachable!("handled above"),
        });
    }
    if policy == ResolvedClosePolicy::Effective(AutoClosePolicy::Checked)
        && declared_check_count == 0
    {
        return LandingLine::neutral(format!(
            "Task {task_key} declares no checks, so nothing closes it automatically"
        ));
    }
    LandingLine::neutral(future_close_text(task_key, policy))
}

/// Goal 4: the configured policy for the claimed task, never a prediction
/// that it will close. Five fixed forms over a claimed task's effective
/// policy, plus one failure form (`danger` tone) with two renderings
/// depending on whether a task key is available at all.
pub fn close_line(outcome: &ClaimedTaskCloseOutcome<'_>) -> LandingLine {
    match outcome {
        ClaimedTaskCloseOutcome::Unclaimed => LandingLine::neutral("No task claimed by this run"),
        ClaimedTaskCloseOutcome::Claimed { task_key, policy } => match policy {
            ResolvedClosePolicy::Effective(_) | ResolvedClosePolicy::NoneConfigured => {
                LandingLine::neutral(future_close_text(task_key, *policy))
            }
            ResolvedClosePolicy::Unresolved => LandingLine::toned(
                format!("Task {task_key} close policy unresolved"),
                LineTone::Danger,
            ),
        },
        ClaimedTaskCloseOutcome::Unavailable => {
            LandingLine::toned("Claimed task close policy unresolved", LineTone::Danger)
        }
    }
}

/// The full three-line projection, in the `landing` block's fixed order.
pub fn landing_lines(
    worktree: Option<&WorktreeProvenance>,
    merge_intent: Option<MergeRung>,
    merge_frames: &[MergeFrame],
    close: &ClaimedTaskCloseOutcome<'_>,
) -> LandingLines {
    LandingLines {
        worktree: worktree_line(worktree),
        merge: merge_line(merge_intent, merge_frames),
        close: close_line(close),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procedure::session::MergeStage;

    fn frame(status: MergeStatus) -> MergeFrame {
        MergeFrame {
            stage: MergeStage::Landing,
            status,
            reason: None,
            evidence: Vec::new(),
            park_reason: None,
            deep_decisions: Vec::new(),
            reclaim: None,
        }
    }

    #[test]
    fn worktree_line_renders_the_id_or_the_absence_form() {
        let provenance = WorktreeProvenance {
            id: "wt-ab12ef".to_string(),
            branch: "ctx/run/wt-ab12ef".to_string(),
            seed_snapshots: Vec::new(),
            path: Some("/tmp/wt-ab12ef".to_string()),
            base: None,
        };
        assert_eq!(
            worktree_line(Some(&provenance)).text,
            "\u{2192} Runs in worktree wt-ab12ef"
        );
        assert_eq!(
            worktree_line(None).text,
            "\u{2192} No worktree recorded for this run"
        );
    }

    #[test]
    fn merge_line_covers_every_intent_and_terminal_cell() {
        assert_eq!(
            merge_line(Some(MergeRung::Standard), &[]).text,
            "\u{2192} Merges automatically when this run lands"
        );
        assert_eq!(
            merge_line(Some(MergeRung::Deep), &[]).text,
            "\u{2192} Merges deep automatically when this run lands"
        );
        assert_eq!(
            merge_line(None, &[]).text,
            "\u{2192} No automatic merge intent recorded"
        );

        // A nonterminal frame changes nothing.
        let nonterminal = merge_line(
            Some(MergeRung::Standard),
            &[frame(MergeStatus::LockAcquired)],
        );
        assert_eq!(
            nonterminal.text,
            "\u{2192} Merges automatically when this run lands"
        );

        let mut merged = frame(MergeStatus::Merged);
        merged.evidence.push("landed=abc123".to_string());
        let line = merge_line(Some(MergeRung::Deep), &[merged]);
        assert_eq!(line.text, "\u{2192} Merged at abc123");
        assert_eq!(line.tone, LineTone::Ok);

        let line = merge_line(None, &[frame(MergeStatus::Merged)]);
        assert_eq!(line.text, "\u{2192} Merged");
        assert_eq!(line.tone, LineTone::Ok);

        let line = merge_line(Some(MergeRung::Standard), &[frame(MergeStatus::Parked)]);
        assert_eq!(line.text, "\u{2192} Merge parked");
        assert_eq!(line.tone, LineTone::Warn);

        for status in [
            MergeStatus::PostMergeCleanupFailure,
            MergeStatus::RecoveryFailure,
        ] {
            let line = merge_line(Some(MergeRung::Standard), &[frame(status)]);
            assert_eq!(line.text, "\u{2192} Merge failed");
            assert_eq!(line.tone, LineTone::Danger);
        }

        // The last terminal frame in order wins, nonterminal frames after it
        // notwithstanding order — here Parked (terminal) precedes a later
        // nonterminal frame, which must not resurrect the intent form.
        let line = merge_line(
            Some(MergeRung::Standard),
            &[frame(MergeStatus::Parked), frame(MergeStatus::LockAcquired)],
        );
        assert_eq!(line.text, "\u{2192} Merge parked");
    }

    #[test]
    fn merge_line_supersedes_a_terminal_frame_even_with_no_intent_recorded() {
        let line = merge_line(None, &[frame(MergeStatus::Merged)]);
        assert_eq!(line.text, "\u{2192} Merged");
    }

    #[test]
    fn close_line_covers_all_six_outcomes() {
        assert_eq!(
            close_line(&ClaimedTaskCloseOutcome::Unclaimed).text,
            "\u{2192} No task claimed by this run"
        );
        assert_eq!(
            close_line(&ClaimedTaskCloseOutcome::Claimed {
                task_key: "0243.4",
                policy: ResolvedClosePolicy::Effective(AutoClosePolicy::Confirm),
            })
            .text,
            "\u{2192} Task 0243.4 closes only on owner confirmation"
        );
        assert_eq!(
            close_line(&ClaimedTaskCloseOutcome::Claimed {
                task_key: "0243.4",
                policy: ResolvedClosePolicy::Effective(AutoClosePolicy::Checked),
            })
            .text,
            "\u{2192} Task 0243.4 closes when its declared checks pass"
        );
        assert_eq!(
            close_line(&ClaimedTaskCloseOutcome::Claimed {
                task_key: "0243.4",
                policy: ResolvedClosePolicy::Effective(AutoClosePolicy::Merge),
            })
            .text,
            "\u{2192} Task 0243.4 closes when a run lands, checks or not"
        );
        assert_eq!(
            close_line(&ClaimedTaskCloseOutcome::Claimed {
                task_key: "0243.4",
                policy: ResolvedClosePolicy::NoneConfigured,
            })
            .text,
            "\u{2192} Task 0243.4 has no auto-close policy"
        );
        let unresolved_with_key = close_line(&ClaimedTaskCloseOutcome::Claimed {
            task_key: "0243.4",
            policy: ResolvedClosePolicy::Unresolved,
        });
        assert_eq!(
            unresolved_with_key.text,
            "\u{2192} Task 0243.4 close policy unresolved"
        );
        assert_eq!(unresolved_with_key.tone, LineTone::Danger);

        let unavailable = close_line(&ClaimedTaskCloseOutcome::Unavailable);
        assert_eq!(
            unavailable.text,
            "\u{2192} Claimed task close policy unresolved"
        );
        assert_eq!(unavailable.tone, LineTone::Danger);
    }

    #[test]
    fn selected_task_close_line_prefers_recorded_outcomes_and_handles_no_checks() {
        assert_eq!(
            selected_task_close_line(
                "0266.5",
                None,
                None,
                SelectedTaskClaimState::Active,
                ResolvedClosePolicy::Effective(AutoClosePolicy::Checked),
                0,
            )
            .text,
            "\u{2192} Task 0266.5 declares no checks, so nothing closes it automatically"
        );
        let terminal = selected_task_close_line(
            "0266.5",
            None,
            None,
            SelectedTaskClaimState::Terminal,
            ResolvedClosePolicy::Effective(AutoClosePolicy::Merge),
            1,
        );
        assert_eq!(
            terminal.text,
            "\u{2192} Task 0266.5 did not close because its run did not land"
        );
        let cancelled = selected_task_close_line(
            "0266.5",
            Some(TaskStatus::Cancelled),
            None,
            SelectedTaskClaimState::Terminal,
            ResolvedClosePolicy::Unresolved,
            0,
        );
        assert_eq!(cancelled.text, "\u{2192} Task 0266.5 was cancelled");
        assert_eq!(cancelled.tone, LineTone::Warn);
    }
}
