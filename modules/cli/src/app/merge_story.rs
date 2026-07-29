//! Single translation source for `ctx traits merge` outcomes (P472; the P467
//! coordination clause): turns a [`MergeFrame`]/[`MergeReport`] park or
//! failure into a plain-language sentence, a next action, and labeled gate
//! evidence rows. Pure — no IO, no terminal, no Git — so every mapping here
//! is directly unit-testable. Consumed by both `merge::print_report` (the CLI
//! half) and the dashboard MERGES-screen detail pane, so the wording is
//! written exactly once.
//!
//! Classification prefers typed fields (`frame.stage`, `frame.park_reason`,
//! `report.status`) wherever they already discriminate a class; the handful
//! of sub-causes that are only distinguishable by text (the five preflight
//! causes, the reconciliation/gate/landing/cleanup families) match on the
//! shared constants in [`reasons`] — the SAME constants `merge.rs`'s park
//! call sites format their reason strings from, so the classifier cannot
//! silently fall out of sync with what the binary actually emits. An
//! unrecognized reason never disappears: it renders under [`ParkClass::Unknown`]
//! with the raw text preserved and a generic next action.

use ctx_traits_core::procedure::activity::ActivityKind;
use ctx_traits_core::procedure::session::{MergeFrame, MergeStage, MergeStatus, ParkReason};

use super::merge::MergeReport;

