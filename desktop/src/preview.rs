//! The Sessions preview run block and footer: plain Rust, no gpui types, so
//! the content contract is assertable in plain `#[test]`s here, mirroring
//! `bottom_bar.rs`'s model/view split. `preview_view.rs` paints exactly this
//! model.
//!
//! Every identity/state fact rendered here (`run_id`, `trait_id`,
//! `elapsed_seconds`, `started_at_epoch`, the variant, the task answer) comes
//! from the one accepted `DetailBaseline` a [`crate::detail::PreviewState`]
//! carries — specifically from `DetailBaseline::row`, the exact `RunRow`
//! candidate its `LoadRequest` was issued for and committed alongside, never
//! from a `RunRow` read separately out of `CenterFace`. That is what keeps a
//! resync's new run/elapsed from ever being painted beside a stale
//! variant/task (review-verdict-1 blocker `selected-preview-not-atomic`).

use std::time::Duration;

use ctx_traits_core::procedure::activity::compact_elapsed_text;
use ctx_traits_core::procedure::runtime::{loop_rounds, loop_rounds_label};
use ctx_traits_core::procedure::story::{
    SlotValueForm, VerdictTone, finding_count_segment, verdict_presentation,
};
use ctx_traits_io::center::{ClaimedTaskResult, ClosePolicyResolution};
use ctx_traits_io::run_summary::RunSummary;

use crate::detail::{DetailBaseline, PreviewState};
use crate::detail_tree::current_position_path;
use crate::run_row::{
    RowState, RunRow, StateRole, presentation as row_presentation, task_status_presentation,
};

/// A value segment with its own tone. `None` marks "identity, deliberately
/// not a state" (rule 7's actual distinction) — resolved to `tokens::TEXT`
/// by the view; `Some(role)` resolves through the existing `role_color`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueSegment {
    pub text: String,
    pub role: Option<StateRole>,
}

impl ValueSegment {
    pub fn neutral(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            role: None,
        }
    }

    pub fn toned(text: impl Into<String>, role: StateRole) -> Self {
        Self {
            text: text.into(),
            role: Some(role),
        }
    }

    /// The one separator glyph the design grammar allows, with ASCII spaces
    /// on both sides so concatenating a row's segment texts reads
    /// `identity · value`, never `identity·value`.
    pub fn dot() -> Self {
        Self::neutral(" \u{b7} ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyValueRow {
    pub key: String,
    pub value: Vec<ValueSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedBlock {
    pub heading: String,
    pub rows: Vec<KeyValueRow>,
}

fn trait_row(row: &RunRow, baseline: &DetailBaseline) -> KeyValueRow {
    let mut value = vec![ValueSegment::neutral(row.trait_id.clone())];
    match &baseline.variant {
        Ok(Some(variant)) => {
            value.push(ValueSegment::dot());
            value.push(ValueSegment::neutral(variant.clone()));
        }
        Ok(None) => {}
        Err(_reason) => {
            value.push(ValueSegment::dot());
            value.push(ValueSegment::toned("variant unresolved", StateRole::Danger));
        }
    }
    KeyValueRow {
        key: "trait".to_string(),
        value,
    }
}

fn run_row_row(row: &RunRow) -> KeyValueRow {
    let elapsed = compact_elapsed_text(Duration::from_secs(row.elapsed_seconds));
    KeyValueRow {
        key: "run".to_string(),
        value: vec![
            ValueSegment::neutral(row.run_id.clone()),
            ValueSegment::dot(),
            ValueSegment::neutral(elapsed),
        ],
    }
}

/// The one claimed-task-outcome mapping, shared by [`task_row`] and
/// `screen_header::sessions_header` so the preview and the header can never
/// disagree about the same run's same claimed-task answer on the same
/// screen. `Task` carries the served value itself (the caller composes its
/// own presentation from it); the other four outcomes carry the one
/// non-success wording each renders, everywhere.
pub(crate) enum ClaimedTaskWording<'a> {
    Task(
        &'a ctx_traits_core::task::provider::ClaimedTask,
        &'a ClosePolicyResolution,
    ),
    Wording {
        text: &'static str,
        danger: bool,
    },
}

pub(crate) fn claimed_task_wording(
    claimed_task: &Result<ClaimedTaskResult, String>,
) -> ClaimedTaskWording<'_> {
    match claimed_task {
        Ok(ClaimedTaskResult::Task(task, policy)) => ClaimedTaskWording::Task(task, policy),
        Ok(ClaimedTaskResult::Unclaimed) => ClaimedTaskWording::Wording {
            text: "no task claimed",
            danger: false,
        },
        Ok(ClaimedTaskResult::Missing) => ClaimedTaskWording::Wording {
            text: "task unavailable: missing",
            danger: true,
        },
        Ok(ClaimedTaskResult::Ambiguous(_)) => ClaimedTaskWording::Wording {
            text: "task unavailable: ambiguous session",
            danger: true,
        },
        Err(_reason) => ClaimedTaskWording::Wording {
            text: "task unavailable: center error",
            danger: true,
        },
    }
}

fn task_row(baseline: &DetailBaseline) -> KeyValueRow {
    let value = match claimed_task_wording(&baseline.claimed_task) {
        ClaimedTaskWording::Task(task, _policy) => {
            let mut value = vec![ValueSegment::neutral(task.key.clone())];
            if let Some(status) = task.stored_status {
                let presented = task_status_presentation(status);
                value.push(ValueSegment::dot());
                value.push(ValueSegment::toned(presented.word, presented.role));
            }
            value
        }
        ClaimedTaskWording::Wording {
            text,
            danger: false,
        } => vec![ValueSegment::neutral(text)],
        ClaimedTaskWording::Wording { text, danger: true } => {
            vec![ValueSegment::toned(text, StateRole::Danger)]
        }
    };
    KeyValueRow {
        key: "task".to_string(),
        value,
    }
}

fn pending_block() -> NamedBlock {
    let pending = vec![ValueSegment::neutral("loading")];
    NamedBlock {
        heading: "run".to_string(),
        rows: vec![
            KeyValueRow {
                key: "trait".to_string(),
                value: pending.clone(),
            },
            KeyValueRow {
                key: "run".to_string(),
                value: pending.clone(),
            },
            KeyValueRow {
                key: "task".to_string(),
                value: pending,
            },
        ],
    }
}

fn unreadable_rows(text: String) -> Vec<KeyValueRow> {
    let unreadable = vec![ValueSegment::toned(text, StateRole::Danger)];
    vec![
        KeyValueRow {
            key: "trait".to_string(),
            value: unreadable.clone(),
        },
        KeyValueRow {
            key: "run".to_string(),
            value: unreadable.clone(),
        },
        KeyValueRow {
            key: "task".to_string(),
            value: unreadable,
        },
    ]
}

fn failed_block(reason: &str) -> NamedBlock {
    let text = if reason.is_empty() {
        "unreadable".to_string()
    } else {
        format!("unreadable \u{b7} {reason}")
    };
    NamedBlock {
        heading: "run".to_string(),
        rows: unreadable_rows(text),
    }
}

/// Goal 8: a projected row already flagged `RowState::Unreadable` (derived
/// from the ledger's own `parse_error`) renders unreadable — no `0s`, no
/// empty identity, no invented task — even though the ledger read itself
/// succeeded well enough to produce an accepted `DetailBaseline`.
fn unreadable_row_block(row: &RunRow) -> NamedBlock {
    let text = if row.detail_text.is_empty() {
        "unreadable".to_string()
    } else {
        format!("unreadable \u{b7} {}", row.detail_text)
    };
    NamedBlock {
        heading: "run".to_string(),
        rows: unreadable_rows(text),
    }
}