/// One point of merge progress this phase turns into an [`ActivityEvent`]-
/// shaped draft (frame_id/kind/text/tool, minus `sequence` — assigning that
/// is `MergeLive::emit`'s job in `merge.rs`, the only place that knows the
/// live sequence counter). Every variant maps onto the EXISTING P504
/// `ActivityKind` vocabulary (see [`activity_event`]) rather than growing a
/// merge-specific one.
pub(crate) enum MergeProgress<'a> {
    /// Waiting for the cross-process merge lock.
    LockWaiting,
    /// The merge lock was acquired; about to enter preflight.
    LockAcquired,
    /// Entering a new stage — a plain "starting <stage>" sentence, distinct
    /// from the terminal [`MergeProgress::FrameRecorded`] event the stage
    /// eventually produces.
    StageEntered(MergeStage),
    /// A ledger frame was just appended — the terminal event for whatever
    /// stage/attempt it closes out. Text is [`explain_frame`]'s sentence, so
    /// a park says the exact same sentence live as in `story` and on the
    /// MERGES screen (the phase's core invariant).
    FrameRecorded(&'a MergeFrame),
    /// One declared `[merge] gate` command is about to run.
    GateCommandStarted { label: &'a str },
    /// One declared `[merge] gate` command finished.
    GateCommandFinished {
        label: &'a str,
        exit_code: Option<i32>,
        timed_out: bool,
    },
    /// A merger harness call is about to be dispatched (confirmation,
    /// reconciliation-loop iteration, or seed-harvest adjudication).
    MergerDispatching { role: &'a str, attempt: u32 },
    /// A merger reply needs a P463 item D correction retry.
    MergerRetrying { role: &'a str, attempt: u32 },
}

/// One [`ActivityEvent`]-shaped draft, minus `sequence`.
pub(crate) struct ActivityDraft {
    pub(crate) frame_id: String,
    pub(crate) kind: ActivityKind,
    pub(crate) text: Option<String>,
    pub(crate) tool: Option<String>,
}

/// Plain-language outcome for a gate command's exit code — never the raw
/// `Some(1)`/`None` debug tokens. Shares wording with [`gate_result_text`]
/// (which parses the same values back out of a formatted evidence string)
/// but takes the typed values directly, since this is called live, before
/// the evidence string is formatted.
fn gate_outcome_text(exit_code: Option<i32>, timed_out: bool) -> String {
    if timed_out {
        return "timed out".to_string();
    }
    match exit_code {
        Some(0) => "passed".to_string(),
        Some(code) => format!("failed (exit {code})"),
        None => "killed by signal (no exit code)".to_string(),
    }
}

/// Turn one [`MergeProgress`] point into an [`ActivityDraft`] — pure, no IO,
/// same discipline as [`explain_frame`]/[`explain_report`]. `frame_id` is
/// always `merge:<stage-slug>` (`stage_text`'s wording) so a live row and its
/// eventual `story`/MERGES-screen row share the same identity.
pub(crate) fn activity_event(progress: &MergeProgress<'_>) -> ActivityDraft {
    match progress {
        MergeProgress::LockWaiting => ActivityDraft {
            frame_id: "merge:lock".to_string(),
            kind: ActivityKind::Stalled,
            text: Some("waiting for the merge lock".to_string()),
            tool: None,
        },
        MergeProgress::LockAcquired => ActivityDraft {
            frame_id: "merge:lock".to_string(),
            kind: ActivityKind::Dispatching,
            text: Some("merge lock acquired".to_string()),
            tool: None,
        },
        MergeProgress::StageEntered(stage) => ActivityDraft {
            frame_id: format!("merge:{}", stage_text(*stage)),
            kind: ActivityKind::Dispatching,
            text: Some(format!("starting {}", stage_text(*stage))),
            tool: None,
        },
        MergeProgress::FrameRecorded(frame) => ActivityDraft {
            frame_id: format!("merge:{}", stage_text(frame.stage)),
            // P549 live rows: a row folding these events needs to tell a
            // successful stage checkpoint from a terminal failure without
            // re-parsing `text` — `Stalled` (already used for the lock-wait
            // point, whose frame_id "merge:lock" is the one FrameRecorded
            // frame_id that can never carry it, since `Lock` only ever
            // records `LockAcquired`) is the closest honest fit for "needs
            // human attention", reused rather than adding a new
            // `ActivityKind` variant.
            kind: if matches!(
                frame.status,
                MergeStatus::Parked
                    | MergeStatus::PostMergeCleanupFailure
                    | MergeStatus::RecoveryFailure
            ) {
                ActivityKind::Stalled
            } else {
                ActivityKind::ValidatingOutput
            },
            text: Some(explain_frame(frame).sentence),
            tool: None,
        },
        MergeProgress::GateCommandStarted { label } => ActivityDraft {
            frame_id: "merge:gates".to_string(),
            kind: ActivityKind::RunningTool,
            text: Some(format!("running gate command {label}")),
            tool: Some((*label).to_string()),
        },
        MergeProgress::GateCommandFinished {
            label,
            exit_code,
            timed_out,
        } => ActivityDraft {
            frame_id: "merge:gates".to_string(),
            kind: ActivityKind::RunningTool,
            text: Some(format!(
                "gate {label} {}",
                gate_outcome_text(*exit_code, *timed_out)
            )),
            tool: Some((*label).to_string()),
        },
        MergeProgress::MergerDispatching { role, attempt } => ActivityDraft {
            frame_id: "merge:reconciliation".to_string(),
            kind: ActivityKind::Dispatching,
            text: Some(format!("dispatching {role} (attempt {attempt})")),
            tool: None,
        },
        MergeProgress::MergerRetrying { role, attempt } => ActivityDraft {
            frame_id: "merge:reconciliation".to_string(),
            kind: ActivityKind::Retrying,
            text: Some(format!("retrying {role} (attempt {attempt})")),
            tool: None,
        },
    }
}

/// The literal reason-text fragments every classified park/failure cause is
/// keyed on. `merge.rs`'s park call sites format from these same constants
/// (directly, or as a `format!` prefix) — see each constant's use site there.
pub(crate) mod reasons {
    pub(crate) const BRANCH_MISMATCH_PREFIX: &str = "invocation checkout is on branch";
    pub(crate) const INVOCATION_CHECKOUT_DIRTY_PREFIX: &str = "invocation checkout (";
    pub(crate) const WORKTREE_UNREGISTERED: &str = "worktree registration verification failed";
    pub(crate) const WORKTREE_DIRTY_SUFFIX: &str = "is not clean";
    pub(crate) const REBASE_IN_PROGRESS_SUFFIX: &str = "already has a rebase in progress";

    /// Preflight git/filesystem probe failures — distinct from the four
    /// probe-succeeded-but-disqualifying causes above: these are the probe
    /// itself failing to run, not a clean answer disqualifying the merge.
    pub(crate) const CHECKOUT_BRANCH_PROBE_FAILED: &str =
        "failed to determine invocation checkout branch";
    pub(crate) const CHECKOUT_CLEANLINESS_PROBE_FAILED: &str =
        "failed to check invocation checkout cleanliness";
    pub(crate) const WORKTREE_CLEANLINESS_PROBE_FAILED: &str =
        "failed to check worktree cleanliness";
    pub(crate) const WORKTREE_REBASE_STATE_PROBE_FAILED: &str =
        "failed to check worktree rebase state";

    pub(crate) const REBASE_FAILED: &str = "rebase failed";
    pub(crate) const REBASE_CONTINUE_FAILED: &str = "rebase continue failed";

    pub(crate) const STALE_BASE_OVERLAP_PREFIX: &str = "stale-base overlap:";

    pub(crate) const RECEIPT_INVALID_PREFIX: &str = "merger receipt invalid after";
    pub(crate) const MERGER_TIMEOUT_PREFIX: &str = "merger harness call timed out";
    pub(crate) const MERGER_IDLE_TIMEOUT_PREFIX: &str = "merger harness call idle-timed out";
    pub(crate) const MERGER_SIGNAL_KILL_PREFIX: &str =
        "merger harness process was killed by a signal";
    pub(crate) const MERGER_NONZERO_EXIT_PREFIX: &str = "merger harness exited";
    pub(crate) const MERGER_TRUNCATED_PREFIX: &str = "merger harness output exceeded capture limit";
    pub(crate) const DEEP_REFUSE_REGRESSION_PREFIX: &str = "deep merger refused";

    pub(crate) const RECONCILIATION_ITERATION_CAP_PREFIX: &str = "reconciliation exceeded";
    pub(crate) const RECONCILIATION_NO_PROGRESS_PREFIX: &str = "reconciliation made no progress";
    pub(crate) const WORKTREE_DIRTY_AFTER_RECONCILIATION: &str =
        "worktree left dirty, conflicted, or mid-rebase after reconciliation";
    pub(crate) const GENERATED_ARTIFACT_REBUILD_FAILED_PREFIX: &str =
        "generated-artifact rebuild command failed";

    pub(crate) const GATE_FAILED_PREFIX: &str = "pre-landing gate ";
    pub(crate) const GATE_LEFT_DIRTY: &str =
        "pre-landing gate left the worktree tracked-tree dirty; refusing to land";
    /// P462: the worktree's volume fell below `[merge] disk-floor-mb` before
    /// any declared gate command ran — parked so no evidence of a build was
    /// ever produced, since one couldn't safely be attempted.
    pub(crate) const GATE_DISK_FLOOR_PREFIX: &str = "insufficient disk space for pre-landing gate:";

    pub(crate) const MAIN_ADVANCED_INFIX: &str = "advanced from";
    pub(crate) const MAIN_NOT_CLEAN_INFIX: &str =
        "is no longer clean since the rebase revision was captured";
    pub(crate) const FAST_FORWARD_FAILED: &str = "fast-forward failed";

    pub(crate) const HARVEST_ADJUDICATION_PARKED_PREFIX: &str = "seed harvest adjudication parked:";
    pub(crate) const HARVEST_DIGEST_MISMATCH: &str = "seed harvest parked: seed ancestor digest mismatch (baseline may have raced or been tampered with)";
    pub(crate) const HARVEST_ANCESTOR_UNAVAILABLE: &str =
        "seed harvest parked: seed ancestor unavailable";
    pub(crate) const HARVEST_CHANGED_BOTH_SIDES: &str =
        "seed harvest parked: changed independently on both sides";

    pub(crate) const UNEXPECTED_FAILURE_PREFIX: &str = "unexpected failure during merge";

    pub(crate) const LOCK_TIMEOUT_PREFIX: &str = "timed out after";
    pub(crate) const LOCK_UNAVAILABLE_PREFIX: &str = "merge lock held by another process";
    pub(crate) const LOCK_HELD_BY_PREFIX: &str = "merge lock held by";

    /// Ledger reason prefix for [`super::merge::MergeStatus::PostMergeCleanupFailure`]
    /// — `main` has already fast-forwarded; only cleanup afterward failed. See
    /// `session.rs`'s doc comment on `MergeStatus`: this must never be
    /// reported as parked.
    pub(crate) const POST_MERGE_CLEANUP_FAILED_PREFIX: &str = "post-merge cleanup failed after";
    /// Ledger reason for [`super::merge::MergeStatus::RecoveryFailure`] — an
    /// in-progress rebase's abort could not itself be confirmed, so nothing
    /// can be claimed about the branch/worktree being left intact.
    pub(crate) const RECOVERY_UNCONFIRMED: &str =
        "rebase recovery unconfirmed: could not verify the in-progress rebase was aborted";
}

/// The classified cause of a park/failure — used for tone and retry
/// eligibility, never displayed to a user directly (that is what
/// [`Explanation::sentence`] is for).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParkClass {
    PreflightBranchMismatch,
    PreflightDirtyMain,
    PreflightWorktreeMissing,
    PreflightWorktreeDirty,
    PreflightRebaseInProgress,
    PreflightProbeFailed,
    Rebase,
    StaleBaseOverlap,
    ReceiptInvalid,
    MergerTimeout,
    MergerIdleTimeout,
    MergerSignalKilled,
    MergerNonzeroExit,
    MergerTruncated,
    DeepRefuseRegression,
    ReconciliationIterationCap,
    ReconciliationNoProgress,
    ReconciliationDirtyAfter,
    GeneratedArtifactRebuildFailed,
    GateFailed,
    GateLeftDirty,
    GateDiskFloor,
    LandingMainAdvanced,
    LandingMainNotClean,
    LandingFastForwardFailed,
    HarvestConflict,
    LockTimeout,
    LockUnavailable,
    Unexpected,
    /// [`MergeStatus::PostMergeCleanupFailure`] — the merge LANDED; only
    /// cleanup afterward (worktree/branch removal, seed-harvest apply)
    /// failed. Never a park — `main` has already moved.
    CleanupFailedAfterLanding,
    /// [`MergeStatus::RecoveryFailure`] — an in-progress rebase's abort
    /// could not itself be confirmed, so nothing can be claimed about the
    /// branch/worktree being left intact. Never a park — `Parked` promises
    /// exactly the intactness this outcome could not verify.
    RecoveryUnconfirmed,
    /// [`MergeStatus::Merged`] — the merge landed cleanly. Never a park.
    Landed,
    /// A non-terminal status (`LockAcquired`/`GatesPassed`/`Reconciled`) —
    /// unreachable in practice, since [`super::dashboard`] only ever calls
    /// [`explain_frame`] on the last **terminal** frame of a session
    /// (`MergeStatus::is_terminal`), but the match must still resolve
    /// safely rather than mislabel a still-running attempt as parked.
    InProgress,
    Unknown,
}

impl ParkClass {
    /// Every variant, for the match-exhaustive translator coverage test — a
    /// new class added later cannot silently ship untranslated.
    #[cfg(test)]
    fn all() -> &'static [ParkClass] {
        &[
            ParkClass::PreflightBranchMismatch,
            ParkClass::PreflightDirtyMain,
            ParkClass::PreflightWorktreeMissing,
            ParkClass::PreflightWorktreeDirty,
            ParkClass::PreflightRebaseInProgress,
            ParkClass::PreflightProbeFailed,
            ParkClass::Rebase,
            ParkClass::StaleBaseOverlap,
            ParkClass::ReceiptInvalid,
            ParkClass::MergerTimeout,
            ParkClass::MergerIdleTimeout,
            ParkClass::MergerSignalKilled,
            ParkClass::MergerNonzeroExit,
            ParkClass::MergerTruncated,
            ParkClass::DeepRefuseRegression,
            ParkClass::ReconciliationIterationCap,
            ParkClass::ReconciliationNoProgress,
            ParkClass::ReconciliationDirtyAfter,
            ParkClass::GeneratedArtifactRebuildFailed,
            ParkClass::GateFailed,
            ParkClass::GateLeftDirty,
            ParkClass::GateDiskFloor,
            ParkClass::LandingMainAdvanced,
            ParkClass::LandingMainNotClean,
            ParkClass::LandingFastForwardFailed,
            ParkClass::HarvestConflict,
            ParkClass::LockTimeout,
            ParkClass::LockUnavailable,
            ParkClass::Unexpected,
            ParkClass::CleanupFailedAfterLanding,
            ParkClass::RecoveryUnconfirmed,
            ParkClass::Landed,
            ParkClass::InProgress,
            ParkClass::Unknown,
        ]
    }
}

/// One park/failure explained in plain language: a short headline (for list
/// rows), the full sentence (for the detail pane / CLI panel), the exact next
/// action, and the classification driving both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Explanation {
    pub(crate) headline: String,
    pub(crate) sentence: String,
    pub(crate) next_action: String,
    pub(crate) class: ParkClass,
}

/// One labeled row of gate (or other) evidence, parsed out of a `MergeFrame`/
/// `MergeReport`'s raw `evidence` strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GateRow {
    pub(crate) label: String,
    pub(crate) value: String,
}

/// Plain-language sentence for the stage a merge reached — never the bare
/// `{:?}` debug form.
pub(crate) fn stage_sentence(stage: MergeStage) -> &'static str {
    match stage {
        MergeStage::Lock => "before anything was touched, waiting for the merge lock",
        MergeStage::Preflight => "before anything was touched, during preflight checks",
        MergeStage::Rebase => "while rebasing the run branch onto the target",
        MergeStage::Reconciliation => "while reconciling conflicts with the merger",
        MergeStage::Gates => "after reconciliation, while running the pre-landing gate",
        MergeStage::Landing => "while fast-forwarding the target branch",
        MergeStage::Cleanup => "after landing, during post-merge cleanup",
    }
}

fn classify_reason(reason: &str) -> ParkClass {
    use reasons::*;
    if reason == CHECKOUT_BRANCH_PROBE_FAILED
        || reason == CHECKOUT_CLEANLINESS_PROBE_FAILED
        || reason == WORKTREE_CLEANLINESS_PROBE_FAILED
        || reason == WORKTREE_REBASE_STATE_PROBE_FAILED
    {
        ParkClass::PreflightProbeFailed
    } else if reason.starts_with(BRANCH_MISMATCH_PREFIX) {
        ParkClass::PreflightBranchMismatch
    } else if reason.starts_with(INVOCATION_CHECKOUT_DIRTY_PREFIX) {
        ParkClass::PreflightDirtyMain
    } else if reason == WORKTREE_UNREGISTERED {
        ParkClass::PreflightWorktreeMissing
    } else if reason.starts_with("worktree ") && reason.ends_with(REBASE_IN_PROGRESS_SUFFIX) {
        ParkClass::PreflightRebaseInProgress
    } else if reason.starts_with("worktree ") && reason.ends_with(WORKTREE_DIRTY_SUFFIX) {
        ParkClass::PreflightWorktreeDirty
    } else if reason == REBASE_FAILED || reason == REBASE_CONTINUE_FAILED {
        ParkClass::Rebase
    } else if reason.starts_with(STALE_BASE_OVERLAP_PREFIX) {
        ParkClass::StaleBaseOverlap
    } else if reason.starts_with(RECEIPT_INVALID_PREFIX) {
        ParkClass::ReceiptInvalid
    } else if reason.starts_with(MERGER_TIMEOUT_PREFIX) {
        ParkClass::MergerTimeout
    } else if reason.starts_with(MERGER_IDLE_TIMEOUT_PREFIX) {
        ParkClass::MergerIdleTimeout
    } else if reason.starts_with(MERGER_SIGNAL_KILL_PREFIX) {
        ParkClass::MergerSignalKilled
    } else if reason.starts_with(MERGER_NONZERO_EXIT_PREFIX) {
        ParkClass::MergerNonzeroExit
    } else if reason.starts_with(MERGER_TRUNCATED_PREFIX) {
        ParkClass::MergerTruncated
    } else if reason.starts_with(DEEP_REFUSE_REGRESSION_PREFIX) {
        ParkClass::DeepRefuseRegression
    } else if reason.starts_with(RECONCILIATION_ITERATION_CAP_PREFIX) {
        ParkClass::ReconciliationIterationCap
    } else if reason.starts_with(RECONCILIATION_NO_PROGRESS_PREFIX) {
        ParkClass::ReconciliationNoProgress
    } else if reason == WORKTREE_DIRTY_AFTER_RECONCILIATION {
        ParkClass::ReconciliationDirtyAfter
    } else if reason.starts_with(GENERATED_ARTIFACT_REBUILD_FAILED_PREFIX) {
        ParkClass::GeneratedArtifactRebuildFailed
    } else if reason == GATE_LEFT_DIRTY {
        ParkClass::GateLeftDirty
    } else if reason.starts_with(GATE_DISK_FLOOR_PREFIX) {
        ParkClass::GateDiskFloor
    } else if reason.starts_with(GATE_FAILED_PREFIX) {
        ParkClass::GateFailed
    } else if reason.contains(MAIN_ADVANCED_INFIX) {
        ParkClass::LandingMainAdvanced
    } else if reason.contains(MAIN_NOT_CLEAN_INFIX) {
        ParkClass::LandingMainNotClean
    } else if reason == FAST_FORWARD_FAILED {
        ParkClass::LandingFastForwardFailed
    } else if reason.starts_with(HARVEST_ADJUDICATION_PARKED_PREFIX)
        || reason == HARVEST_DIGEST_MISMATCH
        || reason == HARVEST_ANCESTOR_UNAVAILABLE
        || reason == HARVEST_CHANGED_BOTH_SIDES
    {
        ParkClass::HarvestConflict
    } else if reason.starts_with(UNEXPECTED_FAILURE_PREFIX) {
        ParkClass::Unexpected
    } else {
        ParkClass::Unknown
    }
}