/// Compose the `run` identity block from the one accepted preview
/// projection. `Loading`/`Failed` render their own honest, non-fabricated
/// shape; only `Accepted` reaches into the baseline for trait/run/task —
/// and every one of those three rows reads the same baseline, so they can
/// never disagree about which run they describe.
/// The heading carries the posture marker: a settled, current baseline reads
/// plain `run`; a resync in flight (fingerprint move, or recovery from a
/// prior outage) reads `run · refreshing` even though `baseline` still shows
/// the last-accepted facts; a retained baseline behind a lost connection
/// reads `run · stale` — never silently identical to current (review-
/// verdict-1 blocker `selected-preview-not-atomic`).
/// The one stale/refreshing marker word, shared by every block that must
/// not paint a resync-in-flight or connection-lost baseline as if it were
/// current — `sessions_run_block`'s heading, `sessions_footer`, and
/// `screen_header::sessions_header` all resolve through this rather than
/// inventing their own wording for the same two postures.
pub(crate) fn staleness_word(stale: Option<&str>, refreshing: bool) -> Option<&'static str> {
    if stale.is_some() {
        Some("stale")
    } else if refreshing {
        Some("refreshing")
    } else {
        None
    }
}

/// `frame N of M` — the 0265.14 counter shared by the Sessions bottom bar's
/// detail segment and the screen header's summary slot. Every absence
/// (`NoCountedFrames`, `NoneReached`, or a resolution `Err`) renders no
/// segment rather than a fabricated `frame 0 of 0`/`frame 0 of M`.
pub(crate) fn frame_counter_text(
    progress: &Result<ctx_traits_core::procedure::run::RunProgress, String>,
) -> Option<String> {
    match progress {
        Ok(ctx_traits_core::procedure::run::RunProgress::Reached { ordinal, total }) => {
            Some(format!("frame {ordinal} of {total}"))
        }
        _ => None,
    }
}

fn accepted_heading(stale: Option<&str>, refreshing: bool) -> String {
    match staleness_word(stale, refreshing) {
        Some(word) => format!("run \u{b7} {word}"),
        None => "run".to_string(),
    }
}

pub fn sessions_run_block(state: Option<&PreviewState<'_>>) -> NamedBlock {
    match state {
        None | Some(PreviewState::Loading) => pending_block(),
        Some(PreviewState::Failed(reason)) => failed_block(reason),
        Some(PreviewState::Accepted {
            baseline,
            stale,
            refreshing,
            ..
        }) => {
            if baseline.row.state == RowState::Unreadable {
                return unreadable_row_block(&baseline.row);
            }
            NamedBlock {
                heading: accepted_heading(*stale, *refreshing),
                rows: vec![
                    trait_row(&baseline.row, baseline),
                    run_row_row(&baseline.row),
                    task_row(baseline),
                ],
            }
        }
    }
}

/// The footer text: `<run id> · started HH:MM`, or just the run id when
/// `started_at_epoch` is `None` — no epoch zero, no current clock, no
/// dangling separator. `Loading`/`Failed` render their own non-empty,
/// non-fabricated text rather than an empty string standing in for "nothing
/// to show".
pub fn sessions_footer(state: Option<&PreviewState<'_>>) -> String {
    match state {
        None | Some(PreviewState::Loading) => "loading".to_string(),
        Some(PreviewState::Failed(reason)) => {
            if reason.is_empty() {
                "unreadable".to_string()
            } else {
                format!("unreadable \u{b7} {reason}")
            }
        }
        Some(PreviewState::Accepted {
            baseline,
            stale,
            refreshing,
            ..
        }) => {
            if baseline.row.state == RowState::Unreadable {
                let text = if baseline.row.detail_text.is_empty() {
                    "unreadable".to_string()
                } else {
                    format!("unreadable \u{b7} {}", baseline.row.detail_text)
                };
                return text;
            }
            let base = match baseline.row.started_at_epoch {
                Some(epoch) => {
                    let offset = ctx_traits_io::clock::local_utc_offset_seconds(epoch);
                    let clock = ctx_traits_io::clock::epoch_clock_minutes(epoch, offset);
                    format!("{} \u{b7} started {clock}", baseline.row.run_id)
                }
                None => baseline.row.run_id.clone(),
            };
            if stale.is_some() {
                format!("{base} \u{b7} stale")
            } else if *refreshing {
                format!("{base} \u{b7} refreshing")
            } else {
                base
            }
        }
    }
}

/// The `in progress` block's one bordered "now" item: a served frame title
/// (or the fixed absence literal, muted) opposite the served state word,
/// plus a narrated line. Present only while the selection is live and
/// readable (goal 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowItem {
    pub title: String,
    pub title_muted: bool,
    pub state_word: String,
    pub state_role: StateRole,
    pub narration: String,
}

/// The `verdict` block: a heading round, one `status` key/value row, and
/// zero or more blocker lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerdictBlock {
    pub heading: String,
    pub status_row: KeyValueRow,
    pub blocker_lines: Vec<String>,
}

fn verdict_tone_to_state_role(tone: VerdictTone) -> StateRole {
    match tone {
        VerdictTone::Ok => StateRole::Ok,
        VerdictTone::Warn => StateRole::Warn,
        VerdictTone::Neutral => StateRole::Neutral,
        VerdictTone::Danger => StateRole::Danger,
    }
}

/// `None` unless the selection is `Accepted`, settled (not `stale`, not
/// `refreshing`), readable (not `RowState::Unreadable`), and currently
/// live — `live` is the selection's current liveness, refreshed from every
/// center delta, never the frozen `baseline.row.live` a stale or refreshing
/// selection may have loaded under. Returning `None` (not a block with a
/// dropped marker) is what makes the whole block vanish on a live-to-finished
/// transition, including a fingerprint-identical one that never triggers a
/// resync.
pub fn sessions_now_item(state: Option<&PreviewState<'_>>) -> Option<NowItem> {
    let Some(PreviewState::Accepted {
        baseline,
        stale,
        refreshing,
        live,
        ..
    }) = state
    else {
        return None;
    };
    if stale.is_some() || *refreshing {
        return None;
    }
    if baseline.row.state == RowState::Unreadable || !*live {
        return None;
    }

    let summary = RunSummary::from_session(&baseline.session);
    let rounds = loop_rounds(&current_position_path(&baseline.session));
    let round_label = loop_rounds_label(&rounds);

    let (title, title_muted) = match summary
        .current_sequence_title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
    {
        Some(title) => {
            let title = match round_label {
                Some(round) => format!("{title} \u{b7} {round}"),
                None => title.to_string(),
            };
            (title, false)
        }
        None => ("no current frame".to_string(), true),
    };

    // `*live` is already established true above; derive the marker from the
    // shared live presentation rather than the frozen `baseline.row.state`,
    // which a fingerprint-identical liveness-only delta never replaces.
    let state_presentation = row_presentation(&RowState::Live);

    Some(NowItem {
        title,
        title_muted,
        state_word: state_presentation.word.to_string(),
        state_role: state_presentation.role,
        narration: crate::placeholders::NOW_NARRATION.label.to_string(),
    })
}

/// `None` unless the selection is `Accepted`, settled (not `stale`, not
/// `refreshing`), readable, and recognised verdict evidence carries a round
/// (goals 5, 6, 7). Withholding the block while stale or refreshing keeps it
/// from presenting last-accepted evidence as current.
pub fn sessions_verdict_block(state: Option<&PreviewState<'_>>) -> Option<VerdictBlock> {
    let Some(PreviewState::Accepted {
        baseline,
        stale,
        refreshing,
        ..
    }) = state
    else {
        return None;
    };
    if stale.is_some() || *refreshing {
        return None;
    }
    if baseline.row.state == RowState::Unreadable {
        return None;
    }

    let presentation = verdict_presentation(
        &baseline.session.accepted_slot_values,
        &baseline.session.slot_revisions,
    )?;

    let heading = format!("verdict \u{b7} round {}", presentation.round);

    let role = verdict_tone_to_state_role(presentation.tone);
    let mut value = vec![ValueSegment::toned(presentation.status_word.clone(), role)];
    if let Some(segment) = finding_count_segment(presentation.finding_count) {
        value.push(ValueSegment::dot());
        value.push(ValueSegment::neutral(segment));
    }
    let status_row = KeyValueRow {
        key: "status".to_string(),
        value,
    };

    let blocker_lines = presentation
        .blockers
        .into_iter()
        .map(|blocker| match blocker.what {
            Some(what) => format!("{} \u{b7} {what}", blocker.id),
            None => blocker.id,
        })
        .collect();

    Some(VerdictBlock {
        heading,
        status_row,
        blocker_lines,
    })
}

/// `None` unless the selection is `Accepted`, settled (not `stale`, not
/// `refreshing`), readable, and the run's committed evidence carries at
/// least one slot row (goal 2 — no evidence renders no block, not an empty
/// heading). Reuses [`NamedBlock`]/`named_block_element` unchanged: this is
/// the same heading-over-key/value-rows composition `0265.10` already
/// paints, not a new form (goal 6).
pub fn sessions_slots_block(state: Option<&PreviewState<'_>>) -> Option<NamedBlock> {
    let Some(PreviewState::Accepted {
        baseline,
        stale,
        refreshing,
        ..
    }) = state
    else {
        return None;
    };
    if stale.is_some() || *refreshing {
        return None;
    }
    if baseline.row.state == RowState::Unreadable {
        return None;
    }

    let ledger_rows = ctx_traits_core::procedure::story::slot_ledger_rows(
        &baseline.session.accepted_slot_values,
        &baseline.session.slot_revisions,
    );
    if ledger_rows.is_empty() {
        return None;
    }

    let rows = ledger_rows
        .into_iter()
        .map(|row| {
            let value = match row.form {
                SlotValueForm::Filled => vec![ValueSegment::neutral("filled")],
                SlotValueForm::FilledWithWrites(n) => vec![
                    ValueSegment::neutral("filled"),
                    ValueSegment::dot(),
                    ValueSegment::neutral(format!("r{n}")),
                ],
                SlotValueForm::Unreadable => {
                    vec![ValueSegment::toned("unreadable", StateRole::Danger)]
                }
            };
            KeyValueRow {
                key: row.slot_id,
                value,
            }
        })
        .collect();

    Some(NamedBlock {
        heading: "slots".to_string(),
        rows,
    })
}

/// One `landing` block line: fixed/served text plus the tone it renders in.
/// `role: None` is the block's own default (`text-secondary`, rule 5) —
/// deliberately not [`ValueSegment`]'s "identity, no state" `None`
/// (`tokens::TEXT`), since every landing line is a stated fact, never an
/// identity value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandingLine {
    pub text: String,
    pub role: Option<StateRole>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandingBlock {
    pub heading: String,
    pub lines: Vec<LandingLine>,
}

fn landing_tone_role(tone: ctx_traits_core::procedure::landing::LineTone) -> Option<StateRole> {
    use ctx_traits_core::procedure::landing::LineTone;
    match tone {
        LineTone::Neutral => None,
        LineTone::Ok => Some(StateRole::Ok),
        LineTone::Warn => Some(StateRole::Warn),
        LineTone::Danger => Some(StateRole::Danger),
    }
}

fn landing_line(line: ctx_traits_core::procedure::landing::LandingLine) -> LandingLine {
    LandingLine {
        role: landing_tone_role(line.tone),
        text: line.text,
    }
}

/// Goal 5/7: the served claimed-task answer, reduced to what the core
/// projection needs — never the desktop's own guess at what "unresolved"
/// means, and never carrying the config-failure reason text (the line never
/// renders it).
fn claimed_task_close_outcome(
    claimed_task: &Result<ClaimedTaskResult, String>,
) -> ctx_traits_core::procedure::landing::ClaimedTaskCloseOutcome<'_> {
    use ctx_traits_core::procedure::landing::{ClaimedTaskCloseOutcome, ResolvedClosePolicy};
    match claimed_task {
        Ok(ClaimedTaskResult::Unclaimed) => ClaimedTaskCloseOutcome::Unclaimed,
        Ok(ClaimedTaskResult::Task(task, policy)) => ClaimedTaskCloseOutcome::Claimed {
            task_key: &task.key,
            policy: match policy {
                ClosePolicyResolution::Effective(policy) => ResolvedClosePolicy::Effective(*policy),
                ClosePolicyResolution::NoneConfigured => ResolvedClosePolicy::NoneConfigured,
                ClosePolicyResolution::Unresolved(_) => ResolvedClosePolicy::Unresolved,
            },
        },
        Ok(ClaimedTaskResult::Missing) | Ok(ClaimedTaskResult::Ambiguous(_)) | Err(_) => {
            ClaimedTaskCloseOutcome::Unavailable
        }
    }
}