fn headline_for(class: ParkClass) -> &'static str {
    match class {
        ParkClass::PreflightBranchMismatch => "wrong branch checked out",
        ParkClass::PreflightDirtyMain => "target checkout is dirty",
        ParkClass::PreflightWorktreeMissing => "worktree not registered",
        ParkClass::PreflightWorktreeDirty => "worktree is dirty",
        ParkClass::PreflightRebaseInProgress => "rebase already in progress",
        ParkClass::PreflightProbeFailed => "could not check preflight state",
        ParkClass::Rebase => "rebase failed",
        ParkClass::StaleBaseOverlap => "base moved on overlapping paths",
        ParkClass::ReceiptInvalid => "merger reply was invalid",
        ParkClass::MergerTimeout => "merger timed out",
        ParkClass::MergerIdleTimeout => "merger produced no output",
        ParkClass::MergerSignalKilled => "merger process was killed",
        ParkClass::MergerNonzeroExit => "merger exited with an error",
        ParkClass::MergerTruncated => "merger output was truncated",
        ParkClass::DeepRefuseRegression => "deep merger refused",
        ParkClass::ReconciliationIterationCap => "reconciliation loop capped out",
        ParkClass::ReconciliationNoProgress => "reconciliation made no progress",
        ParkClass::ReconciliationDirtyAfter => "worktree left unclean",
        ParkClass::GeneratedArtifactRebuildFailed => "generated-artifact rebuild failed",
        ParkClass::GateFailed => "gate failed",
        ParkClass::GateLeftDirty => "gate left the tree dirty",
        ParkClass::GateDiskFloor => "insufficient disk space for the gate",
        ParkClass::LandingMainAdvanced => "target branch moved",
        ParkClass::LandingMainNotClean => "target checkout became dirty",
        ParkClass::LandingFastForwardFailed => "fast-forward failed",
        ParkClass::HarvestConflict => "seed harvest conflict",
        ParkClass::LockTimeout => "merge lock timed out",
        ParkClass::LockUnavailable => "merge lock unavailable",
        ParkClass::Unexpected => "unexpected failure",
        ParkClass::CleanupFailedAfterLanding => "landed, but cleanup afterward failed",
        ParkClass::RecoveryUnconfirmed => "rebase recovery could not be confirmed",
        ParkClass::Landed => "landed cleanly",
        ParkClass::InProgress => "merge attempt in progress",
        ParkClass::Unknown => "unrecognized reason",
    }
}

fn next_action_for(class: ParkClass) -> &'static str {
    match class {
        ParkClass::PreflightBranchMismatch | ParkClass::PreflightDirtyMain => {
            "check out the target branch cleanly in the invocation checkout, then retry"
        }
        ParkClass::PreflightWorktreeMissing => {
            "re-register the run's worktree (or re-run `ctx traits worktree`), then retry"
        }
        ParkClass::PreflightWorktreeDirty => "inspect and clean the run's worktree, then retry",
        ParkClass::PreflightRebaseInProgress => {
            "resolve or abort the in-progress rebase in the worktree, then retry"
        }
        ParkClass::PreflightProbeFailed => {
            "the repository/worktree could not be inspected (see evidence for the raw error); retry, and if it recurs check the repository by hand"
        }
        ParkClass::Rebase => "inspect the rebase conflict in the worktree, then retry",
        ParkClass::StaleBaseOverlap => {
            "review the overlapping paths against the responsible target commits, then retry (or re-run without `--park-on-overlap` to land anyway)"
        }
        ParkClass::ReceiptInvalid
        | ParkClass::MergerTimeout
        | ParkClass::MergerIdleTimeout
        | ParkClass::MergerSignalKilled
        | ParkClass::MergerNonzeroExit
        | ParkClass::MergerTruncated => {
            "retry the merge; if it keeps failing, inspect the merger transcript in evidence"
        }
        ParkClass::DeepRefuseRegression => {
            "review the deep merger's refusal rationale in evidence, then decide by hand"
        }
        ParkClass::ReconciliationIterationCap | ParkClass::ReconciliationNoProgress => {
            "inspect the worktree's conflicted paths by hand, then retry"
        }
        ParkClass::ReconciliationDirtyAfter => "clean the worktree by hand, then retry",
        ParkClass::GeneratedArtifactRebuildFailed => {
            "inspect the rebuild command's output (see the gate-output evidence row), then retry"
        }
        ParkClass::GateFailed => "fix the failing gate command locally, then retry",
        ParkClass::GateLeftDirty => "inspect and clean the worktree after the gate ran, then retry",
        ParkClass::GateDiskFloor => {
            "declared regenerable caches were pruned; free space and re-run merge"
        }
        ParkClass::LandingMainAdvanced | ParkClass::LandingMainNotClean => "retry the merge",
        ParkClass::LandingFastForwardFailed => {
            "inspect the target branch by hand (a non-fast-forward state is unexpected), then retry"
        }
        ParkClass::HarvestConflict => {
            "review the seed-harvest conflict in evidence, then decide by hand"
        }
        ParkClass::LockTimeout | ParkClass::LockUnavailable => {
            "wait for the other merge to finish, then retry"
        }
        ParkClass::Unexpected => "inspect the error detail, then retry",
        ParkClass::CleanupFailedAfterLanding => {
            "the merge already landed — clean up the run's worktree and branch by hand (see evidence), then drop this row from the queue"
        }
        ParkClass::RecoveryUnconfirmed => {
            "inspect the worktree by hand before doing anything else — its state after the aborted rebase could not be verified"
        }
        ParkClass::Landed => {
            "nothing left to do — drop it from the queue once you're done reviewing"
        }
        ParkClass::InProgress => "wait for the current attempt to finish, then check back",
        ParkClass::Unknown => "inspect the ledger's raw reason and evidence, then retry",
    }
}