/// `None` unless the selection is `Accepted`, settled (not `stale`, not
/// `refreshing`), and readable — the same gate [`sessions_slots_block`]
/// applies. Unlike slots, a block always renders once that gate passes: the
/// three lines' truthful absence forms guarantee three lines regardless of
/// how little evidence the run carries (goal 1).
pub fn sessions_landing_block(state: Option<&PreviewState<'_>>) -> Option<LandingBlock> {
    let Some(PreviewState::Accepted {
        baseline,
        stale,
        refreshing,
        ..
    }) = state
    else {
        return None;
    };
    if stale.is_some() || *refreshing {
        return None;
    }
    if baseline.row.state == RowState::Unreadable {
        return None;
    }

    let provenance = &baseline.session.provenance;
    let close_outcome = claimed_task_close_outcome(&baseline.claimed_task);
    let lines = ctx_traits_core::procedure::landing::landing_lines(
        provenance.worktree.as_ref(),
        provenance.merge_intent,
        &provenance.merge_frames,
        &close_outcome,
    );

    Some(LandingBlock {
        heading: "landing".to_string(),
        lines: vec![
            landing_line(lines.worktree),
            landing_line(lines.merge),
            landing_line(lines.close),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_core::task::TaskStatus;
    use ctx_traits_core::task::provider::ClaimedTask;

    fn fixture_session(run_id: &str, trait_id: &str, elapsed_seconds: u64) -> serde_json::Value {
        serde_json::json!({
            "schema-version": "0.1.0",
            "session-id": "session",
            "run-id": run_id,
            "trait-id": trait_id,
            "current-run-index": 0,
            "status": "completed",
            "provenance": {
                "started-by": {"surface": "test", "caller": "preview-fixture"},
                "state-source": "test",
            },
            "ledger": {
                "run-id": run_id,
                "trait-id": trait_id,
                "current-run-index": 0,
                "final-state": "completed",
                "elapsed-seconds": elapsed_seconds,
            },
            "state-digest": "sha256:fixture",
        })
    }

    fn baseline_with(
        variant: Result<Option<String>, String>,
        claimed_task: Result<ClaimedTaskResult, String>,
    ) -> DetailBaseline {
        baseline_with_run("run-1", "implement-phase", 2_520, variant, claimed_task)
    }

    fn fixture_row(run_id: &str, trait_id: &str, elapsed_seconds: u64) -> RunRow {
        RunRow {
            ledger_path: format!("/repo/{run_id}.json"),
            session_id: "session".to_string(),
            run_id: run_id.to_string(),
            repo_key: "repo".to_string(),
            repo_path: "/repo".to_string(),
            repo_label: "repo".to_string(),
            title: run_id.to_string(),
            session_title: None,
            trait_id: trait_id.to_string(),
            state: RowState::Live,
            state_text: "live".to_string(),
            detail_text: String::new(),
            elapsed_text: String::new(),
            tokens_text: "-".to_string(),
            live: true,
            modified_epoch_secs: 0,
            verdict_rounds: None,
            elapsed_seconds,
            started_at_epoch: None,
        }
    }

    fn baseline_with_run(
        run_id: &str,
        trait_id: &str,
        elapsed_seconds: u64,
        variant: Result<Option<String>, String>,
        claimed_task: Result<ClaimedTaskResult, String>,
    ) -> DetailBaseline {
        DetailBaseline {
            session: serde_json::from_value(fixture_session(run_id, trait_id, elapsed_seconds))
                .expect("fixture session"),
            activity_overlay: crate::detail_tree::ActivityOverlay::default(),
            skipped_activity_lines: 0,
            variant,
            claimed_task,
            progress: Ok(ctx_traits_core::procedure::run::RunProgress::NoCountedFrames),
            row: fixture_row(run_id, trait_id, elapsed_seconds),
        }
    }

    fn accepted(baseline: &DetailBaseline) -> PreviewState<'_> {
        PreviewState::Accepted {
            baseline,
            stale: None,
            refreshing: false,
            live: baseline.row.live,
            session_title: None,
        }
    }

    #[test]
    fn only_the_state_segment_carries_a_role_never_identity_or_the_dot() {
        let task = ClaimedTask {
            key: "0265.10".to_string(),
            title: "title".to_string(),
            description: String::new(),
            stored_status: Some(TaskStatus::Done),
            auto_close: None,
        };
        let baseline = baseline_with(
            Ok(Some("basic".to_string())),
            Ok(ClaimedTaskResult::Task(
                Box::new(task),
                ClosePolicyResolution::NoneConfigured,
            )),
        );
        let state = accepted(&baseline);
        let block = sessions_run_block(Some(&state));
        let task_row = &block.rows[2];
        assert_eq!(task_row.value[0].role, None, "the key/task-id is neutral");
        assert_eq!(task_row.value[1].role, None, "the dot is neutral");
        assert_eq!(
            task_row.value[2].role,
            Some(StateRole::Ok),
            "the state word carries the role"
        );
    }

    #[test]
    fn compound_values_join_into_the_spaced_dot_form() {
        let task = ClaimedTask {
            key: "0243.4".to_string(),
            title: "title".to_string(),
            description: String::new(),
            stored_status: Some(TaskStatus::Done),
            auto_close: None,
        };
        let baseline = baseline_with_run(
            "run-1",
            "implement-phase",
            2_520,
            Ok(Some("basic".to_string())),
            Ok(ClaimedTaskResult::Task(
                Box::new(task),
                ClosePolicyResolution::NoneConfigured,
            )),
        );
        let state = accepted(&baseline);
        let block = sessions_run_block(Some(&state));
        let joined = |row: &KeyValueRow| -> String {
            row.value
                .iter()
                .map(|segment| segment.text.clone())
                .collect()
        };
        assert_eq!(joined(&block.rows[0]), "implement-phase \u{b7} basic");
        assert_eq!(
            joined(&block.rows[1]),
            format!(
                "run-1 \u{b7} {}",
                compact_elapsed_text(Duration::from_secs(2_520))
            )
        );
        assert_eq!(joined(&block.rows[2]), "0243.4 \u{b7} done");
    }

    #[test]
    fn task_failure_wording_uses_no_prohibited_glyph() {
        for outcome in [
            ClaimedTaskResult::Missing,
            ClaimedTaskResult::Ambiguous(vec!["a".to_string()]),
        ] {
            let baseline = baseline_with(Ok(None), Ok(outcome));
            let row = task_row(&baseline);
            assert!(!row.value[0].text.contains('\u{2014}'), "no em dash");
        }
        let failed = baseline_with(Ok(None), Err("center down".to_string()));
        let row = task_row(&failed);
        assert!(!row.value[0].text.contains('\u{2014}'), "no em dash");
    }

    #[test]
    fn trait_row_variant_served_none_and_err_render_without_a_dangling_dot() {
        let served = baseline_with(
            Ok(Some("basic".to_string())),
            Ok(ClaimedTaskResult::Unclaimed),
        );
        let row = trait_row(&served.row, &served);
        assert_eq!(row.value.len(), 3);
        assert_eq!(row.value[2].text, "basic");

        let none = baseline_with(Ok(None), Ok(ClaimedTaskResult::Unclaimed));
        let row = trait_row(&none.row, &none);
        assert_eq!(row.value.len(), 1);

        let err = baseline_with(
            Err("digest mismatch".to_string()),
            Ok(ClaimedTaskResult::Unclaimed),
        );
        let row = trait_row(&err.row, &err);
        assert_eq!(row.value.len(), 3);
        assert_eq!(row.value[2].role, Some(StateRole::Danger));
    }

    #[test]
    fn run_row_elapsed_matches_the_shared_compact_formatter() {
        let baseline = baseline_with_run(
            "run-1",
            "implement-phase",
            2_520,
            Ok(None),
            Ok(ClaimedTaskResult::Unclaimed),
        );
        let composed = run_row_row(&baseline.row);
        assert_eq!(
            composed.value[2].text,
            compact_elapsed_text(Duration::from_secs(2_520))
        );
    }

    #[test]
    fn all_task_boundaries_render_the_documented_shape() {
        let unclaimed = baseline_with(Ok(None), Ok(ClaimedTaskResult::Unclaimed));
        assert_eq!(
            task_row(&unclaimed).value,
            vec![ValueSegment::neutral("no task claimed")]
        );

        let no_status = baseline_with(
            Ok(None),
            Ok(ClaimedTaskResult::Task(
                Box::new(ClaimedTask {
                    key: "0265.10".to_string(),
                    title: "t".to_string(),
                    description: String::new(),
                    stored_status: None,
                    auto_close: None,
                }),
                ClosePolicyResolution::NoneConfigured,
            )),
        );
        assert_eq!(
            task_row(&no_status).value,
            vec![ValueSegment::neutral("0265.10")]
        );

        for outcome in [
            ClaimedTaskResult::Missing,
            ClaimedTaskResult::Ambiguous(vec!["a".to_string()]),
        ] {
            let baseline = baseline_with(Ok(None), Ok(outcome));
            let row = task_row(&baseline);
            assert_eq!(row.value.len(), 1);
            assert_eq!(row.value[0].role, Some(StateRole::Danger));
        }

        let failed = baseline_with(Ok(None), Err("center down".to_string()));
        let row = task_row(&failed);
        assert_eq!(row.value[0].role, Some(StateRole::Danger));
    }

    #[test]
    fn footer_omits_started_when_epoch_is_none_with_no_dangling_separator() {
        let baseline = baseline_with(Ok(None), Ok(ClaimedTaskResult::Unclaimed));
        let state = accepted(&baseline);
        let footer = sessions_footer(Some(&state));
        assert_eq!(footer, "run-1");
        assert!(!footer.contains("started"));
    }

    #[test]
    fn footer_renders_minute_precision_when_started_at_epoch_is_some() {
        let mut session = fixture_session("run-1", "implement-phase", 2_520);
        session["provenance"]["started-at-epoch"] = serde_json::json!(3_723);
        let mut row = fixture_row("run-1", "implement-phase", 2_520);
        row.started_at_epoch = Some(3_723);
        let baseline = DetailBaseline {
            session: serde_json::from_value(session).expect("fixture session"),
            activity_overlay: crate::detail_tree::ActivityOverlay::default(),
            skipped_activity_lines: 0,
            variant: Ok(None),
            claimed_task: Ok(ClaimedTaskResult::Unclaimed),
            progress: Ok(ctx_traits_core::procedure::run::RunProgress::NoCountedFrames),
            row,
        };
        let state = accepted(&baseline);
        let footer = sessions_footer(Some(&state));
        assert!(footer.starts_with("run-1 \u{b7} started "));
    }

    #[test]
    fn stale_and_refreshing_postures_render_a_visible_marker_never_identical_to_current() {
        let baseline = baseline_with(Ok(None), Ok(ClaimedTaskResult::Unclaimed));

        let current = PreviewState::Accepted {
            baseline: &baseline,
            stale: None,
            refreshing: false,
            live: baseline.row.live,
            session_title: None,
        };
        let refreshing = PreviewState::Accepted {
            baseline: &baseline,
            stale: None,
            refreshing: true,
            live: baseline.row.live,
            session_title: None,
        };
        let stale = PreviewState::Accepted {
            baseline: &baseline,
            stale: Some("subscription closed"),
            refreshing: true,
            live: baseline.row.live,
            session_title: None,
        };

        let current_block = sessions_run_block(Some(&current));
        let refreshing_block = sessions_run_block(Some(&refreshing));
        let stale_block = sessions_run_block(Some(&stale));

        assert_eq!(current_block.heading, "run");
        assert_ne!(refreshing_block.heading, current_block.heading);
        assert!(refreshing_block.heading.contains("refreshing"));
        assert_ne!(stale_block.heading, current_block.heading);
        assert!(stale_block.heading.contains("stale"));

        let current_footer = sessions_footer(Some(&current));
        let refreshing_footer = sessions_footer(Some(&refreshing));
        let stale_footer = sessions_footer(Some(&stale));
        assert_ne!(refreshing_footer, current_footer);
        assert!(refreshing_footer.contains("refreshing"));
        assert_ne!(stale_footer, current_footer);
        assert!(stale_footer.contains("stale"));
    }

    #[test]
    fn unreadable_state_produces_no_zero_seconds_no_empty_identity_no_task_and_a_nonempty_footer() {
        let state = PreviewState::Failed("bad json");
        let block = sessions_run_block(Some(&state));
        for row in &block.rows {
            assert_eq!(row.value.len(), 1);
            assert!(row.value[0].text.contains("bad json"));
            assert_eq!(row.value[0].role, Some(StateRole::Danger));
        }
        let footer = sessions_footer(Some(&state));
        assert!(
            !footer.is_empty(),
            "the unreadable footer must not be blank"
        );
        assert!(!footer.contains("started"), "no fabricated provenance");
    }

    /// A projected row already flagged `RowState::Unreadable` renders
    /// unreadable even though the ledger read succeeded and produced an
    /// `Accepted` baseline — no `0s`, no empty identity, no invented task
    /// (goal 8).
    #[test]
    fn a_committed_row_flagged_unreadable_renders_unreadable_even_when_accepted() {
        let mut baseline = baseline_with(Ok(None), Ok(ClaimedTaskResult::Unclaimed));
        baseline.row.state = RowState::Unreadable;
        baseline.row.detail_text = "trailing comma".to_string();
        let state = accepted(&baseline);

        let block = sessions_run_block(Some(&state));
        for row in &block.rows {
            assert_eq!(row.value.len(), 1);
            assert!(row.value[0].text.contains("trailing comma"));
            assert_eq!(row.value[0].role, Some(StateRole::Danger));
            assert!(!row.value[0].text.contains("0s"));
        }
        let footer = sessions_footer(Some(&state));
        assert!(footer.contains("trailing comma"));
        assert!(!footer.contains("started"), "no fabricated provenance");
    }
}

#[cfg(test)]
mod now_and_verdict_tests {
    use super::*;
    use ctx_traits_core::digest::canonical_digest;
    use ctx_traits_core::procedure::runtime::{AcceptanceStatus, SlotRevision, Value, ValueSource};
    use ctx_traits_core::reference::{Kind, Reference};
    use ctx_traits_core::task::provider::ClaimedTask;
    use ctx_traits_io::center::{ClaimedTaskResult, ClosePolicyResolution};

    fn base_session(run_id: &str, trait_id: &str, elapsed_seconds: u64) -> serde_json::Value {
        serde_json::json!({
            "schema-version": "0.1.0",
            "session-id": "session",
            "run-id": run_id,
            "trait-id": trait_id,
            "current-run-index": 0,
            "status": "completed",
            "provenance": {
                "started-by": {"surface": "test", "caller": "preview-fixture"},
                "state-source": "test",
            },
            "ledger": {
                "run-id": run_id,
                "trait-id": trait_id,
                "current-run-index": 0,
                "final-state": "completed",
                "elapsed-seconds": elapsed_seconds,
            },
            "state-digest": "sha256:fixture",
        })
    }

    fn slot_revision(slot_id: &str) -> SlotRevision {
        SlotRevision {
            slot_ref: Reference::local(Kind::Slot, slot_id).expect("valid slot ref"),
            value_digest: ctx_traits_core::digest::Digest::source(slot_id),
            acceptance_order: 0,
            operation: None,
            submitted_payload: None,
            prior_value_digest: None,
            prior_value: None,
            source: None,
            command_execution: None,
            runtime_binding: false,
            projection: None,
            position_path: Vec::new(),
            loop_id: None,
            iteration_index: None,
            for_each_id: None,
            item_index: None,
        }
    }

    fn accepted_value(slot_id: &str, value: serde_json::Value) -> Value {
        Value {
            ref_text: format!("slot:{slot_id}"),
            value_digest: canonical_digest(&value).expect("digest"),
            value,
            schema_ref: None,
            source: ValueSource::HostInput,
            producer_evidence: None,
            command_execution: None,
            producer_agent: None,
            producer_harness: None,
            producer_check_verdict: false,
            acceptance: AcceptanceStatus::Accepted,
            position_path: Vec::new(),
            acceptance_order: None,
            schema_validation: Vec::new(),
        }
    }

    fn baseline(row: RunRow, session_json: serde_json::Value) -> DetailBaseline {
        DetailBaseline {
            session: serde_json::from_value(session_json).expect("fixture session"),
            activity_overlay: crate::detail_tree::ActivityOverlay::default(),
            skipped_activity_lines: 0,
            variant: Ok(None),
            claimed_task: Ok(ClaimedTaskResult::Unclaimed),
            progress: Ok(ctx_traits_core::procedure::run::RunProgress::NoCountedFrames),
            row,
        }
    }

    fn live_row() -> RunRow {
        RunRow {
            ledger_path: "/repo/run-1.json".to_string(),
            session_id: "session".to_string(),
            run_id: "run-1".to_string(),
            repo_key: "repo".to_string(),
            repo_path: "/repo".to_string(),
            repo_label: "repo".to_string(),
            title: "run-1".to_string(),
            session_title: None,
            trait_id: "implement-phase".to_string(),
            state: RowState::Live,
            state_text: "live".to_string(),
            detail_text: String::new(),
            elapsed_text: String::new(),
            tokens_text: "-".to_string(),
            live: true,
            modified_epoch_secs: 0,
            verdict_rounds: None,
            elapsed_seconds: 10,
            started_at_epoch: None,
        }
    }

    fn accepted_state(baseline: &DetailBaseline) -> PreviewState<'_> {
        PreviewState::Accepted {
            baseline,
            stale: None,
            refreshing: false,
            live: baseline.row.live,
            session_title: None,
        }
    }

    #[test]
    fn now_item_present_only_for_a_live_readable_selection() {
        let baseline = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        let state = accepted_state(&baseline);
        assert!(sessions_now_item(Some(&state)).is_some());

        let mut not_live = baseline.clone();
        not_live.row.live = false;
        let state = accepted_state(&not_live);
        assert!(sessions_now_item(Some(&state)).is_none());

        let mut unreadable = baseline.clone();
        unreadable.row.state = RowState::Unreadable;
        let state = accepted_state(&unreadable);
        assert!(sessions_now_item(Some(&state)).is_none());

        assert!(sessions_now_item(Some(&PreviewState::Failed("bad"))).is_none());
        assert!(sessions_now_item(Some(&PreviewState::Loading)).is_none());
        assert!(sessions_now_item(None).is_none());
    }

    #[test]
    fn stale_or_refreshing_accepted_preview_yields_neither_state_block() {
        let baseline = baseline(live_row(), base_session("run-1", "implement-phase", 10));

        let stale = PreviewState::Accepted {
            baseline: &baseline,
            stale: Some("subscription closed"),
            refreshing: false,
            live: baseline.row.live,
            session_title: None,
        };
        assert!(sessions_now_item(Some(&stale)).is_none());
        assert!(sessions_verdict_block(Some(&stale)).is_none());

        let refreshing = PreviewState::Accepted {
            baseline: &baseline,
            stale: None,
            refreshing: true,
            live: baseline.row.live,
            session_title: None,
        };
        assert!(sessions_now_item(Some(&refreshing)).is_none());
        assert!(sessions_verdict_block(Some(&refreshing)).is_none());
    }

    #[test]
    fn a_live_only_transition_drops_the_now_item_without_touching_the_frozen_baseline() {
        let baseline = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        assert!(baseline.row.live, "fixture starts live");

        // A fingerprint-identical `RowChanged` moves the selection's current
        // liveness without replacing the accepted baseline — `baseline.row.live`
        // stays `true` while the served `live` flag has already flipped.
        let live_now_finished = PreviewState::Accepted {
            baseline: &baseline,
            stale: None,
            refreshing: false,
            live: false,
            session_title: None,
        };
        assert!(sessions_now_item(Some(&live_now_finished)).is_none());
    }

    #[test]
    fn now_item_title_falls_back_to_the_absence_literal_when_absent() {
        let baseline = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        let state = accepted_state(&baseline);
        let item = sessions_now_item(Some(&state)).expect("now item");
        assert_eq!(item.title, "no current frame");
        assert!(item.title_muted);
        assert!(!item.title.contains(" \u{b7} "));
    }

    #[test]
    fn now_item_title_falls_back_to_the_absence_literal_when_blank() {
        let mut session_json = base_session("run-1", "implement-phase", 10);
        session_json["current-sequence-title"] = serde_json::json!("   ");
        let baseline = baseline(live_row(), session_json);
        let state = accepted_state(&baseline);
        let item = sessions_now_item(Some(&state)).expect("now item");
        assert_eq!(item.title, "no current frame");
        assert!(item.title_muted);
        assert!(!item.title.contains(" \u{b7} "));
    }

    #[test]
    fn now_item_state_word_is_accent_for_a_live_row() {
        let baseline = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        let state = accepted_state(&baseline);
        let item = sessions_now_item(Some(&state)).expect("now item");
        assert_eq!(item.state_role, StateRole::Accent);
        assert_eq!(item.state_word, "running");
    }

    #[test]
    fn now_item_state_word_is_accent_when_live_moves_true_without_a_baseline_replace() {
        // A fingerprint-identical liveness-only delta updates `PreviewState`'s
        // current `live` without replacing `baseline.row.state`, which can
        // still read a stale non-live row state from before the transition.
        let mut row = live_row();
        row.state = RowState::Paused;
        row.state_text = "paused".to_string();
        row.live = false;
        let baseline = baseline(row, base_session("run-1", "implement-phase", 10));
        let state = PreviewState::Accepted {
            baseline: &baseline,
            stale: None,
            refreshing: false,
            live: true,
            session_title: None,
        };
        let item = sessions_now_item(Some(&state)).expect("now item");
        assert_eq!(item.state_role, StateRole::Accent);
        assert_eq!(item.state_word, "running");
    }

    #[test]
    fn now_item_title_with_no_loop_round_position_is_the_served_title_alone() {
        let mut session_json = base_session("run-1", "implement-phase", 10);
        session_json["current-sequence-title"] = serde_json::json!("review the plan");
        let baseline = baseline(live_row(), session_json);
        let state = accepted_state(&baseline);
        let item = sessions_now_item(Some(&state)).expect("now item");
        assert_eq!(item.title, "review the plan");
        assert!(!item.title_muted);
        assert!(!item.title.contains(" \u{b7} "));
    }

    #[test]
    fn now_item_title_appends_the_loop_round_when_the_position_is_inside_a_loop() {
        let mut session_json = base_session("run-1", "implement-phase", 10);
        session_json["current-sequence-title"] = serde_json::json!("review the plan");
        session_json["active-path"] = serde_json::json!([
            {"kind": "procedure", "id": "root", "index": 0},
            {"kind": "loop", "id": "the-loop", "index": 0, "iteration": 1},
            {"kind": "item", "id": "current", "index": 0, "iteration": 1},
        ]);
        let baseline = baseline(live_row(), session_json);
        let state = accepted_state(&baseline);
        let item = sessions_now_item(Some(&state)).expect("now item");
        assert_eq!(item.title, "review the plan \u{b7} 2");
    }

    #[test]
    fn verdict_block_is_none_without_recognised_evidence() {
        let baseline = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        let state = accepted_state(&baseline);
        assert!(sessions_verdict_block(Some(&state)).is_none());

        assert!(sessions_verdict_block(Some(&PreviewState::Failed("bad"))).is_none());
        assert!(sessions_verdict_block(Some(&PreviewState::Loading)).is_none());
        assert!(sessions_verdict_block(None).is_none());
    }

    #[test]
    fn verdict_block_heading_and_status_row_use_the_dot_never_an_em_dash() {
        let mut baseline_value: DetailBaseline =
            baseline(live_row(), base_session("run-1", "implement-phase", 10));
        baseline_value.session.slot_revisions = vec![slot_revision("review-verdict")];
        baseline_value.session.accepted_slot_values = vec![accepted_value(
            "review-verdict",
            serde_json::json!({"status": "revise", "blockers": [{"id": "b1"}, {"id": "b2"}]}),
        )];
        let state = accepted_state(&baseline_value);
        let block = sessions_verdict_block(Some(&state)).expect("verdict block");
        assert_eq!(block.heading, "verdict \u{b7} round 1");
        assert!(!block.heading.contains('\u{2014}'));
        assert_eq!(block.status_row.value[0].role, Some(StateRole::Neutral));
        let joined: String = block
            .status_row
            .value
            .iter()
            .map(|segment| segment.text.clone())
            .collect();
        assert_eq!(joined, "revise \u{b7} 2 findings");
        assert!(!joined.contains('\u{2014}'));
        assert_eq!(
            block.blocker_lines,
            vec!["b1".to_string(), "b2".to_string()]
        );
    }

    /// review-verdict-1 blocker `required-rendered-preview-evidence-missing`:
    /// a second recognised verdict revision must move the rendered heading
    /// to `round 2`, not silently stay at `round 1`.
    #[test]
    fn verdict_block_heading_advances_with_a_second_revision() {
        let mut baseline_value: DetailBaseline =
            baseline(live_row(), base_session("run-1", "implement-phase", 10));
        baseline_value.session.slot_revisions = vec![
            slot_revision("review-verdict"),
            slot_revision("review-verdict"),
        ];
        baseline_value.session.accepted_slot_values = vec![accepted_value(
            "review-verdict",
            serde_json::json!({"status": "approved"}),
        )];
        let state = accepted_state(&baseline_value);
        let block = sessions_verdict_block(Some(&state)).expect("verdict block");
        assert_eq!(block.heading, "verdict \u{b7} round 2");
        assert_ne!(block.heading, "verdict \u{b7} round 1");
    }

    #[test]
    fn verdict_block_approved_has_no_count_segment() {
        let mut baseline_value = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        baseline_value.session.slot_revisions = vec![slot_revision("review-verdict")];
        baseline_value.session.accepted_slot_values = vec![accepted_value(
            "review-verdict",
            serde_json::json!({"status": "approved"}),
        )];
        let state = accepted_state(&baseline_value);
        let block = sessions_verdict_block(Some(&state)).expect("verdict block");
        assert_eq!(block.status_row.value.len(), 1);
        assert_eq!(block.status_row.value[0].text, "approved");
        assert_eq!(block.status_row.value[0].role, Some(StateRole::Ok));
        assert!(block.blocker_lines.is_empty());
    }

    #[test]
    fn verdict_block_singular_finding_count_reads_finding_not_findings() {
        let mut baseline_value = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        baseline_value.session.slot_revisions = vec![slot_revision("review-verdict")];
        baseline_value.session.accepted_slot_values = vec![accepted_value(
            "review-verdict",
            serde_json::json!({"status": "revise", "blockers": [{"id": "b1"}]}),
        )];
        let state = accepted_state(&baseline_value);
        let block = sessions_verdict_block(Some(&state)).expect("verdict block");
        let joined: String = block
            .status_row
            .value
            .iter()
            .map(|segment| segment.text.clone())
            .collect();
        assert_eq!(joined, "revise \u{b7} 1 finding");
    }

    #[test]
    fn verdict_block_partial_round_with_no_revise_member_is_pending_with_no_count_segment() {
        let mut baseline_value = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        baseline_value.session.slot_revisions = vec![
            SlotRevision {
                acceptance_order: 1,
                ..slot_revision("review-verdict-1")
            },
            SlotRevision {
                acceptance_order: 0,
                ..slot_revision("review-verdict-2")
            },
        ];
        baseline_value.session.accepted_slot_values = vec![
            accepted_value(
                "review-verdict-1",
                serde_json::json!({"status": "approved"}),
            ),
            accepted_value(
                "review-verdict-2",
                serde_json::json!({"status": "approved"}),
            ),
        ];
        // Only slot 1 has a second (round-2) revision: the round is 2, but
        // slot 2's accepted value is still its round-1 value, so it lags.
        baseline_value.session.slot_revisions.push(SlotRevision {
            acceptance_order: 2,
            ..slot_revision("review-verdict-1")
        });
        let state = accepted_state(&baseline_value);
        let block = sessions_verdict_block(Some(&state)).expect("verdict block");
        assert_eq!(block.status_row.value.len(), 1);
        assert_eq!(block.status_row.value[0].text, "pending");
        assert_eq!(block.status_row.value[0].role, Some(StateRole::Neutral));
        assert!(block.blocker_lines.is_empty());
    }

    #[test]
    fn verdict_tone_maps_exhaustively_to_state_role() {
        assert_eq!(verdict_tone_to_state_role(VerdictTone::Ok), StateRole::Ok);
        assert_eq!(
            verdict_tone_to_state_role(VerdictTone::Warn),
            StateRole::Warn
        );
        assert_eq!(
            verdict_tone_to_state_role(VerdictTone::Neutral),
            StateRole::Neutral
        );
        assert_eq!(
            verdict_tone_to_state_role(VerdictTone::Danger),
            StateRole::Danger
        );
    }

    #[test]
    fn verdict_block_unreadable_has_no_segment_and_no_blocker_lines() {
        let mut baseline_value = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        baseline_value.session.slot_revisions = vec![slot_revision("review-verdict")];
        baseline_value.session.accepted_slot_values = vec![accepted_value(
            "review-verdict",
            serde_json::json!({"status": "scratch-status-outside-pair", "blockers": [{"id": "b1"}]}),
        )];
        let state = accepted_state(&baseline_value);
        let block = sessions_verdict_block(Some(&state)).expect("verdict block");
        assert_eq!(block.status_row.value.len(), 1);
        assert_eq!(block.status_row.value[0].role, Some(StateRole::Danger));
        assert!(block.blocker_lines.is_empty());
    }

    #[test]
    fn verdict_block_blocker_lines_render_id_alone_or_id_dot_what() {
        let mut baseline_value = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        baseline_value.session.slot_revisions = vec![slot_revision("review-verdict")];
        baseline_value.session.accepted_slot_values = vec![accepted_value(
            "review-verdict",
            serde_json::json!({
                "status": "revise",
                "blockers": [{"id": "b1"}, {"id": "b2", "what": "the defect"}],
            }),
        )];
        let state = accepted_state(&baseline_value);
        let block = sessions_verdict_block(Some(&state)).expect("verdict block");
        assert_eq!(
            block.blocker_lines,
            vec!["b1".to_string(), "b2 \u{b7} the defect".to_string()]
        );
    }

    #[test]
    fn verdict_block_returns_none_for_an_unreadable_row_even_with_evidence() {
        let mut row = live_row();
        row.state = RowState::Unreadable;
        let mut baseline_value = baseline(row, base_session("run-1", "implement-phase", 10));
        baseline_value.session.slot_revisions = vec![slot_revision("review-verdict")];
        baseline_value.session.accepted_slot_values = vec![accepted_value(
            "review-verdict",
            serde_json::json!({"status": "approved"}),
        )];
        let state = accepted_state(&baseline_value);
        assert!(sessions_verdict_block(Some(&state)).is_none());
    }

    #[test]
    fn slots_block_none_for_non_accepted_and_no_evidence_states() {
        let baseline_value = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        let state = accepted_state(&baseline_value);
        assert!(
            sessions_slots_block(Some(&state)).is_none(),
            "no slot evidence renders no block"
        );

        let mut not_live = baseline_value.clone();
        not_live.row.live = false;
        not_live.session.accepted_slot_values =
            vec![accepted_value("draft", serde_json::json!({}))];
        let state = accepted_state(&not_live);
        assert!(sessions_slots_block(Some(&state)).is_some());

        let mut unreadable = baseline_value.clone();
        unreadable.row.state = RowState::Unreadable;
        unreadable.session.accepted_slot_values =
            vec![accepted_value("draft", serde_json::json!({}))];
        let state = accepted_state(&unreadable);
        assert!(sessions_slots_block(Some(&state)).is_none());

        assert!(sessions_slots_block(Some(&PreviewState::Failed("bad"))).is_none());
        assert!(sessions_slots_block(Some(&PreviewState::Loading)).is_none());
        assert!(sessions_slots_block(None).is_none());
    }

    #[test]
    fn stale_or_refreshing_accepted_preview_yields_no_slots_block() {
        let mut baseline_value = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        baseline_value.session.accepted_slot_values =
            vec![accepted_value("draft", serde_json::json!({}))];

        let stale = PreviewState::Accepted {
            baseline: &baseline_value,
            stale: Some("subscription closed"),
            refreshing: false,
            live: baseline_value.row.live,
            session_title: None,
        };
        assert!(sessions_slots_block(Some(&stale)).is_none());

        let refreshing = PreviewState::Accepted {
            baseline: &baseline_value,
            stale: None,
            refreshing: true,
            live: baseline_value.row.live,
            session_title: None,
        };
        assert!(sessions_slots_block(Some(&refreshing)).is_none());
    }

    #[test]
    fn slots_block_heading_is_exactly_slots() {
        let mut baseline_value = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        baseline_value.session.accepted_slot_values =
            vec![accepted_value("draft", serde_json::json!({}))];
        let state = accepted_state(&baseline_value);
        let block = sessions_slots_block(Some(&state)).expect("slots block");
        assert_eq!(block.heading, "slots");
    }

    #[test]
    fn slots_block_renders_filled_and_write_counted_rows_with_the_dot_separator() {
        let mut baseline_value = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        baseline_value.session.accepted_slot_values = vec![
            accepted_value("draft", serde_json::json!({})),
            accepted_value("review-verdict-1", serde_json::json!({})),
        ];
        let matching_revision = SlotRevision {
            value_digest: canonical_digest(&serde_json::json!({})).expect("digest"),
            ..slot_revision("review-verdict-1")
        };
        baseline_value.session.slot_revisions = vec![matching_revision.clone(), matching_revision];
        let state = accepted_state(&baseline_value);
        let block = sessions_slots_block(Some(&state)).expect("slots block");

        let draft_row = block
            .rows
            .iter()
            .find(|row| row.key == "draft")
            .expect("draft row");
        let joined: String = draft_row.value.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "filled");
        assert!(draft_row.value.iter().all(|segment| segment.role.is_none()));

        let verdict_row = block
            .rows
            .iter()
            .find(|row| row.key == "review-verdict-1")
            .expect("verdict row");
        let joined: String = verdict_row.value.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "filled \u{b7} r2");
        assert_eq!(joined.matches('\u{b7}').count(), 1);
        assert!(
            verdict_row
                .value
                .iter()
                .all(|segment| segment.role.is_none())
        );
    }

    #[test]
    fn slots_block_renders_unreadable_in_danger_tone() {
        let broken = SlotRevision {
            value_digest: ctx_traits_core::digest::Digest::source("stale"),
            ..slot_revision("broken")
        };
        let mut baseline_value = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        baseline_value.session.accepted_slot_values =
            vec![accepted_value("broken", serde_json::json!({}))];
        baseline_value.session.slot_revisions = vec![broken];
        let state = accepted_state(&baseline_value);
        let block = sessions_slots_block(Some(&state)).expect("slots block");

        assert_eq!(block.rows.len(), 1);
        assert_eq!(block.rows[0].value.len(), 1);
        assert_eq!(block.rows[0].value[0].text, "unreadable");
        assert_eq!(block.rows[0].value[0].role, Some(StateRole::Danger));
    }

    #[test]
    fn slots_block_row_order_matches_the_ledger_projection() {
        let mut baseline_value = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        baseline_value.session.accepted_slot_values = vec![
            accepted_value("zulu", serde_json::json!({})),
            accepted_value("alpha", serde_json::json!({})),
            accepted_value("mike", serde_json::json!({})),
        ];
        let state = accepted_state(&baseline_value);
        let block = sessions_slots_block(Some(&state)).expect("slots block");
        let keys: Vec<&str> = block.rows.iter().map(|row| row.key.as_str()).collect();
        assert_eq!(keys, vec!["zulu", "alpha", "mike"]);
    }

    // --- 0265.15: sessions_landing_block -----------------------------------

    #[test]
    fn landing_block_is_none_unless_accepted_settled_and_readable() {
        assert!(sessions_landing_block(None).is_none());
        assert!(sessions_landing_block(Some(&PreviewState::Loading)).is_none());
        assert!(sessions_landing_block(Some(&PreviewState::Failed("bad"))).is_none());

        let baseline_value = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        let stale = PreviewState::Accepted {
            baseline: &baseline_value,
            stale: Some("resync"),
            refreshing: false,
            live: true,
            session_title: None,
        };
        assert!(sessions_landing_block(Some(&stale)).is_none());
        let refreshing = PreviewState::Accepted {
            baseline: &baseline_value,
            stale: None,
            refreshing: true,
            live: true,
            session_title: None,
        };
        assert!(sessions_landing_block(Some(&refreshing)).is_none());

        let mut unreadable_row = live_row();
        unreadable_row.state = RowState::Unreadable;
        let unreadable_baseline =
            baseline(unreadable_row, base_session("run-1", "implement-phase", 10));
        let unreadable = accepted_state(&unreadable_baseline);
        assert!(sessions_landing_block(Some(&unreadable)).is_none());
    }

    /// Goal 1: three lines always present, with the absence forms for a run
    /// with none of the underlying evidence — never fewer than three, never
    /// omitted.
    #[test]
    fn landing_block_renders_the_absence_trio_for_a_run_with_no_evidence() {
        let baseline_value = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        let state = accepted_state(&baseline_value);
        let block = sessions_landing_block(Some(&state)).expect("landing block always renders");
        assert_eq!(block.heading, "landing");
        assert_eq!(block.lines.len(), 3);
        assert_eq!(
            block.lines[0].text,
            "\u{2192} No worktree recorded for this run"
        );
        assert_eq!(
            block.lines[1].text,
            "\u{2192} No automatic merge intent recorded"
        );
        assert_eq!(block.lines[2].text, "\u{2192} No task claimed by this run");
        assert!(block.lines.iter().all(|line| line.role.is_none()));
    }

    #[test]
    fn landing_block_renders_worktree_and_terminal_merge_evidence() {
        use ctx_traits_core::procedure::session::{
            MergeFrame, MergeStage, MergeStatus, WorktreeProvenance,
        };

        let mut baseline_value = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        baseline_value.session.provenance.worktree = Some(WorktreeProvenance {
            id: "wt-ab12ef".to_string(),
            branch: "ctx/run/wt-ab12ef".to_string(),
            seed_snapshots: Vec::new(),
            path: None,
        });
        baseline_value.session.provenance.merge_frames = vec![MergeFrame {
            stage: MergeStage::Landing,
            status: MergeStatus::Merged,
            reason: None,
            evidence: vec!["landed=deadbeef".to_string()],
            park_reason: None,
            deep_decisions: Vec::new(),
        }];
        let state = accepted_state(&baseline_value);
        let block = sessions_landing_block(Some(&state)).expect("landing block");
        assert_eq!(block.lines[0].text, "\u{2192} Runs in worktree wt-ab12ef");
        assert_eq!(block.lines[1].text, "\u{2192} Merged at deadbeef");
        assert_eq!(block.lines[1].role, Some(StateRole::Ok));
    }

    /// Goal 4/7: the close line comes entirely from the served claimed-task
    /// answer — an effective policy, a resolved absence, and the unresolved
    /// failure form all render distinctly.
    #[test]
    fn landing_block_close_line_covers_effective_none_configured_and_unresolved() {
        let claimed = ClaimedTask {
            key: "0243.4".to_string(),
            title: "t".to_string(),
            description: String::new(),
            stored_status: None,
            auto_close: None,
        };

        let mut baseline_value = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        baseline_value.claimed_task = Ok(ClaimedTaskResult::Task(
            Box::new(claimed.clone()),
            ClosePolicyResolution::Effective(ctx_traits_core::task::AutoClosePolicy::Checked),
        ));
        let state = accepted_state(&baseline_value);
        let block = sessions_landing_block(Some(&state)).expect("landing block");
        assert_eq!(
            block.lines[2].text,
            "\u{2192} Task 0243.4 closes when its declared checks pass"
        );
        assert_eq!(block.lines[2].role, None);

        let mut none_configured =
            baseline(live_row(), base_session("run-1", "implement-phase", 10));
        none_configured.claimed_task = Ok(ClaimedTaskResult::Task(
            Box::new(claimed.clone()),
            ClosePolicyResolution::NoneConfigured,
        ));
        let state = accepted_state(&none_configured);
        let block = sessions_landing_block(Some(&state)).expect("landing block");
        assert_eq!(
            block.lines[2].text,
            "\u{2192} Task 0243.4 has no auto-close policy"
        );

        let mut unresolved = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        unresolved.claimed_task = Ok(ClaimedTaskResult::Task(
            Box::new(claimed),
            ClosePolicyResolution::Unresolved("malformed config".to_string()),
        ));
        let state = accepted_state(&unresolved);
        let block = sessions_landing_block(Some(&state)).expect("landing block");
        assert_eq!(
            block.lines[2].text,
            "\u{2192} Task 0243.4 close policy unresolved"
        );
        assert_eq!(block.lines[2].role, Some(StateRole::Danger));
        assert!(
            !block.lines[2].text.contains("malformed config"),
            "the unresolved reason text is never rendered"
        );

        let mut missing = baseline(live_row(), base_session("run-1", "implement-phase", 10));
        missing.claimed_task = Ok(ClaimedTaskResult::Missing);
        let state = accepted_state(&missing);
        let block = sessions_landing_block(Some(&state)).expect("landing block");
        assert_eq!(
            block.lines[2].text,
            "\u{2192} Claimed task close policy unresolved"
        );
        assert_eq!(block.lines[2].role, Some(StateRole::Danger));
    }
}