fn sentence_for(class: ParkClass, stage: MergeStage, reason: &str) -> String {
    if class == ParkClass::Unknown {
        return format!(
            "this merge parked at the {} stage; the recorded reason has no plain-language translation yet",
            stage_text(stage)
        );
    }
    format!(
        "{} — this merge parked {} ({})",
        headline_for(class),
        stage_sentence(stage),
        reason
    )
}

pub(crate) fn stage_text(stage: MergeStage) -> &'static str {
    match stage {
        MergeStage::Lock => "lock",
        MergeStage::Preflight => "preflight",
        MergeStage::Rebase => "rebase",
        MergeStage::Reconciliation => "reconciliation",
        MergeStage::Gates => "gates",
        MergeStage::Landing => "landing",
        MergeStage::Cleanup => "cleanup",
    }
}

/// A terminal outcome recorded on a [`MergeFrame`]/[`MergeReport`] that is
/// NOT a park — the merge either already landed (only cleanup afterward
/// failed) or its post-rebase state could not itself be verified. Neither
/// case may be described as "parked" or "did not land" (`session.rs`'s doc
/// comment on `MergeStatus` is explicit that both outcomes must never be
/// reported as parked).
fn explain_non_park_terminal(class: ParkClass, reason: &str) -> Explanation {
    Explanation {
        headline: headline_for(class).to_string(),
        sentence: format!("{} — {reason}", headline_for(class)),
        next_action: next_action_for(class).to_string(),
        class,
    }
}

/// Explain a `Merged` frame: the merge landed cleanly, never a park.
fn explain_landed() -> Explanation {
    Explanation {
        headline: headline_for(ParkClass::Landed).to_string(),
        sentence: "the merge landed cleanly".to_string(),
        next_action: next_action_for(ParkClass::Landed).to_string(),
        class: ParkClass::Landed,
    }
}

/// Explain a non-terminal frame (`LockAcquired`/`GatesPassed`/`Reconciled`):
/// the attempt this frame belongs to has not finished yet, so nothing about
/// it — park, landing, or failure — can be claimed. Unreachable in practice
/// (see [`ParkClass::InProgress`]'s doc), but `explain_frame` must resolve
/// every `MergeStatus` rather than assume only terminal frames ever arrive.
fn explain_in_progress(stage: MergeStage) -> Explanation {
    Explanation {
        headline: headline_for(ParkClass::InProgress).to_string(),
        sentence: format!(
            "this merge attempt is still running — it has not yet reached a final outcome ({})",
            stage_sentence(stage)
        ),
        next_action: next_action_for(ParkClass::InProgress).to_string(),
        class: ParkClass::InProgress,
    }
}

/// Explain a ledger-sourced merge frame. Typed-first and total over every
/// [`MergeStatus`] variant — no wildcard arm — so a status this function
/// does not know how to explain is a compile error here, not a silent
/// park-shaped mislabel: `Merged` and the two non-park terminal statuses
/// (`PostMergeCleanupFailure`, `RecoveryFailure`) are each classified from
/// `frame.status` alone, before any reason-text matching; the three
/// non-terminal statuses resolve to [`ParkClass::InProgress`] rather than
/// falling through to the park path. Only `MergeStatus::Parked` falls
/// through to `park_reason`/reason-text classification; `frame.park_reason`,
/// when present, takes precedence over prefix matching — today only
/// [`ParkReason::StaleBaseOverlap`], which renders from its typed fields
/// rather than the free-text `reason`.
pub(crate) fn explain_frame(frame: &MergeFrame) -> Explanation {
    match frame.status {
        MergeStatus::Merged => return explain_landed(),
        MergeStatus::PostMergeCleanupFailure => {
            let reason = frame
                .reason
                .as_deref()
                .unwrap_or(reasons::POST_MERGE_CLEANUP_FAILED_PREFIX);
            return explain_non_park_terminal(ParkClass::CleanupFailedAfterLanding, reason);
        }
        MergeStatus::RecoveryFailure => {
            let reason = frame
                .reason
                .as_deref()
                .unwrap_or(reasons::RECOVERY_UNCONFIRMED);
            return explain_non_park_terminal(ParkClass::RecoveryUnconfirmed, reason);
        }
        MergeStatus::LockAcquired | MergeStatus::GatesPassed | MergeStatus::Reconciled => {
            return explain_in_progress(frame.stage);
        }
        MergeStatus::Parked => {}
    }
    if let Some(ParkReason::StaleBaseOverlap {
        paths,
        main_commits,
    }) = &frame.park_reason
    {
        let sentence = format!(
            "the run branch and the target branch both changed {} path(s) since the branch's base — {} target commit(s) are responsible — this merge parked before rebasing so the overlap could be reviewed",
            paths.len(),
            main_commits.len()
        );
        return Explanation {
            headline: headline_for(ParkClass::StaleBaseOverlap).to_string(),
            sentence,
            next_action: next_action_for(ParkClass::StaleBaseOverlap).to_string(),
            class: ParkClass::StaleBaseOverlap,
        };
    }
    let reason = frame.reason.as_deref().unwrap_or("no reason recorded");
    let class = classify_reason(reason);
    Explanation {
        headline: headline_for(class).to_string(),
        sentence: sentence_for(class, frame.stage, reason),
        next_action: next_action_for(class).to_string(),
        class,
    }
}

/// Explain a [`MergeReport`] outcome (a retry/CLI result). Typed-first, like
/// [`explain_frame`]: `post-merge-cleanup-failure` and `recovery-failure` —
/// the two terminal outcomes that are not parks — and `lock-timeout`/
/// `lock-unavailable` — the two classes that never write a ledger frame —
/// are all classified from `report.status` alone. Only `report.status ==
/// "parked"` (or an otherwise-unrecognized status) falls back to
/// classifying `report.reason` the same way [`explain_frame`] does; the
/// resulting fallback sentence is neutral about outcome ("did not land") only
/// because every status reaching it there really is un-landed.
pub(crate) fn explain_report(report: &MergeReport) -> Explanation {
    let reason = report.reason.as_deref().unwrap_or(report.status.as_str());
    match report.status.as_str() {
        "post-merge-cleanup-failure" => {
            return explain_non_park_terminal(ParkClass::CleanupFailedAfterLanding, reason);
        }
        "recovery-failure" => {
            return explain_non_park_terminal(ParkClass::RecoveryUnconfirmed, reason);
        }
        _ => {}
    }
    let class = match report.status.as_str() {
        "lock-timeout" => ParkClass::LockTimeout,
        "lock-unavailable" => ParkClass::LockUnavailable,
        _ => classify_reason(reason),
    };
    let sentence = if class == ParkClass::Unknown {
        format!(
            "this merge did not land (status: {}); the recorded reason has no plain-language translation yet",
            report.status
        )
    } else {
        format!("{} — {}", headline_for(class), reason)
    };
    Explanation {
        headline: headline_for(class).to_string(),
        sentence,
        next_action: next_action_for(class).to_string(),
        class,
    }
}

/// Splits a frame/report's raw `evidence` strings into labeled rows: a
/// `gate=<label> argv=<argv> exit=<code> timed-out=<bool>` token becomes a
/// `command`/`result` pair; a `gate-output=<path>` token becomes an `output`
/// row; every other evidence token (revisions, lock wait, overlap paths,
/// retry warnings) renders under its own `key=` prefix as its label, verbatim
/// value — nothing is dropped.
pub(crate) fn gate_rows(evidence: &[String]) -> Vec<GateRow> {
    let mut rows = Vec::new();
    for entry in evidence {
        if let Some(rest) = entry.strip_prefix("gate=") {
            let (label, remainder) = rest.split_once(" argv=").unwrap_or((rest, ""));
            rows.push(GateRow {
                label: "command".to_string(),
                value: label.to_string(),
            });
            if !remainder.is_empty() {
                push_gate_outcome_rows(&mut rows, remainder);
            }
            continue;
        }
        if let Some(path) = entry.strip_prefix("gate-output=") {
            rows.push(GateRow {
                label: "output".to_string(),
                value: path.to_string(),
            });
            continue;
        }
        match entry.split_once('=') {
            Some((label, value)) => rows.push(GateRow {
                label: label.to_string(),
                value: value.to_string(),
            }),
            None => rows.push(GateRow {
                label: "evidence".to_string(),
                value: entry.clone(),
            }),
        }
    }
    rows
}

/// Splits a `gate=` token's `argv=<argv> exit=<code> timed-out=<bool>` tail
/// into an `argv` row (the raw command vector, kept verbatim rather than
/// dropped) and a plain-language `result` row — never a raw `exit=Some(1)
/// timed-out=false` dump.
fn push_gate_outcome_rows(rows: &mut Vec<GateRow>, remainder: &str) {
    let Some((argv, tail)) = remainder.split_once(" exit=") else {
        rows.push(GateRow {
            label: "result".to_string(),
            value: format!("argv={remainder}"),
        });
        return;
    };
    rows.push(GateRow {
        label: "argv".to_string(),
        value: argv.to_string(),
    });
    let result = match tail.split_once(" timed-out=") {
        Some((exit, timed_out)) => gate_result_text(exit, timed_out),
        None => format!("exit={tail}"),
    };
    rows.push(GateRow {
        label: "result".to_string(),
        value: result,
    });
}

/// Plain-language outcome for a gate's `exit=<Rust Option<i32> debug>`/
/// `timed-out=<bool>` pair — never the raw `Some(1)`/`false` tokens.
fn gate_result_text(exit: &str, timed_out: &str) -> String {
    if timed_out == "true" {
        return "timed out".to_string();
    }
    match exit {
        "Some(0)" => "passed".to_string(),
        "None" => "killed by signal (no exit code)".to_string(),
        other => match other
            .strip_prefix("Some(")
            .and_then(|s| s.strip_suffix(')'))
        {
            Some(code) => format!("failed (exit {code})"),
            None => format!("failed (exit {other})"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_core::procedure::session::MergeStatus;

    fn frame(stage: MergeStage, reason: &str) -> MergeFrame {
        MergeFrame {
            stage,
            status: MergeStatus::Parked,
            reason: Some(reason.to_string()),
            evidence: Vec::new(),
            park_reason: None,
            deep_decisions: Vec::new(),
        }
    }

    fn report(status: &str, reason: Option<&str>) -> MergeReport {
        MergeReport {
            status: status.to_string(),
            run_id: "r1".to_string(),
            reason: reason.map(str::to_string),
            ..Default::default()
        }
    }

    // One test per park class: non-empty sentence and non-empty next action,
    // driven from the exact same shared reason constants `merge.rs` formats
    // from.
    #[test]
    fn every_park_class_from_merge_rs_reason_text_translates() {
        use reasons::*;
        let cases: &[(MergeStage, String)] = &[
            (
                MergeStage::Preflight,
                format!("{BRANCH_MISMATCH_PREFIX} \"foo\", expected \"main\" (from config)"),
            ),
            (
                MergeStage::Preflight,
                format!("{INVOCATION_CHECKOUT_DIRTY_PREFIX}main) is not clean"),
            ),
            (MergeStage::Preflight, WORKTREE_UNREGISTERED.to_string()),
            (
                MergeStage::Preflight,
                CHECKOUT_BRANCH_PROBE_FAILED.to_string(),
            ),
            (
                MergeStage::Preflight,
                CHECKOUT_CLEANLINESS_PROBE_FAILED.to_string(),
            ),
            (
                MergeStage::Preflight,
                WORKTREE_CLEANLINESS_PROBE_FAILED.to_string(),
            ),
            (
                MergeStage::Preflight,
                WORKTREE_REBASE_STATE_PROBE_FAILED.to_string(),
            ),
            (
                MergeStage::Preflight,
                format!("worktree \"w1\" {REBASE_IN_PROGRESS_SUFFIX}"),
            ),
            (
                MergeStage::Preflight,
                format!("worktree \"w1\" {WORKTREE_DIRTY_SUFFIX}"),
            ),
            (MergeStage::Rebase, REBASE_FAILED.to_string()),
            (
                MergeStage::Rebase,
                format!("{STALE_BASE_OVERLAP_PREFIX} run branch and main both changed 1 path(s)"),
            ),
            (
                MergeStage::Reconciliation,
                format!("{RECEIPT_INVALID_PREFIX} 2 attempt(s): bad json"),
            ),
            (
                MergeStage::Reconciliation,
                format!("{MERGER_TIMEOUT_PREFIX} after 1000ms (budget 500ms)"),
            ),
            (
                MergeStage::Reconciliation,
                MERGER_IDLE_TIMEOUT_PREFIX.to_string(),
            ),
            (
                MergeStage::Reconciliation,
                MERGER_SIGNAL_KILL_PREFIX.to_string(),
            ),
            (
                MergeStage::Reconciliation,
                format!("{MERGER_NONZERO_EXIT_PREFIX} Some(1): boom"),
            ),
            (
                MergeStage::Reconciliation,
                MERGER_TRUNCATED_PREFIX.to_string(),
            ),
            (
                MergeStage::Reconciliation,
                format!("{DEEP_REFUSE_REGRESSION_PREFIX} (rule refuse-regression): would revert"),
            ),
            (
                MergeStage::Reconciliation,
                format!(
                    "{RECONCILIATION_ITERATION_CAP_PREFIX} 8 merger iterations without landing"
                ),
            ),
            (
                MergeStage::Reconciliation,
                RECONCILIATION_NO_PROGRESS_PREFIX.to_string(),
            ),
            (
                MergeStage::Reconciliation,
                WORKTREE_DIRTY_AFTER_RECONCILIATION.to_string(),
            ),
            (
                MergeStage::Reconciliation,
                format!("{GENERATED_ARTIFACT_REBUILD_FAILED_PREFIX}: [\"just\"] exit=Some(1)"),
            ),
            (
                MergeStage::Gates,
                format!("{GATE_FAILED_PREFIX}just-test failed: exit=Some(1)"),
            ),
            (MergeStage::Gates, GATE_LEFT_DIRTY.to_string()),
            (
                MergeStage::Landing,
                format!("main {MAIN_ADVANCED_INFIX} deadbeef to cafebabe"),
            ),
            (MergeStage::Landing, format!("main {MAIN_NOT_CLEAN_INFIX}")),
            (MergeStage::Landing, FAST_FORWARD_FAILED.to_string()),
            (
                MergeStage::Cleanup,
                format!("{HARVEST_ADJUDICATION_PARKED_PREFIX} refuse"),
            ),
            (MergeStage::Cleanup, HARVEST_DIGEST_MISMATCH.to_string()),
            (
                MergeStage::Cleanup,
                HARVEST_ANCESTOR_UNAVAILABLE.to_string(),
            ),
            (MergeStage::Cleanup, HARVEST_CHANGED_BOTH_SIDES.to_string()),
            (
                MergeStage::Rebase,
                format!("{UNEXPECTED_FAILURE_PREFIX}: boom"),
            ),
        ];
        for (stage, reason) in cases {
            let explanation = explain_frame(&frame(*stage, reason));
            assert!(
                !explanation.sentence.is_empty(),
                "empty sentence for {reason:?}"
            );
            assert!(
                !explanation.next_action.is_empty(),
                "empty next action for {reason:?}"
            );
            assert_ne!(
                explanation.class,
                ParkClass::Unknown,
                "reason {reason:?} unexpectedly fell back to Unknown"
            );
        }
    }

    // Non-park terminal outcomes must never be reported as parked (a
    // shipped defect this test guards against reopening): a
    // `PostMergeCleanupFailure` frame/report says the merge landed, and a
    // `RecoveryFailure` frame/report makes no claim about the branch/
    // worktree being intact — neither ever says "parked" or "did not land",
    // and the cleanup case never suggests "retry" (the merge already
    // happened; retrying it would be wrong).
    #[test]
    fn cleanup_and_recovery_failures_are_never_reported_as_parked() {
        let cleanup_frame = MergeFrame {
            stage: MergeStage::Cleanup,
            status: MergeStatus::PostMergeCleanupFailure,
            reason: Some(format!(
                "{} fast-forward to deadbeef: worktree removal failed",
                reasons::POST_MERGE_CLEANUP_FAILED_PREFIX
            )),
            evidence: Vec::new(),
            park_reason: None,
            deep_decisions: Vec::new(),
        };
        let explanation = explain_frame(&cleanup_frame);
        assert_eq!(explanation.class, ParkClass::CleanupFailedAfterLanding);
        assert!(!explanation.sentence.contains("parked"));
        assert!(!explanation.sentence.contains("did not land"));
        assert!(!explanation.next_action.contains("retry"));

        let recovery_frame = MergeFrame {
            stage: MergeStage::Rebase,
            status: MergeStatus::RecoveryFailure,
            reason: Some(reasons::RECOVERY_UNCONFIRMED.to_string()),
            evidence: Vec::new(),
            park_reason: None,
            deep_decisions: Vec::new(),
        };
        let explanation = explain_frame(&recovery_frame);
        assert_eq!(explanation.class, ParkClass::RecoveryUnconfirmed);
        assert!(!explanation.sentence.contains("parked"));
        assert!(!explanation.sentence.contains("did not land"));

        let cleanup_report = report(
            "post-merge-cleanup-failure",
            Some(
                "post-merge cleanup failed after fast-forward to deadbeef: branch delete failed: boom",
            ),
        );
        let explanation = explain_report(&cleanup_report);
        assert_eq!(explanation.class, ParkClass::CleanupFailedAfterLanding);
        assert!(!explanation.sentence.contains("parked"));
        assert!(!explanation.sentence.contains("did not land"));
        assert!(!explanation.next_action.contains("retry"));

        let recovery_report = report(
            "recovery-failure",
            Some(
                "rebase recovery unconfirmed: could not verify the in-progress rebase was aborted: boom",
            ),
        );
        let explanation = explain_report(&recovery_report);
        assert_eq!(explanation.class, ParkClass::RecoveryUnconfirmed);
        assert!(!explanation.sentence.contains("parked"));
        assert!(!explanation.sentence.contains("did not land"));
    }

    // A `Merged` frame — the fourth terminal status, and by far the most
    // common one — must never fall through to the park classifier: no
    // `Unknown` class, and no "unrecognized"/"parked" text anywhere in the
    // explanation. Guards the recurrence of
    // `merged-frames-still-routed-through-the-park-classifier`.
    #[test]
    fn merged_frame_is_never_routed_through_the_park_classifier() {
        let merged_frame = MergeFrame {
            stage: MergeStage::Landing,
            status: MergeStatus::Merged,
            reason: None,
            evidence: Vec::new(),
            park_reason: None,
            deep_decisions: Vec::new(),
        };
        let explanation = explain_frame(&merged_frame);
        assert_eq!(explanation.class, ParkClass::Landed);
        assert_ne!(explanation.class, ParkClass::Unknown);
        assert!(!explanation.headline.contains("unrecognized"));
        assert!(!explanation.headline.contains("parked"));
        assert!(!explanation.sentence.contains("unrecognized"));
        assert!(!explanation.sentence.contains("parked"));
    }

    // The three non-terminal statuses must also resolve safely rather than
    // fall through to the park path — `explain_frame`'s match is total, not
    // just total for the four terminal statuses.
    #[test]
    fn non_terminal_frame_is_never_routed_through_the_park_classifier() {
        for status in [
            MergeStatus::LockAcquired,
            MergeStatus::GatesPassed,
            MergeStatus::Reconciled,
        ] {
            let in_progress_frame = MergeFrame {
                stage: MergeStage::Reconciliation,
                status,
                reason: None,
                evidence: Vec::new(),
                park_reason: None,
                deep_decisions: Vec::new(),
            };
            let explanation = explain_frame(&in_progress_frame);
            assert_eq!(
                explanation.class,
                ParkClass::InProgress,
                "status {status:?}"
            );
            assert!(!explanation.sentence.contains("unrecognized"));
            assert!(!explanation.sentence.contains("this merge parked"));
        }
    }

    // Total test: every `ParkClass` variant has both a non-empty headline and
    // a non-empty next action — a new class added later cannot silently ship
    // untranslated.
    #[test]
    fn every_park_class_variant_has_headline_and_next_action() {
        for class in ParkClass::all() {
            assert!(!headline_for(*class).is_empty());
            assert!(!next_action_for(*class).is_empty());
        }
    }

    #[test]
    fn stale_base_overlap_renders_from_typed_fields_not_debug_dump() {
        let frame = MergeFrame {
            stage: MergeStage::Rebase,
            status: MergeStatus::Parked,
            reason: Some("stale-base overlap: ...".to_string()),
            evidence: Vec::new(),
            park_reason: Some(ParkReason::StaleBaseOverlap {
                paths: vec!["a.rs".to_string(), "b.rs".to_string()],
                main_commits: vec!["deadbeef".to_string()],
            }),
            deep_decisions: Vec::new(),
        };
        let explanation = explain_frame(&frame);
        assert_eq!(explanation.class, ParkClass::StaleBaseOverlap);
        assert!(!explanation.sentence.contains("StaleBaseOverlap {"));
        assert!(explanation.sentence.contains('2'));
    }

    #[test]
    fn unrecognized_reason_falls_back_to_unknown_with_next_action() {
        let explanation = explain_frame(&frame(
            MergeStage::Gates,
            "a brand new reason nobody wrote a translation for",
        ));
        assert_eq!(explanation.class, ParkClass::Unknown);
        assert!(explanation.sentence.contains("gates"));
        assert!(!explanation.next_action.is_empty());
    }

    #[test]
    fn explain_report_covers_lock_timeout_and_lock_unavailable() {
        let timeout = explain_report(&report(
            "lock-timeout",
            Some("timed out after 500ms waiting for merge lock held by pid 1"),
        ));
        assert_eq!(timeout.class, ParkClass::LockTimeout);
        assert!(!timeout.next_action.is_empty());

        let unavailable = explain_report(&report(
            "lock-unavailable",
            Some("merge lock held by another process"),
        ));
        assert_eq!(unavailable.class, ParkClass::LockUnavailable);
        assert!(!unavailable.next_action.is_empty());
    }

    #[test]
    fn gate_rows_parses_gate_evidence_and_preserves_unknown_tokens() {
        let evidence = vec![
            "gate=just-test argv=[\"just\", \"test\"] exit=Some(1) timed-out=false".to_string(),
            "gate-output=/tmp/capture.txt".to_string(),
            "target=main@deadbeef".to_string(),
            "just-a-bare-token".to_string(),
        ];
        let rows = gate_rows(&evidence);
        assert_eq!(rows[0].label, "command");
        assert_eq!(rows[0].value, "just-test");
        assert_eq!(rows[1].label, "argv");
        assert_eq!(rows[1].value, "[\"just\", \"test\"]");
        assert_eq!(rows[2].label, "result");
        assert_eq!(rows[2].value, "failed (exit 1)");
        assert_eq!(rows[3].label, "output");
        assert_eq!(rows[3].value, "/tmp/capture.txt");
        assert_eq!(rows[4].label, "target");
        assert_eq!(rows[4].value, "main@deadbeef");
        assert_eq!(rows[5].label, "evidence");
        assert_eq!(rows[5].value, "just-a-bare-token");
    }

    // The friendly result text never leaks the raw `Some(_)`/`None`/
    // `timed-out=` tokens the ledger evidence encodes it with.
    #[test]
    fn gate_result_text_is_never_a_raw_debug_dump() {
        assert_eq!(gate_result_text("Some(0)", "false"), "passed");
        assert_eq!(gate_result_text("Some(1)", "false"), "failed (exit 1)");
        assert_eq!(
            gate_result_text("None", "false"),
            "killed by signal (no exit code)"
        );
        assert_eq!(gate_result_text("Some(1)", "true"), "timed out");
    }

    // P549: every `MergeStage`×relevant-`MergeStatus` combination projects to
    // a live activity event with non-empty text — the same coverage
    // `every_park_class_from_merge_rs_reason_text_translates` already gives
    // `explain_frame` for park reasons, extended to the live projection.
    #[test]
    fn every_stage_and_status_projects_to_a_nonempty_activity_event() {
        let stages = [
            MergeStage::Lock,
            MergeStage::Preflight,
            MergeStage::Rebase,
            MergeStage::Reconciliation,
            MergeStage::Gates,
            MergeStage::Landing,
            MergeStage::Cleanup,
        ];
        let statuses = [
            MergeStatus::LockAcquired,
            MergeStatus::GatesPassed,
            MergeStatus::Reconciled,
            MergeStatus::Merged,
            MergeStatus::Parked,
            MergeStatus::PostMergeCleanupFailure,
            MergeStatus::RecoveryFailure,
        ];
        for stage in stages {
            for status in statuses {
                let frame = MergeFrame {
                    stage,
                    status,
                    reason: Some("some reason".to_string()),
                    evidence: Vec::new(),
                    park_reason: None,
                    deep_decisions: Vec::new(),
                };
                let draft = activity_event(&MergeProgress::FrameRecorded(&frame));
                assert!(
                    draft.text.as_deref().is_some_and(|text| !text.is_empty()),
                    "empty text for {stage:?}/{status:?}"
                );
                assert_eq!(draft.frame_id, format!("merge:{}", stage_text(stage)));
                let expected_kind = if matches!(
                    status,
                    MergeStatus::Parked
                        | MergeStatus::PostMergeCleanupFailure
                        | MergeStatus::RecoveryFailure
                ) {
                    ActivityKind::Stalled
                } else {
                    ActivityKind::ValidatingOutput
                };
                assert_eq!(
                    draft.kind, expected_kind,
                    "wrong kind for {stage:?}/{status:?}"
                );
            }
        }
    }

    // P549's core invariant: a `Parked` frame's live event text equals
    // `explain_frame`'s sentence, so the live pane, `story`, and the MERGES
    // screen all say the exact same thing about the same park.
    #[test]
    fn parked_frame_live_text_equals_explain_frame_sentence() {
        let parked = frame(
            MergeStage::Gates,
            "pre-landing gate just-test failed: exit=Some(1)",
        );
        let draft = activity_event(&MergeProgress::FrameRecorded(&parked));
        assert_eq!(
            draft.text.as_deref(),
            Some(explain_frame(&parked).sentence.as_str())
        );
    }

    // Gate events carry the gate command label as `tool` — the P549 gate loop
    // must be able to attribute a gate's activity to the exact command it
    // covers, not a bare "running a gate" line.
    #[test]
    fn gate_events_carry_the_gate_command_label_as_tool() {
        let started = activity_event(&MergeProgress::GateCommandStarted { label: "just-test" });
        assert_eq!(started.tool.as_deref(), Some("just-test"));
        assert_eq!(started.frame_id, "merge:gates");

        let finished = activity_event(&MergeProgress::GateCommandFinished {
            label: "just-test",
            exit_code: Some(1),
            timed_out: false,
        });
        assert_eq!(finished.tool.as_deref(), Some("just-test"));
        assert!(
            finished
                .text
                .as_deref()
                .unwrap()
                .contains("failed (exit 1)")
        );
    }
}
