//! Session/plan → view-model projection: builds a `RunView` from a dry
//! plan plus live session status, and reconstructs the same shape from a
//! ledger + P521 activity sidecar for dashboard preview/attach.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use ctx_traits_core::procedure::activity::{ActivityEvent, ActivityKind};

use super::model::{
    HistoryOutcome, HistoryStep, JourneyRow, PresentationState, RunHeader, RunNarration, RunOutput,
    RunView, StepState,
};
use super::planned::{
    PlannedItemLocation, accepted_refs, active_loop_container_keys, child_location, flatten_step,
    harness_by_role, loop_container_key, parallel_child_location, ref_slug, step_key,
    structural_path_key, structural_path_matches,
};
use super::session_text::{
    harness_summary, input_text, output_port_status, phase_text, stop_reason_summary,
};
use super::{
    CURRENT_STREAM_CAP, EventRow, RunPanelState, StreamRow, StreamRowKind, journey_lines,
    progress_lines, story_history_lines,
};
use crate::app::tui;
use crate::app::{merge_story, story};

pub(super) fn run_view(
    trait_ref: &ctx_traits_core::Trait,
    plan: &ctx_traits_core::procedure::run::Plan,
    session: &ctx_traits_core::procedure::session::Session,
    narration: Option<RunNarration>,
    presentation: PresentationState<'_>,
) -> RunView {
    let harness_by_role = harness_by_role(session);
    let accepted = accepted_refs(session);
    let mut steps = plan
        .sequence_items
        .iter()
        .flat_map(|item| {
            flatten_step(
                item,
                &PlannedItemLocation::root(item),
                session,
                &harness_by_role,
                &accepted,
                false,
                presentation.live_drive,
            )
        })
        .collect::<Vec<_>>();
    let active_loop_keys = active_loop_container_keys(session);
    for step in &mut steps {
        if let Some(loop_key) = step.loop_key.clone() {
            let committed = presentation
                .loop_elapsed
                .get(&loop_key)
                .copied()
                .unwrap_or_default();
            let live = presentation
                .active_started
                .as_ref()
                .filter(|_| active_loop_keys.contains(&loop_key))
                .map(|(_, started)| started.elapsed())
                .unwrap_or_default();
            step.elapsed = (presentation.loop_elapsed.contains_key(&loop_key)
                || live > Duration::ZERO)
                .then_some(committed + live);
            step.output_tokens = presentation.loop_output_tokens.get(&loop_key).copied();
            continue;
        }
        let key = step_key(step);
        step.elapsed = presentation
            .finished_durations
            .get(&key)
            .copied()
            .or_else(|| {
                presentation
                    .active_started
                    .as_ref()
                    .filter(|(active_key, _)| active_key == &key)
                    .map(|(_, started)| started.elapsed())
            });
        step.output_tokens = presentation.output_tokens.get(&key).copied();
        step.summary = presentation.step_summaries.get(&key).cloned();
        step.summary_at = presentation.step_summary_at.get(&key).copied();
    }
    let accepted_values = session
        .accepted_slot_values
        .iter()
        .chain(session.accepted_output_port_values.iter())
        .collect::<Vec<_>>();
    let structured_producers = accepted_values
        .iter()
        .filter_map(|value| {
            let port_id =
                crate::app::structured_output::port_id_for_value(trait_ref, &value.ref_text)?;
            let rendered =
                crate::app::structured_output::resolve(trait_ref, &port_id, &value.value)?;
            let revision = session.slot_revisions.iter().find(|revision| {
                revision.slot_ref.as_str() == value.ref_text
                    && revision.value_digest == value.value_digest
            });
            let status = session
                .ledger
                .sequence_statuses
                .iter()
                .filter(|status| {
                    status.status
                        == ctx_traits_core::procedure::runtime::SequenceStatusKind::Accepted
                        && if let Some(revision) = revision {
                            status.position_path == revision.position_path
                        } else {
                            steps.iter().any(|step| {
                                step.run_index == status.run_index
                                    && step.outputs.iter().any(|output| output.slug == port_id)
                            })
                        }
                })
                .max_by_key(|status| status.run_index)?;
            Some((
                status.run_index,
                revision.map_or_else(Vec::new, |revision| revision.position_path.clone()),
                rendered.count,
                port_id,
            ))
        })
        .collect::<Vec<_>>();
    let structured_count = structured_producers
        .iter()
        .map(|(_, _, count, _)| *count)
        .sum();
    for step in &mut steps {
        step.structured_count = structured_producers
            .iter()
            .filter(|(run_index, path, _, _)| {
                *run_index == step.run_index && *path == step.position_path
            })
            .map(|(_, _, count, _)| *count)
            .sum();
    }
    let outputs = session
        .output_ports
        .iter()
        .map(|output| {
            let accepted =
                output.status == ctx_traits_core::procedure::runtime::OutputPortStatus::Accepted;
            RunOutput {
                slug: ref_slug(output.port_ref.as_str()),
                status: output_port_status(&output.status).to_string(),
                accepted,
            }
        })
        .collect::<Vec<_>>();
    let done = steps
        .iter()
        .filter(|step| step.counts_progress && step.state == StepState::Done)
        .count();
    let total = steps.iter().filter(|step| step.counts_progress).count();
    let header = RunHeader {
        session_id: session.session_id.as_str().to_string(),
        run_id: session.run_id.as_str().to_string(),
        input: input_text(session),
        harnesses: harness_summary(&harness_by_role),
        done,
        total,
        phase: phase_text(session),
        completed: session.completion.is_some(),
        landing_not_merged: matches!(
            ctx_traits_core::procedure::session::landing_state(session),
            Some(ctx_traits_core::procedure::session::LandingState::NotMerged)
        )
        .then(|| crate::app::run::not_merged_fact(session))
        .flatten(),
        stopped: session
            .stop_reason
            .as_ref()
            .map(|reason| reason.reason.clone()),
        stop_detail: stop_reason_summary(session),
        state_digest: session.state_digest.to_string(),
        // Distinct actual harness ids across every configured seat of every
        // role (P456) — not distinct joined `"seat1/seat2"` display strings,
        // which would undercount whenever two different two-seat roles
        // happened to share the same joined text, or overcount identical
        // single harnesses shown under different role names.
        harness_count: harness_by_role
            .values()
            .flatten()
            .map(|(_, harness)| harness.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        elapsed: Some(presentation.run_started.elapsed()),
        output_tokens: (!presentation.output_tokens.is_empty())
            .then(|| presentation.output_tokens.values().sum()),
        narrator_tokens: (presentation.narrator_tokens > 0).then_some(presentation.narrator_tokens),
        guide_tokens: (presentation.guide_tokens > 0).then_some(presentation.guide_tokens),
        structured_count,
        structured_label: structured_producers
            .first()
            .map(|(_, _, _, port_id)| port_id.clone()),
        structured_verdict: crate::app::structured_output::producer_verdict(session),
    };
    let history = session
        .ledger
        .sequence_statuses
        .iter()
        .filter(|status| {
            status.status == ctx_traits_core::procedure::runtime::SequenceStatusKind::Accepted
        })
        .map(|status| history_step_from_status(status, Some(plan), Some(session), &presentation))
        .collect();
    RunView {
        header,
        steps,
        history,
        narration,
        outputs,
        // P549: never derived from `trait_ref`/`plan`/`session` — every
        // caller of this pure function overwrites this with the panel's own
        // folded `merge_rows` when a merge span is live.
        merge_rows: Vec::new(),
    }
}

/// Resolves an accepted status to the presentation record written when that
/// exact execution ran. This deliberately does not consult the current plan
/// projection: a root status has no persisted path, and a resumed run may no
/// longer render the branch that produced an earlier accepted status.
pub(super) fn history_step_from_status(
    status: &ctx_traits_core::procedure::runtime::SequenceStatus,
    plan: Option<&ctx_traits_core::procedure::run::Plan>,
    session: Option<&ctx_traits_core::procedure::session::Session>,
    presentation: &PresentationState<'_>,
) -> HistoryStep {
    let key = history_presentation_key(status, presentation);
    let loop_key = history_loop_container_key(status);
    let planned = plan.and_then(|plan| planned_item_for_status(plan, status));
    HistoryStep {
        label: story::format_step_title(&status.title, &status.position_path),
        kind: planned.as_ref().map(|(item, _)| item.kind.clone()),
        outcome: planned.and_then(|(item, _)| {
            session.and_then(|session| history_outcome(item, status, session))
        }),
        elapsed: key
            .as_ref()
            .and_then(|key| presentation.finished_durations.get(key).copied())
            .or_else(|| {
                loop_key
                    .as_ref()
                    .and_then(|key| presentation.loop_elapsed.get(key).copied())
            }),
        output_tokens: key
            .as_ref()
            .and_then(|key| presentation.output_tokens.get(key).copied())
            .or_else(|| {
                loop_key
                    .as_ref()
                    .and_then(|key| presentation.loop_output_tokens.get(key).copied())
            }),
        summary: key
            .as_ref()
            .and_then(|key| presentation.step_summaries.get(key).cloned()),
        summary_at: key
            .as_ref()
            .and_then(|key| presentation.step_summary_at.get(key).copied()),
    }
}

pub(super) fn planned_item_for_status<'a>(
    plan: &'a ctx_traits_core::procedure::run::Plan,
    status: &ctx_traits_core::procedure::runtime::SequenceStatus,
) -> Option<(
    &'a ctx_traits_core::procedure::run::PlannedSequenceItem,
    PlannedItemLocation,
)> {
    fn visit<'a>(
        item: &'a ctx_traits_core::procedure::run::PlannedSequenceItem,
        location: &PlannedItemLocation,
        status: &ctx_traits_core::procedure::runtime::SequenceStatus,
    ) -> Option<(
        &'a ctx_traits_core::procedure::run::PlannedSequenceItem,
        PlannedItemLocation,
    )> {
        let status_path = canonical_status_path(status);
        let matches = structural_path_matches(&status_path, &location.position_path);
        if matches {
            return Some((item, location.clone()));
        }
        for child in &item.children {
            if let Some(found) = visit(child, &child_location(location, item, false, child), status)
            {
                return Some(found);
            }
        }
        for child in &item.otherwise_children {
            if let Some(found) = visit(child, &child_location(location, item, true, child), status)
            {
                return Some(found);
            }
        }
        for branch in &item.parallel_branches {
            for child in &branch.children {
                if let Some(found) = visit(
                    child,
                    &parallel_child_location(location, item, branch.sequence_ref.id(), child),
                    status,
                ) {
                    return Some(found);
                }
            }
        }
        None
    }
    plan.sequence_items
        .iter()
        .find_map(|item| visit(item, &PlannedItemLocation::root(item), status))
}

/// Ledger statuses omit the root segment while revisions retain it. This is
/// the execution identity used for every historical join below.
pub(super) fn canonical_status_path(
    status: &ctx_traits_core::procedure::runtime::SequenceStatus,
) -> Vec<ctx_traits_core::procedure::runtime::PathSegment> {
    if !status.position_path.is_empty() {
        return status.position_path.clone();
    }
    vec![ctx_traits_core::procedure::runtime::PathSegment {
        kind: "procedure".to_string(),
        id: status.item_id.clone(),
        index: status.run_index,
        iteration: None,
        item_index: None,
    }]
}

pub(super) fn history_outcome(
    item: &ctx_traits_core::procedure::run::PlannedSequenceItem,
    status: &ctx_traits_core::procedure::runtime::SequenceStatus,
    session: &ctx_traits_core::procedure::session::Session,
) -> Option<HistoryOutcome> {
    if !matches!(
        item.kind,
        ctx_traits_core::procedure::run::PlannedSequenceKind::Check
            | ctx_traits_core::procedure::run::PlannedSequenceKind::Command
    ) {
        return None;
    }
    let execution_path = canonical_status_path(status);
    let revision = historical_slot_revisions(session)
        .into_iter()
        .rev()
        .find(|revision| {
            item.output_refs
                .iter()
                .any(|output| output.as_str() == revision.slot_ref.as_str())
                && revision.position_path == execution_path
        })?;
    let value = revision
        .submitted_payload
        .as_ref()
        .map(|value| &value.value)
        .or_else(|| {
            session
                .accepted_slot_values
                .iter()
                .chain(session.accepted_output_port_values.iter())
                .find(|value| {
                    value.ref_text == revision.slot_ref.as_str()
                        && value.value_digest == revision.value_digest
                })
                .map(|value| &value.value)
        });
    match item.kind {
        ctx_traits_core::procedure::run::PlannedSequenceKind::Check => {
            let object = value?.as_object()?;
            let ok = object.get("ok")?.as_bool()?;
            let exit_code = object.get("exit-code").and_then(serde_json::Value::as_i64);
            Some(HistoryOutcome::Check { ok, exit_code })
        }
        ctx_traits_core::procedure::run::PlannedSequenceKind::Command => {
            let evidence = revision.command_execution.as_ref().or_else(|| {
                session
                    .accepted_slot_values
                    .iter()
                    .chain(session.accepted_output_port_values.iter())
                    .find(|value| {
                        value.ref_text == revision.slot_ref.as_str()
                            && value.value_digest == revision.value_digest
                    })
                    .and_then(|value| value.command_execution.as_ref())
            })?;
            let succeeded = command_succeeded(item, evidence.exit_code, evidence.timed_out);
            Some(HistoryOutcome::Command {
                succeeded,
                exit_code: evidence.exit_code,
            })
        }
        _ => None,
    }
}

/// Historical status rows must see evidence still isolated behind a parallel
/// barrier. Keep this traversal aligned with the runtime's recorded-revision
/// view, then use acceptance order rather than buffer traversal order.
fn historical_slot_revisions(
    session: &ctx_traits_core::procedure::session::Session,
) -> Vec<&ctx_traits_core::procedure::runtime::SlotRevision> {
    let mut revisions: Vec<_> = session.slot_revisions.iter().collect();
    for frame in &session.ledger.control_stack {
        revisions.extend(frame.parallel_buffer.slot_revisions.iter());
        for branch in &frame.parallel_committed_branches {
            revisions.extend(branch.slot_revisions.iter());
        }
    }
    revisions.sort_by_key(|revision| revision.acceptance_order);
    revisions
}

pub(super) fn command_succeeded(
    item: &ctx_traits_core::procedure::run::PlannedSequenceItem,
    exit_code: Option<i32>,
    timed_out: bool,
) -> bool {
    let success_codes = item
        .command_plan
        .as_ref()
        .map(|plan| plan.success_exit_code.as_slice())
        .filter(|codes| !codes.is_empty())
        .unwrap_or(&[0]);
    !timed_out && exit_code.is_some_and(|code| success_codes.contains(&code))
}

/// Control completion statuses record the root item's ordinary status path,
/// while their aggregate presentation facts use the container-only `loop:`
/// key. Keep this separate from the per-execution lookup so a completed
/// loop/for-each row retains its accumulated facts without making leaf rows
/// depend on the live plan projection.
fn history_loop_container_key(
    status: &ctx_traits_core::procedure::runtime::SequenceStatus,
) -> Option<String> {
    if !matches!(
        status.reason.as_str(),
        "control item completed: loop" | "control item completed: for-each"
    ) {
        return None;
    }
    if status.position_path.is_empty() {
        return Some(format!(
            "loop:procedure:{}:{}",
            status.item_id.as_deref().unwrap_or_default(),
            status.run_index
        ));
    }
    Some(loop_container_key(&status.position_path))
}

fn history_presentation_key(
    status: &ctx_traits_core::procedure::runtime::SequenceStatus,
    presentation: &PresentationState<'_>,
) -> Option<String> {
    let prefix = if status.position_path.len() <= 1 {
        format!(
            "{}:{}:",
            status.run_index,
            status.item_id.as_deref().unwrap_or_default()
        )
    } else {
        format!(
            "{}:{}:",
            status.run_index,
            structural_path_key(&status.position_path)
        )
    };
    presentation
        .finished_durations
        .keys()
        .chain(presentation.output_tokens.keys())
        .chain(presentation.step_summaries.keys())
        .chain(presentation.step_summary_at.keys())
        .find(|key| key.starts_with(&prefix))
        .cloned()
}

/// P469: reconstructs the exact `render_tree_lines` output from a resolved
/// trait/plan plus a session read straight off its ledger, for the SESSIONS
/// dashboard's preview/attach panes — a different process from the driver,
/// so it cannot reuse [`RunPanel`]'s live in-process aggregates.
/// `PresentationState`'s per-step `active_started`/`finished_durations`/
/// `loop_*`/`step_summaries` maps stay empty (a ledger alone carries no
/// per-step timing or narration); the header's own elapsed derives from
/// `session.ledger.elapsed_seconds` instead of a live `Instant`, and its
/// aggregate token line is populated from `session.last_drive_outcome`'s
/// persisted counters under a synthetic key that intentionally matches no
/// real step (so per-step token display correctly stays absent, while the
/// header total — the only thing the ledger can support — still renders).
/// P552: [`render_ledger_run_view`]'s return — the ledger's own pane
/// projection, consumed by both dashboard preview (progress/journey only)
/// and dashboard attach (all four) through the shared [`PaneData`]/
/// [`render_pane_body`] renderer, never a second flat-line reconstruction.
pub(crate) struct LedgerPaneProjection {
    pub(crate) progress: Vec<tui::Line>,
    pub(crate) journey: Vec<JourneyRow>,
    pub(crate) landing: Vec<tui::Line>,
    pub(crate) history: Vec<EventRow>,
    pub(crate) current: Vec<EventRow>,
    /// `true` only when `ledger_path` has an activity sidecar at all (a
    /// legacy session, or one that never dispatched through the CLI's
    /// non-wave path, has none) — the caller's SOLE authority for whether to
    /// present the history/current panes at all. Deliberately independent of
    /// `history`/`current`'s own contents: a current-only sidecar (no
    /// completed step yet) is still available and must not be mistaken for
    /// "no source" just because `history` happens to be empty.
    pub(crate) activity_available: bool,
    /// A human-readable reason to show alongside the panes: `Some` when the
    /// sidecar is absent (`activity_available` is `false`) OR present but the
    /// tolerant reader had to skip unparseable lines (P521) — never fatal,
    /// never fabricated content, just an honest degradation notice.
    pub(crate) activity_degraded: Option<String>,
    /// The resolved trait's own name and the ledger's `started_at_epoch`,
    /// carried alongside the persisted P552 session title so a dashboard
    /// attach can render the exact `<bold title> · <trait name> · Started
    /// at <HH:MM:SS>` row a live run shows, via [`title_row_line`].
    pub(crate) trait_name: String,
    pub(crate) started_at_epoch: Option<u64>,
}

/// P469: reconstructs the exact `progress_lines`/`journey_lines` output from
/// a resolved trait/plan plus a session read straight off its ledger, for
/// the SESSIONS dashboard's preview/attach panes — a different process from
/// the driver, so it cannot reuse [`RunPanel`]'s live in-process aggregates.
/// `PresentationState`'s per-step `active_started`/`finished_durations`/
/// `loop_*` maps stay empty (a ledger alone carries no per-step live
/// timing); `step_summaries`/`step_summary_at` are instead populated from
/// `ledger_path`'s P521 activity sidecar (via [`story::load_activity`])
/// when one exists, so a reconstructed journey pane's per-step summaries and
/// the history pane's per-event rows agree with what a live run would have
/// shown — never a second summary source. The header's own elapsed derives
/// from `session.ledger.elapsed_seconds` instead of a live `Instant`, and its
/// aggregate token line is populated from `session.last_drive_outcome`'s
/// persisted counters under a synthetic key that intentionally matches no
/// real step (so per-step token display correctly stays absent, while the
/// header total — the only thing the ledger can support — still renders).
/// P081: everything a ledger + P521 activity sidecar alone can supply for a
/// [`PresentationState`] — shared by [`render_ledger_run_view`] (the
/// dashboard's own preview/attach reconstruction) and the observer
/// [`RunPanel`]'s [`RunPanel::new_observer`]/[`RunPanel::refresh_from_ledger`],
/// which seed/re-derive a live panel's state from exactly the same source
/// rather than duplicating this derivation. `activity`/`started_at_epoch` are
/// carried through (not folded into the maps below) because the observer
/// also needs them to rebuild `current_stream` — a live-only field this
/// struct itself has no opinion on.
pub(super) struct LedgerPresentationSeed {
    pub(super) run_started: Instant,
    pub(super) output_tokens: BTreeMap<String, u64>,
    pub(super) narrator_tokens: u64,
    pub(super) guide_tokens: u64,
    pub(super) step_summaries: BTreeMap<String, String>,
    pub(super) step_summary_at: BTreeMap<String, Duration>,
    pub(super) activity: Option<ctx_traits_core::procedure::story::ActivityInput>,
    pub(super) started_at_epoch: Option<u64>,
}

pub(super) fn ledger_presentation_seed(
    session: &ctx_traits_core::procedure::session::Session,
    ledger_path: &camino::Utf8Path,
) -> LedgerPresentationSeed {
    let elapsed = Duration::from_secs(session.ledger.elapsed_seconds);
    let run_started = Instant::now()
        .checked_sub(elapsed)
        .unwrap_or_else(Instant::now);
    let token_usage = session
        .last_drive_outcome
        .as_ref()
        .and_then(|outcome| outcome.token_usage.as_ref());
    let mut output_tokens = BTreeMap::new();
    if let Some(work_tokens) = token_usage.and_then(|usage| usage.work_tokens) {
        output_tokens.insert("__ledger-total__".to_string(), work_tokens);
    }
    let narrator_tokens = token_usage
        .and_then(|usage| usage.narrator_tokens)
        .unwrap_or(0);
    let guide_tokens = token_usage
        .and_then(|usage| usage.guide_tokens)
        .unwrap_or(0);
    let started_at_epoch = session.provenance.started_at_epoch;
    let activity = story::load_activity(ledger_path);
    let (step_summaries, step_summary_at) = activity
        .as_ref()
        .map(|activity| sidecar_step_summary_maps(activity, started_at_epoch))
        .unwrap_or_default();
    LedgerPresentationSeed {
        run_started,
        output_tokens,
        narrator_tokens,
        guide_tokens,
        step_summaries,
        step_summary_at,
        activity,
        started_at_epoch,
    }
}

/// P081: applies [`ledger_presentation_seed`] to a live [`RunPanelState`] —
/// the observer panel's counterpart of [`render_ledger_run_view`]'s local
/// bindings. Also rebuilds `current_stream` from the sidecar's latest-frame
/// events (there is no live narration stream to accumulate for an observer),
/// via the SAME [`latest_frame_event_rows`] the ledger reconstruction's
/// CURRENT pane uses. Does not itself call `rebuild_view`/`render_locked` —
/// callers combine this with whichever presentation-only follow-up
/// (finished-note, terminal close) their own call site needs first.
pub(super) fn apply_ledger_seed(state: &mut RunPanelState, ledger_path: &camino::Utf8Path) {
    let seed = ledger_presentation_seed(&state.session, ledger_path);
    // `elapsed_seconds` is stepwise-constant between the drive's call/advance
    // persists, so a refresh may only back-date `run_started` further (larger
    // displayed elapsed), never pull it forward and reset a locally ticking
    // clock — the same ratchet `observe_elapsed_seconds` applies with `max()`.
    state.run_started = state.run_started.min(seed.run_started);
    state.output_tokens = seed.output_tokens;
    state.narrator_tokens = seed.narrator_tokens;
    state.ledger_guide_tokens = seed.guide_tokens;
    state.step_summaries = seed.step_summaries;
    state.step_summary_at = seed.step_summary_at;
    let current_frame = seed
        .activity
        .as_ref()
        .map(|activity| latest_frame_event_rows(activity, seed.started_at_epoch));
    let narrated = current_frame.as_ref().is_some_and(|frame| frame.narrated);
    let row_kind = if narrated {
        StreamRowKind::Narration
    } else {
        StreamRowKind::ModelText
    };
    state.current_stream = current_frame
        .map(|frame| frame.rows)
        .unwrap_or_default()
        .into_iter()
        .map(|row| StreamRow {
            at: row.at.unwrap_or_default(),
            kind: row_kind,
            text: row.tail,
        })
        .collect();
    // P081: a retained ask-refusal notice survives this wholesale rebuild —
    // otherwise the observer's "never silence" rule holds for at most one
    // `RELOAD_INTERVAL` poll. See `observer_notice`'s doc comment.
    if let Some(notice) = state.observer_notice.clone() {
        state.current_stream.push_back(notice);
        while state.current_stream.len() > CURRENT_STREAM_CAP {
            state.current_stream.pop_front();
        }
    }
}

pub(crate) fn render_ledger_run_view(
    trait_ref: &ctx_traits_core::Trait,
    plan: &ctx_traits_core::procedure::run::Plan,
    session: &ctx_traits_core::procedure::session::Session,
    ledger_path: &camino::Utf8Path,
) -> LedgerPaneProjection {
    let seed = ledger_presentation_seed(session, ledger_path);
    let view = run_view(
        trait_ref,
        plan,
        session,
        None,
        PresentationState {
            active_started: &None,
            finished_durations: &BTreeMap::new(),
            output_tokens: &seed.output_tokens,
            loop_elapsed: &BTreeMap::new(),
            loop_output_tokens: &BTreeMap::new(),
            step_summaries: &seed.step_summaries,
            step_summary_at: &seed.step_summary_at,
            narrator_tokens: seed.narrator_tokens,
            guide_tokens: seed.guide_tokens,
            run_started: seed.run_started,
            live_drive: false,
        },
    );
    // P552 review `dashboard-attach-contract-absent`: history/current are
    // supplied ONLY when this ledger actually has a P521 activity sidecar —
    // never derived from the ledger's own step states as a fallback, or a
    // legacy session's `activity_available: false` would still show a
    // populated history pane its own presence contradicts.
    let history = seed
        .activity
        .as_ref()
        .map(|_| story_history_lines(&view))
        .unwrap_or_default();
    let (current, activity_degraded) = current_and_degraded(&seed.activity, seed.started_at_epoch);
    LedgerPaneProjection {
        progress: progress_lines(&view),
        journey: journey_lines(&view),
        landing: landing_lines_from_frames(view.header.completed, &session.provenance.merge_frames),
        history,
        current,
        activity_available: seed.activity.is_some(),
        activity_degraded,
        trait_name: trait_ref.name.as_str().to_string(),
        started_at_epoch: seed.started_at_epoch,
    }
}

/// Attached sessions retain merge frames rather than a live `RunPanel`'s
/// folded events. Those frames are sufficient evidence that landing work was
/// observed, and avoid inventing another persisted projection.
pub(crate) fn landing_lines_from_frames(
    completed: bool,
    frames: &[ctx_traits_core::procedure::session::MergeFrame],
) -> Vec<tui::Line> {
    if !completed || frames.is_empty() {
        return Vec::new();
    }
    let mut latest_frames: Vec<&ctx_traits_core::procedure::session::MergeFrame> = Vec::new();
    for frame in frames {
        if let Some(index) = latest_frames
            .iter()
            .position(|existing| existing.stage == frame.stage)
        {
            latest_frames[index] = frame;
        } else {
            latest_frames.push(frame);
        }
    }
    latest_frames
        .into_iter()
        .map(|frame| {
            use ctx_traits_core::procedure::session::MergeStatus;
            let failed = matches!(
                frame.status,
                MergeStatus::Parked
                    | MergeStatus::PostMergeCleanupFailure
                    | MergeStatus::RecoveryFailure
            );
            let tone = if failed {
                tui::Tone::Fail
            } else {
                tui::Tone::Pass
            };
            let mut line = tui::Line::blank();
            line.push(if failed { "× " } else { "✓ " }, tone);
            line.push(merge_story::stage_text(frame.stage).to_string(), tone);
            line.push("   ", tui::Tone::Muted);
            line.push(
                merge_story::explain_frame(frame).sentence,
                tui::Tone::Default,
            );
            line
        })
        .collect()
}

/// The sidecar-only slice of [`LedgerPaneProjection`]: `current`/`history`
/// and the `activity_available`/`activity_degraded` authority, sourced
/// purely from [`story::load_activity`] — no [`ctx_traits_core::Trait`]
/// or [`ctx_traits_core::procedure::run::Plan`] involved. Its history retains
/// the ledger's persisted status title and position path, so an unchanged
/// dashboard refresh cannot replace round-aware rows with opaque sidecar keys.
/// Shared by the full [`render_ledger_run_view`] reconstruction's
/// trait-resolution-failure fallback and by [`load_sidecar_activity_summary`],
/// the dashboard-only entry point an unchanged-digest refresh calls without
/// re-resolving (or re-attempting to resolve) the trait/plan (P552 review
/// `dashboard-attach-contract-absent`: digest equality alone must be enough
/// to pick this path, so it has to carry everything the sidecar can supply,
/// not just `current`).
pub(crate) struct SidecarActivitySummary {
    pub(crate) history: Vec<EventRow>,
    pub(crate) current: Vec<EventRow>,
    pub(crate) activity_available: bool,
    pub(crate) activity_degraded: Option<String>,
}

/// P552 review `dashboard-attach-contract-absent`: sidecar existence is an
/// independent persisted fact from whether `ledger_path`'s pinned trait can
/// still be resolved — this is the ONE place both a trait-reconstruction
/// failure and the unchanged-digest dashboard refresh read it from, so
/// neither loses history/current availability just because trait loading
/// failed or was skipped as an optimization.
pub(crate) fn load_sidecar_activity_summary(
    session: &ctx_traits_core::procedure::session::Session,
    ledger_path: &camino::Utf8Path,
    started_at_epoch: Option<u64>,
) -> SidecarActivitySummary {
    let activity = story::load_activity(ledger_path);
    let (current, activity_degraded) = current_and_degraded(&activity, started_at_epoch);
    let history = activity
        .as_ref()
        .map(|activity| sidecar_history_lines(session, activity, started_at_epoch))
        .unwrap_or_default();
    SidecarActivitySummary {
        history,
        current,
        activity_available: activity.is_some(),
        activity_degraded,
    }
}

/// Sidecar-only history rows (P552 review `dashboard-attach-contract-absent`):
/// one row per accepted ledger status in ledger order. Sidecar summaries join
/// by the status' structural key prefix, including its persisted iteration,
/// rather than title or current loop state.
fn sidecar_history_lines(
    session: &ctx_traits_core::procedure::session::Session,
    activity: &ctx_traits_core::procedure::story::ActivityInput,
    started_at_epoch: Option<u64>,
) -> Vec<EventRow> {
    let accepted = session
        .ledger
        .sequence_statuses
        .iter()
        .filter(|status| {
            status.status == ctx_traits_core::procedure::runtime::SequenceStatusKind::Accepted
        })
        .collect::<Vec<_>>();
    if accepted.is_empty() {
        let mut entries: Vec<_> = activity.step_summaries.iter().collect();
        entries.sort_by_key(|entry| entry.at_epoch_ms);
        return entries
            .into_iter()
            .map(|entry| {
                EventRow::new(
                    (entry.at_epoch_ms > 0)
                        .then(|| epoch_ms_to_duration(started_at_epoch, entry.at_epoch_ms)),
                    format!("{}: {}", entry.key, entry.text),
                    tui::Tone::Default,
                )
            })
            .collect();
    }
    accepted
        .into_iter()
        .map(|status| {
            let key_prefix = if status.position_path.len() <= 1 {
                format!(
                    "{}:{}:",
                    status.run_index,
                    status.item_id.as_deref().unwrap_or_default()
                )
            } else {
                format!(
                    "{}:{}:",
                    status.run_index,
                    structural_path_key(&status.position_path)
                )
            };
            let summary = activity
                .step_summaries
                .iter()
                .find(|entry| entry.key.starts_with(&key_prefix));
            let tail = match summary {
                Some(summary) => format!(
                    "{}: {}",
                    story::format_step_title(&status.title, &status.position_path),
                    summary.text
                ),
                None => story::format_step_title(&status.title, &status.position_path),
            };
            EventRow::new(
                summary.and_then(|entry| {
                    (entry.at_epoch_ms > 0)
                        .then(|| epoch_ms_to_duration(started_at_epoch, entry.at_epoch_ms))
                }),
                tail,
                summary.map_or(tui::Tone::Muted, |_| tui::Tone::Default),
            )
        })
        .collect()
}

fn current_and_degraded(
    activity: &Option<ctx_traits_core::procedure::story::ActivityInput>,
    started_at_epoch: Option<u64>,
) -> (Vec<EventRow>, Option<String>) {
    let current = activity
        .as_ref()
        .map(|activity| latest_frame_event_rows(activity, started_at_epoch).rows)
        .unwrap_or_default();
    let degraded = match activity {
        None => Some("no activity was recorded for this run".to_string()),
        Some(activity) if activity.skipped_lines > 0 => Some(format!(
            "activity partially recorded for this run ({} unparseable line(s) skipped)",
            activity.skipped_lines
        )),
        Some(_) => None,
    };
    (current, degraded)
}

fn epoch_ms_to_duration(started_at_epoch: Option<u64>, at_epoch_ms: u64) -> Duration {
    let started_ms = started_at_epoch.map_or(0, |epoch| epoch.saturating_mul(1000));
    Duration::from_millis(at_epoch_ms.saturating_sub(started_ms))
}

/// P552: the ledger reconstruction's `step_summaries`/`step_summary_at`
/// source — the same P455 per-step summaries a live run's `RunPanel` folds
/// in directly, here read back from the P521 activity sidecar so a
/// reconstructed journey pane's step rows and history pane rows agree.
pub(super) fn sidecar_step_summary_maps(
    activity: &ctx_traits_core::procedure::story::ActivityInput,
    started_at_epoch: Option<u64>,
) -> (BTreeMap<String, String>, BTreeMap<String, Duration>) {
    let mut summaries = BTreeMap::new();
    let mut at = BTreeMap::new();
    for entry in &activity.step_summaries {
        summaries.insert(entry.key.clone(), entry.text.clone());
        if entry.at_epoch_ms > 0 {
            at.insert(
                entry.key.clone(),
                epoch_ms_to_duration(started_at_epoch, entry.at_epoch_ms),
            );
        }
    }
    (summaries, at)
}

/// P146: whether [`latest_frame_event_rows`] found parked narrations for the
/// latest frame — the observer's `apply_ledger_seed` needs this to know
/// whether to paint `current_stream` rows as `StreamRowKind::Narration`
/// (same words the live panel showed) or `ModelText` (the quoted fallback).
pub(super) struct LatestFrameRows {
    pub(super) rows: Vec<EventRow>,
    pub(super) narrated: bool,
}

/// P552/P146 attached CURRENT-activity source: the most recently observed
/// frame's rows, in order — not every event ever recorded, and never a
/// debug-trace tail. Prefers parked narrations for that frame (P146: the
/// same words the live panel's narration pane showed); falls back to the
/// frame's raw events, quoted, when no narration was parked (older
/// sessions, narrator disabled) — never the adapter's raw tool-input JSON.
pub(super) fn latest_frame_event_rows(
    activity: &ctx_traits_core::procedure::story::ActivityInput,
    started_at_epoch: Option<u64>,
) -> LatestFrameRows {
    let Some(latest_frame_id) = activity
        .events
        .last()
        .map(|event| event.event.frame_id.clone())
    else {
        return LatestFrameRows {
            rows: Vec::new(),
            narrated: false,
        };
    };
    let narration_rows: Vec<EventRow> = activity
        .narrations
        .iter()
        .filter(|narration| narration.frame_id == latest_frame_id)
        .map(|narration| {
            EventRow::new(
                (narration.at_epoch_ms > 0)
                    .then(|| epoch_ms_to_duration(started_at_epoch, narration.at_epoch_ms)),
                narration.text.clone(),
                tui::Tone::Default,
            )
        })
        .collect();
    if !narration_rows.is_empty() {
        return LatestFrameRows {
            rows: narration_rows,
            narrated: true,
        };
    }
    let rows = activity
        .events
        .iter()
        .filter(|event| event.event.frame_id == latest_frame_id)
        .map(|event| {
            EventRow::new(
                Some(epoch_ms_to_duration(started_at_epoch, event.at_epoch_ms)),
                activity_event_fallback_tail(&event.event),
                activity_event_tone(&event.event.kind),
            )
        })
        .collect();
    LatestFrameRows {
        rows,
        narrated: false,
    }
}

/// The no-narration fallback tail for a raw activity event: `StreamingOutput`/
/// `Thinking` quote the agent's own message text (never rendered raw); a
/// `RunningTool` row shows only the tool label, dropping its raw tool-input
/// JSON `text`; every other kind keeps its kind label. House ruling: quoted
/// agent text or a label, never raw stream JSON.
fn activity_event_fallback_tail(event: &ActivityEvent) -> String {
    match event.kind {
        ActivityKind::StreamingOutput | ActivityKind::Thinking => event
            .text
            .as_deref()
            .map(tui::quote_line)
            .unwrap_or_else(|| activity_kind_label(&event.kind).to_string()),
        ActivityKind::RunningTool => event
            .tool
            .clone()
            .unwrap_or_else(|| activity_kind_label(&event.kind).to_string()),
        _ => activity_kind_label(&event.kind).to_string(),
    }
}

fn activity_kind_label(kind: &ActivityKind) -> &'static str {
    match kind {
        ActivityKind::Dispatching => "dispatching",
        ActivityKind::Thinking => "thinking",
        ActivityKind::RunningTool => "running tool",
        ActivityKind::StreamingOutput => "streaming output",
        ActivityKind::ValidatingOutput => "validating output",
        ActivityKind::Retrying => "retrying",
        ActivityKind::Stalled => "stalled",
        ActivityKind::Compacting => "compacting",
        ActivityKind::NoActivityReported => "no activity reported",
        ActivityKind::RateLimited => "rate limited",
    }
}

fn activity_event_tone(kind: &ActivityKind) -> tui::Tone {
    match kind {
        ActivityKind::ValidatingOutput => tui::Tone::Pass,
        ActivityKind::Stalled | ActivityKind::RateLimited => tui::Tone::Fail,
        _ => tui::Tone::Default,
    }
}

pub(super) fn completed_narration(view: &RunView) -> Option<&RunNarration> {
    view.header
        .completed
        .then_some(view.narration.as_ref())
        .flatten()
        .filter(|n| n.finished)
}

/// The narration to paint under the active step. While the run is live this is
/// the in-progress line (passthrough or narrator text, placeholder included) —
/// gating it on completion killed all live output (Group 44 regression). After
/// completion only the finished settle line remains, rendered by the tail.
pub(super) fn display_narration(view: &RunView) -> Option<&RunNarration> {
    match view.header {
        _ if view.header.completed => view.narration.as_ref().filter(|n| n.finished),
        _ if view.header.stopped.is_some() => None, // A stopped run's "live" narration is stale passthrough from the last frame
        _ => view.narration.as_ref(),
    }
}

pub(super) fn active_step_index(view: &RunView) -> Option<usize> {
    (view.steps.iter().position(|step| step.active))
        .or_else(|| (view.steps.iter()).position(|step| step.state == StepState::Running))
        .or_else(|| (view.steps.iter()).position(|step| step.state == StepState::Failed))
        .or_else(|| (view.steps.iter()).rposition(|step| step.state == StepState::Done))
}

#[cfg(test)]
mod tests {
    use super::super::RunPanel;
    use super::super::planned::{stamp_control_stack_iterations, structural_step_key};
    use super::super::render::{EVENT_PREFIX_SEP, event_row_line, story_row_line};
    use super::*;
    use crate::app::tui_ratatui::RatatuiPane;

    fn activity_event(frame_id: &str, kind: ActivityKind, text: Option<&str>) -> ActivityEvent {
        ActivityEvent {
            sequence: 0,
            frame_id: frame_id.to_string(),
            kind,
            text: text.map(str::to_string),
            tool: None,
            tokens: None,
            rate_limit: None,
        }
    }

    // P549: a fresh stage-boundary event (`Dispatching`) starts a Running
    // row; a `ValidatingOutput` FrameRecorded event closes that same row

    fn planned_item(
        title: &str,
        kind: ctx_traits_core::procedure::run::PlannedSequenceKind,
        run_index: usize,
        sequence_index: usize,
    ) -> ctx_traits_core::procedure::run::PlannedSequenceItem {
        ctx_traits_core::procedure::run::PlannedSequenceItem {
            sequence_index,
            run_index,
            item_id: Some(title.to_string()),
            title: title.to_string(),
            input_refs: Vec::new(),
            output_refs: Vec::new(),
            kind,
            agent_ref: None,
            structural_seat: None,
            sequence_ref: None,
            otherwise_sequence_ref: None,
            prompt_source: None,
            command_plan: None,
            children: Vec::new(),
            otherwise_children: Vec::new(),
            parallel_branches: Vec::new(),
            max_branches: None,
            join: None,
            branch_failure: Vec::new(),
            concurrent: false,
            status: ctx_traits_core::procedure::run::SequenceItemStatus::Planned,
        }
    }

    fn attribution_plan(
        items: Vec<ctx_traits_core::procedure::run::PlannedSequenceItem>,
    ) -> ctx_traits_core::procedure::run::Plan {
        ctx_traits_core::procedure::run::Plan {
            run_id: ctx_traits_core::procedure::run::Id::new("test-run").unwrap(),
            trait_id: "test".to_string(),
            worktree_required: false,
            sequence_items: items,
            slots: Vec::new(),
            producer_edges: Vec::new(),
            port_requirements: Vec::new(),
            output_ports: Vec::new(),
            session_title_sink: None,
            acceptance: ctx_traits_core::procedure::run::AcceptanceState::Pending,
        }
    }

    fn session_with_history_revisions(
        revisions: Vec<ctx_traits_core::procedure::runtime::SlotRevision>,
        control_stack: Vec<ctx_traits_core::procedure::runtime::ControlFrame>,
    ) -> ctx_traits_core::procedure::session::Session {
        use ctx_traits_core::digest::Digest;
        use ctx_traits_core::procedure::runtime::FinalState;
        ctx_traits_core::procedure::session::Session {
            schema_version: "1".to_string(),
            session_id: ctx_traits_core::procedure::session::SessionId::new("session-test")
                .unwrap(),
            run_id: ctx_traits_core::procedure::run::Id::new("test-run").unwrap(),
            trait_id: "test".to_string(),
            source_digest: None,
            canonical_digest: None,
            current_run_index: 0,
            current_source_index: None,
            current_sequence_item_id: None,
            current_sequence_title: None,
            current_agent: None,
            status: ctx_traits_core::procedure::session::Status::AwaitingAgentOutput,
            warnings: Vec::new(),
            accepted_port_values: Vec::new(),
            accepted_slot_values: Vec::new(),
            accepted_output_port_values: Vec::new(),
            slot_revisions: revisions.clone(),
            emitted_signals: Vec::new(),
            rejected_submissions: Vec::new(),
            unresolved_inputs: Vec::new(),
            resource_evidence: Vec::new(),
            provider_capability_reports: Vec::new(),
            output_ports: Vec::new(),
            resolved_settings: Vec::new(),
            resolved_budgets: Vec::new(),
            active_path: Vec::new(),
            control_stack: control_stack.clone(),
            stop_reason: None,
            final_output_summary: Vec::new(),
            next_frame: None,
            last_validation_report: None,
            completion: None,
            last_drive_outcome: None,
            provenance: ctx_traits_core::procedure::session::Provenance {
                started_by: ctx_traits_core::procedure::session::CallerProvenance {
                    surface: "test".to_string(),
                    caller: "test".to_string(),
                    agent: None,
                    harness: None,
                },
                state_source: "test".to_string(),
                agent_assignments: None,
                harness_probes: Vec::new(),
                warnings: Vec::new(),
                trait_source: None,
                query_selection: None,
                worktree: None,
                merge_frames: Vec::new(),
                merge_intent: None,
                out_of_tree_mutations: Vec::new(),
                started_at_epoch: None,
                trust_approval: None,
                session_title: None,
                task_digest: None,
                task_key: None,
                dependency_override: None,
            },
            ledger: ctx_traits_core::procedure::runtime::State {
                run_id: ctx_traits_core::procedure::run::Id::new("test-run").unwrap(),
                trait_id: "test".to_string(),
                strict_loops: false,
                source_digest: None,
                canonical_digest: None,
                current_run_index: 0,
                sequence_statuses: Vec::new(),
                accepted_port_values: Vec::new(),
                accepted_slot_values: Vec::new(),
                accepted_output_port_values: Vec::new(),
                slot_revisions: revisions,
                resource_evidence: Vec::new(),
                emitted_signals: Vec::new(),
                rejected_attempts: Vec::new(),
                provider_capability_reports: Vec::new(),
                output_ports: Vec::new(),
                resolved_settings: Vec::new(),
                resolved_budgets: Vec::new(),
                active_path: Vec::new(),
                control_stack,
                branch_decisions: Vec::new(),
                conditional_input_decisions: Vec::new(),
                ask_decisions: Vec::new(),
                failure_routes: Vec::new(),
                guard_evaluations: Vec::new(),
                parallel_panel_records: Vec::new(),
                stop_reason: None,
                elapsed_seconds: 0,
                final_state: FinalState::Running,
            },
            state_digest: Digest::source("test"),
        }
    }

    fn revision(
        slot: &str,
        path: Vec<ctx_traits_core::procedure::runtime::PathSegment>,
        order: usize,
        value: serde_json::Value,
    ) -> ctx_traits_core::procedure::runtime::SlotRevision {
        use ctx_traits_core::digest::Digest;
        use ctx_traits_core::reference::Reference;
        ctx_traits_core::procedure::runtime::SlotRevision {
            slot_ref: Reference::parse(slot).unwrap(),
            value_digest: Digest::source(&format!("{slot}-{order}")),
            acceptance_order: order,
            operation: None,
            submitted_payload: Some(ctx_traits_core::procedure::runtime::RevisionValue { value }),
            prior_value_digest: None,
            prior_value: None,
            source: None,
            command_execution: None,
            runtime_binding: false,
            projection: None,
            position_path: path,
            loop_id: None,
            iteration_index: None,
            for_each_id: None,
            item_index: None,
        }
    }

    fn parallel_control_frame(
        parallel_buffer: ctx_traits_core::procedure::runtime::EffectBuffer,
    ) -> ctx_traits_core::procedure::runtime::ControlFrame {
        use ctx_traits_core::procedure::runtime::ControlKind;
        ctx_traits_core::procedure::runtime::ControlFrame {
            kind: ControlKind::Parallel,
            parent_run_index: 0,
            control_item_id: Some("parallel".to_string()),
            sequence_id: "parallel".to_string(),
            next_index: 0,
            iteration_index: Some(0),
            max_iterations: None,
            unbounded: false,
            max_items: None,
            item_index: None,
            item_total: None,
            over_slot: None,
            item_slot: None,
            list_digest: None,
            concurrent: false,
            until: None,
            abort_if: None,
            on_exhausted: None,
            on_abort: None,
            on_complete: None,
            on_failure: None,
            parallel_branch_sequence_ids: vec!["branch-a".to_string()],
            parallel_buffer,
            parallel_committed_branches: Vec::new(),
            branch_decisions_watermark: 0,
            guard_evaluations_watermark: 0,
            join: None,
            branch_failure: Vec::new(),
            parallel_branch_refs: Vec::new(),
            parallel_branch_outcomes: Vec::new(),
        }
    }

    fn accepted_status(
        title: &str,
        run_index: usize,
        position_path: Vec<ctx_traits_core::procedure::runtime::PathSegment>,
    ) -> ctx_traits_core::procedure::runtime::SequenceStatus {
        ctx_traits_core::procedure::runtime::SequenceStatus {
            sequence_index: run_index,
            run_index,
            item_id: Some(title.to_string()),
            title: title.to_string(),
            status: ctx_traits_core::procedure::runtime::SequenceStatusKind::Accepted,
            reason: String::new(),
            position_path,
        }
    }

    fn line_text(line: &tui::Line) -> String {
        line.segments().map(|(text, _)| text).collect()
    }

    #[test]
    fn history_outcome_execution_attribution_preserves_root_repeated_and_parallel_paths() {
        use ctx_traits_core::procedure::run::{PlannedParallelBranch, PlannedSequenceKind};
        use ctx_traits_core::procedure::runtime::PathSegment;
        use ctx_traits_core::reference::Reference;

        let mut root = planned_item("root-check", PlannedSequenceKind::Check, 0, 0);
        root.output_refs = vec![Reference::parse("slot:root").unwrap()];
        let mut loop_item = planned_item("repeat", PlannedSequenceKind::Loop, 1, 1);
        loop_item.sequence_ref = Some(Reference::parse("sequence:repeat-body").unwrap());
        let mut repeated_check = planned_item("repeated-check", PlannedSequenceKind::Check, 1, 0);
        repeated_check.output_refs = vec![Reference::parse("slot:repeat").unwrap()];
        loop_item.children = vec![repeated_check];
        let mut parallel = planned_item("parallel", PlannedSequenceKind::Parallel, 2, 2);
        let mut branch_command = planned_item("branch-command", PlannedSequenceKind::Command, 2, 0);
        branch_command.output_refs = vec![Reference::parse("slot:branch").unwrap()];
        parallel.parallel_branches = vec![PlannedParallelBranch {
            sequence_ref: Reference::parse("sequence:branch-a").unwrap(),
            children: vec![branch_command],
        }];
        let plan = attribution_plan(vec![root, loop_item, parallel]);

        let root = accepted_status("root-check", 0, Vec::new());
        assert_eq!(
            planned_item_for_status(&plan, &root).unwrap().0.title,
            "root-check"
        );

        let mut repeated_statuses = Vec::new();
        for iteration in [0, 1] {
            let repeated = accepted_status(
                "repeated-check",
                1,
                vec![
                    PathSegment {
                        kind: "procedure".to_string(),
                        id: Some("repeat".to_string()),
                        index: 1,
                        iteration: None,
                        item_index: None,
                    },
                    PathSegment {
                        kind: "loop".to_string(),
                        id: Some("repeat-body".to_string()),
                        index: 0,
                        iteration: Some(iteration),
                        item_index: None,
                    },
                    PathSegment {
                        kind: "item".to_string(),
                        id: Some("repeated-check".to_string()),
                        index: 0,
                        iteration: Some(iteration),
                        item_index: None,
                    },
                ],
            );
            assert_eq!(
                planned_item_for_status(&plan, &repeated).unwrap().0.title,
                "repeated-check"
            );
            repeated_statuses.push(repeated);
        }

        let parallel = accepted_status(
            "branch-command",
            2,
            vec![
                PathSegment {
                    kind: "procedure".to_string(),
                    id: Some("parallel".to_string()),
                    index: 2,
                    iteration: None,
                    item_index: None,
                },
                PathSegment {
                    kind: "parallel".to_string(),
                    id: Some("branch-a".to_string()),
                    index: 0,
                    iteration: None,
                    item_index: None,
                },
                PathSegment {
                    kind: "item".to_string(),
                    id: Some("branch-command".to_string()),
                    index: 0,
                    iteration: None,
                    item_index: None,
                },
            ],
        );
        assert_eq!(
            planned_item_for_status(&plan, &parallel).unwrap().0.title,
            "branch-command"
        );

        let root_path = canonical_status_path(&root);
        let repeated_paths = repeated_statuses
            .iter()
            .map(canonical_status_path)
            .collect::<Vec<_>>();
        let revisions = vec![
            revision("slot:root", root_path, 1, serde_json::json!({"ok": true})),
            revision(
                "slot:repeat",
                repeated_paths[0].clone(),
                2,
                serde_json::json!({"ok": false, "exit-code": 4}),
            ),
            revision(
                "slot:repeat",
                repeated_paths[1].clone(),
                3,
                serde_json::json!({"ok": true}),
            ),
        ];
        let mut branch_revision = revision(
            "slot:branch",
            canonical_status_path(&parallel),
            4,
            serde_json::Value::Null,
        );
        branch_revision.command_execution = Some(
            ctx_traits_core::procedure::runtime::CommandExecutionEvidence {
                argv: vec!["true".to_string()],
                output_slot: "slot:branch".to_string(),
                executable_digest: None,
                exit_code: Some(0),
                timed_out: false,
                output_tail: None,
            },
        );
        let parallel_frame =
            parallel_control_frame(ctx_traits_core::procedure::runtime::EffectBuffer {
                slot_revisions: vec![branch_revision.clone()],
                ..Default::default()
            });
        let session = session_with_history_revisions(revisions.clone(), vec![parallel_frame]);
        let outcome =
            |item: &ctx_traits_core::procedure::run::PlannedSequenceItem,
             status: &ctx_traits_core::procedure::runtime::SequenceStatus| {
                history_outcome(item, status, &session)
            };
        assert_eq!(
            outcome(planned_item_for_status(&plan, &root).unwrap().0, &root),
            Some(HistoryOutcome::Check {
                ok: true,
                exit_code: None
            })
        );
        assert_eq!(
            outcome(
                planned_item_for_status(&plan, &repeated_statuses[0])
                    .unwrap()
                    .0,
                &repeated_statuses[0],
            ),
            Some(HistoryOutcome::Check {
                ok: false,
                exit_code: Some(4)
            })
        );
        assert_eq!(
            outcome(
                planned_item_for_status(&plan, &repeated_statuses[1])
                    .unwrap()
                    .0,
                &repeated_statuses[1],
            ),
            Some(HistoryOutcome::Check {
                ok: true,
                exit_code: None
            })
        );
        assert_eq!(
            outcome(
                planned_item_for_status(&plan, &parallel).unwrap().0,
                &parallel
            ),
            Some(HistoryOutcome::Command {
                succeeded: true,
                exit_code: Some(0)
            })
        );

        let mut committed_revision = branch_revision;
        committed_revision.acceptance_order = 5;
        committed_revision
            .command_execution
            .as_mut()
            .unwrap()
            .exit_code = Some(2);
        let mut committed_frame = parallel_control_frame(Default::default());
        committed_frame.parallel_committed_branches =
            vec![ctx_traits_core::procedure::runtime::EffectBuffer {
                slot_revisions: vec![committed_revision],
                ..Default::default()
            }];
        let committed_session = session_with_history_revisions(revisions, vec![committed_frame]);
        assert_eq!(
            history_outcome(
                planned_item_for_status(&plan, &parallel).unwrap().0,
                &parallel,
                &committed_session,
            ),
            Some(HistoryOutcome::Command {
                succeeded: false,
                exit_code: Some(2)
            })
        );
    }

    #[test]
    fn history_outcome_attributes_a_top_level_command_to_its_revision() {
        use ctx_traits_core::procedure::run::PlannedSequenceKind;
        use ctx_traits_core::reference::Reference;

        let mut command = planned_item("root-command", PlannedSequenceKind::Command, 0, 0);
        command.output_refs = vec![Reference::parse("slot:command").unwrap()];
        let plan = attribution_plan(vec![command]);
        let status = accepted_status("root-command", 0, Vec::new());
        let mut command_revision = revision(
            "slot:command",
            canonical_status_path(&status),
            1,
            serde_json::Value::Null,
        );
        command_revision.command_execution = Some(
            ctx_traits_core::procedure::runtime::CommandExecutionEvidence {
                argv: vec!["false".to_string()],
                output_slot: "slot:command".to_string(),
                executable_digest: None,
                exit_code: Some(7),
                timed_out: false,
                output_tail: None,
            },
        );
        let session = session_with_history_revisions(vec![command_revision], Vec::new());

        assert_eq!(
            history_outcome(
                planned_item_for_status(&plan, &status).unwrap().0,
                &status,
                &session,
            ),
            Some(HistoryOutcome::Command {
                succeeded: false,
                exit_code: Some(7),
            })
        );
    }

    #[test]
    fn command_history_success_semantics_honor_configured_codes_and_timeouts() {
        use ctx_traits_core::procedure::run::PlannedSequenceKind;
        use ctx_traits_core::r#trait::procedure::CommandPlan;

        let mut command = planned_item("command", PlannedSequenceKind::Command, 0, 0);
        assert!(command_succeeded(&command, Some(0), false));
        assert!(!command_succeeded(&command, Some(1), false));
        command.command_plan = Some(CommandPlan {
            argv: vec!["test".to_string()],
            argv_from: None,
            executable_digest_from: None,
            cwd: None,
            timeout_ms: None,
            idle_timeout_ms: None,
            capture_bytes: None,
            success_exit_code: vec![3],
        });
        assert!(command_succeeded(&command, Some(3), false));
        assert!(!command_succeeded(&command, Some(1), false));
        assert!(!command_succeeded(&command, Some(3), true));

        let row = |succeeded, exit_code| {
            story_row_line(&HistoryStep {
                label: "command".to_string(),
                kind: Some(PlannedSequenceKind::Command),
                outcome: Some(HistoryOutcome::Command {
                    succeeded,
                    exit_code,
                }),
                elapsed: Some(Duration::from_secs(8)),
                output_tokens: None,
                summary: Some("summary".to_string()),
                summary_at: Some(Duration::from_secs(2)),
            })
        };
        assert!(
            row(
                command_succeeded(
                    &planned_item("command", PlannedSequenceKind::Command, 0, 0),
                    Some(0),
                    false
                ),
                Some(0)
            )
            .tail
            .contains("succeeded")
        );
        assert!(
            row(command_succeeded(&command, Some(3), false), Some(3))
                .tail
                .contains("succeeded")
        );
        let failed = row(command_succeeded(&command, Some(1), false), Some(1));
        assert_eq!(failed.tone, tui::Tone::Fail);
        assert!(failed.tail.contains("failed (exit 1)"));
        let timed_out = row(command_succeeded(&command, Some(3), true), Some(3));
        assert_eq!(timed_out.tone, tui::Tone::Fail);
        assert!(timed_out.tail.contains("failed (exit 3)"));
    }

    #[test]
    fn persisted_landing_rows_fold_stages_and_preserve_failures() {
        use ctx_traits_core::procedure::session::{MergeFrame, MergeStage, MergeStatus};

        let frame = |stage, status| MergeFrame {
            stage,
            status,
            reason: None,
            evidence: Vec::new(),
            park_reason: None,
            deep_decisions: Vec::new(),
        };
        let rows = landing_lines_from_frames(
            true,
            &[
                frame(MergeStage::Gates, MergeStatus::GatesPassed),
                frame(MergeStage::Gates, MergeStatus::Parked),
                frame(MergeStage::Cleanup, MergeStatus::PostMergeCleanupFailure),
                frame(MergeStage::Rebase, MergeStatus::RecoveryFailure),
                frame(MergeStage::Landing, MergeStatus::Merged),
            ],
        );
        assert_eq!(rows.len(), 4, "repeated stages update one logical row");
        assert!(line_text(&rows[0]).starts_with("× gates"));
        assert!(line_text(&rows[1]).starts_with("× cleanup"));
        assert!(line_text(&rows[2]).starts_with("× rebase"));
        assert!(line_text(&rows[3]).starts_with("✓ landing"));
    }

    #[test]
    fn accepted_history_uses_persisted_paths_for_top_level_and_resumed_loop_facts() {
        use ctx_traits_core::procedure::runtime::{
            PathSegment, SequenceStatus, SequenceStatusKind,
        };

        let top_level = SequenceStatus {
            sequence_index: 0,
            run_index: 0,
            item_id: Some("prepare".to_string()),
            title: "Prepare workspace".to_string(),
            status: SequenceStatusKind::Accepted,
            reason: String::new(),
            position_path: Vec::new(),
        };
        // This path is from round one even though a resumed run may currently
        // display another branch or a later loop round.
        let resumed_loop = SequenceStatus {
            sequence_index: 0,
            run_index: 1,
            item_id: Some("work".to_string()),
            title: "Execute work".to_string(),
            status: SequenceStatusKind::Accepted,
            reason: String::new(),
            position_path: vec![
                PathSegment {
                    kind: "procedure".to_string(),
                    id: Some("loop-body".to_string()),
                    index: 1,
                    iteration: None,
                    item_index: None,
                },
                PathSegment {
                    kind: "loop".to_string(),
                    id: Some("loop-body".to_string()),
                    index: 0,
                    iteration: Some(0),
                    item_index: None,
                },
                PathSegment {
                    kind: "item".to_string(),
                    id: Some("work".to_string()),
                    index: 0,
                    iteration: Some(0),
                    item_index: None,
                },
            ],
        };
        let top_key = structural_step_key(0, "prepare", &top_level.position_path, "worker");
        let loop_key = structural_step_key(1, "work", &resumed_loop.position_path, "worker");
        let mut elapsed = BTreeMap::new();
        elapsed.insert(top_key.clone(), Duration::from_secs(4));
        elapsed.insert(loop_key.clone(), Duration::from_secs(9));
        let mut tokens = BTreeMap::new();
        tokens.insert(top_key.clone(), 120);
        tokens.insert(loop_key.clone(), 340);
        let mut summaries = BTreeMap::new();
        summaries.insert(top_key.clone(), "prepared".to_string());
        summaries.insert(loop_key.clone(), "executed first round".to_string());
        let mut summary_at = BTreeMap::new();
        summary_at.insert(top_key, Duration::from_secs(5));
        summary_at.insert(loop_key, Duration::from_secs(10));
        let active_started = None;
        let loop_elapsed = BTreeMap::new();
        let loop_tokens = BTreeMap::new();
        let presentation = PresentationState {
            active_started: &active_started,
            finished_durations: &elapsed,
            output_tokens: &tokens,
            loop_elapsed: &loop_elapsed,
            loop_output_tokens: &loop_tokens,
            step_summaries: &summaries,
            step_summary_at: &summary_at,
            narrator_tokens: 0,
            guide_tokens: 0,
            run_started: Instant::now(),
            live_drive: false,
        };

        let top_history = history_step_from_status(&top_level, None, None, &presentation);
        let loop_history = history_step_from_status(&resumed_loop, None, None, &presentation);
        assert_eq!(top_history.label, "Prepare workspace");
        assert_eq!(top_history.elapsed, Some(Duration::from_secs(4)));
        assert_eq!(top_history.output_tokens, Some(120));
        assert_eq!(top_history.summary.as_deref(), Some("prepared"));
        assert_eq!(loop_history.label, "Execute work (1)");
        assert_eq!(loop_history.elapsed, Some(Duration::from_secs(9)));
        assert_eq!(loop_history.output_tokens, Some(340));
        assert_eq!(
            loop_history.summary.as_deref(),
            Some("executed first round")
        );
        assert!(
            event_row_line(&story_row_line(&loop_history), 80)
                .segments()
                .map(|(text, _)| text)
                .collect::<String>()
                .starts_with(&format!("00:00:10{EVENT_PREFIX_SEP}"))
        );
    }

    #[test]
    fn historical_loop_round_is_not_rewritten_by_a_later_live_control_frame() {
        use ctx_traits_core::procedure::runtime::{
            ControlFrame, ControlKind, EffectBuffer, PathSegment, SequenceStatus,
            SequenceStatusKind,
        };

        let historical_path = vec![
            PathSegment {
                kind: "procedure".to_string(),
                id: Some("loop-body".to_string()),
                index: 1,
                iteration: None,
                item_index: None,
            },
            PathSegment {
                kind: "loop".to_string(),
                id: Some("loop-body".to_string()),
                index: 0,
                iteration: Some(0),
                item_index: None,
            },
            PathSegment {
                kind: "item".to_string(),
                id: Some("work".to_string()),
                index: 0,
                iteration: Some(0),
                item_index: None,
            },
        ];
        let live_frame = ControlFrame {
            kind: ControlKind::Loop,
            parent_run_index: 1,
            control_item_id: Some("repeat".to_string()),
            sequence_id: "loop-body".to_string(),
            next_index: 0,
            iteration_index: Some(2),
            max_iterations: None,
            unbounded: false,
            max_items: None,
            item_index: None,
            item_total: None,
            over_slot: None,
            item_slot: None,
            list_digest: None,
            concurrent: false,
            until: None,
            abort_if: None,
            on_exhausted: None,
            on_abort: None,
            on_complete: None,
            on_failure: None,
            parallel_branch_sequence_ids: Vec::new(),
            parallel_buffer: EffectBuffer {
                accepted_slot_values: Vec::new(),
                accepted_output_port_values: Vec::new(),
                slot_revisions: Vec::new(),
                emitted_signals: Vec::new(),
            },
            parallel_committed_branches: Vec::new(),
            branch_decisions_watermark: 0,
            guard_evaluations_watermark: 0,
            join: None,
            branch_failure: Vec::new(),
            parallel_branch_refs: Vec::new(),
            parallel_branch_outcomes: Vec::new(),
        };
        let live_path = stamp_control_stack_iterations(&[live_frame], &historical_path);
        assert_eq!(live_path[1].iteration, Some(2));

        let status = SequenceStatus {
            sequence_index: 0,
            run_index: 1,
            item_id: Some("work".to_string()),
            title: "Execute work".to_string(),
            status: SequenceStatusKind::Accepted,
            reason: String::new(),
            position_path: historical_path,
        };
        let active_started = None;
        let elapsed = BTreeMap::new();
        let tokens = BTreeMap::new();
        let loop_elapsed = BTreeMap::new();
        let loop_tokens = BTreeMap::new();
        let summaries = BTreeMap::new();
        let summary_at = BTreeMap::new();
        let presentation = PresentationState {
            active_started: &active_started,
            finished_durations: &elapsed,
            output_tokens: &tokens,
            loop_elapsed: &loop_elapsed,
            loop_output_tokens: &loop_tokens,
            step_summaries: &summaries,
            step_summary_at: &summary_at,
            narrator_tokens: 0,
            guide_tokens: 0,
            run_started: Instant::now(),
            live_drive: false,
        };
        assert_eq!(
            history_step_from_status(&status, None, None, &presentation).label,
            "Execute work (1)"
        );
    }

    #[test]
    fn accepted_loop_and_for_each_control_history_retain_aggregate_facts() {
        use ctx_traits_core::procedure::runtime::{SequenceStatus, SequenceStatusKind};

        let active_started = None;
        let elapsed = BTreeMap::new();
        let tokens = BTreeMap::new();
        let summaries = BTreeMap::new();
        let summary_at = BTreeMap::new();
        let mut loop_elapsed = BTreeMap::new();
        let mut loop_tokens = BTreeMap::new();
        for (item_id, run_index, elapsed_seconds, output_tokens) in [
            (Some("repeat-checks"), 2, 42, 900),
            (Some("each-check"), 3, 17, 300),
            (None, 4, 13, 200),
            (None, 5, 11, 100),
        ] {
            let key = format!("loop:procedure:{}:{run_index}", item_id.unwrap_or_default());
            loop_elapsed.insert(key.clone(), Duration::from_secs(elapsed_seconds));
            loop_tokens.insert(key, output_tokens);
        }
        let presentation = PresentationState {
            active_started: &active_started,
            finished_durations: &elapsed,
            output_tokens: &tokens,
            loop_elapsed: &loop_elapsed,
            loop_output_tokens: &loop_tokens,
            step_summaries: &summaries,
            step_summary_at: &summary_at,
            narrator_tokens: 0,
            guide_tokens: 0,
            run_started: Instant::now(),
            live_drive: false,
        };

        for (item_id, title, run_index, reason, elapsed_seconds, output_tokens) in [
            (
                Some("repeat-checks"),
                "Repeat checks",
                2,
                "control item completed: loop",
                42,
                900,
            ),
            (
                Some("each-check"),
                "Check each item",
                3,
                "control item completed: for-each",
                17,
                300,
            ),
            (
                None,
                "Repeat anonymous checks",
                4,
                "control item completed: loop",
                13,
                200,
            ),
            (
                None,
                "Check anonymous items",
                5,
                "control item completed: for-each",
                11,
                100,
            ),
        ] {
            let status = SequenceStatus {
                sequence_index: run_index,
                run_index,
                item_id: item_id.map(str::to_string),
                title: title.to_string(),
                status: SequenceStatusKind::Accepted,
                reason: reason.to_string(),
                position_path: Vec::new(),
            };
            let history = history_step_from_status(&status, None, None, &presentation);
            assert_eq!(history.elapsed, Some(Duration::from_secs(elapsed_seconds)));
            assert_eq!(history.output_tokens, Some(output_tokens));
            let row = story_row_line(&history);
            assert!(
                row.tail
                    .contains(&tui::elapsed_text(Duration::from_secs(elapsed_seconds)))
            );
            assert!(row.tail.contains(&format!("{output_tokens} tok")));
            let line = event_row_line(&row, 80);
            let text = line.segments().map(|(text, _)| text).collect::<String>();
            assert!(text.starts_with(&format!(
                "00:00:{elapsed_seconds:02}{EVENT_PREFIX_SEP}{title}"
            )));
        }
    }

    #[test]
    fn latest_frame_current_activity_keeps_a_known_zero_timestamp() {
        let activity = ctx_traits_core::procedure::story::ActivityInput {
            events: vec![ctx_traits_core::procedure::story::TimedActivityEvent {
                at_epoch_ms: 0,
                event: activity_event("frame", ActivityKind::StreamingOutput, Some("working")),
            }],
            step_summaries: Vec::new(),
            narrations: Vec::new(),
            skipped_lines: 0,
        };
        let frame = latest_frame_event_rows(&activity, None);
        assert!(!frame.narrated);
        assert_eq!(frame.rows.len(), 1);
        assert_eq!(frame.rows[0].at, Some(Duration::ZERO));
        let rendered = line_text(&event_row_line(&frame.rows[0], 80));
        assert!(rendered.starts_with(&format!("00:00:00{EVENT_PREFIX_SEP}\"working\"")));
    }

    #[test]
    fn latest_frame_prefers_parked_narrations_over_raw_events() {
        let activity = ctx_traits_core::procedure::story::ActivityInput {
            events: vec![ctx_traits_core::procedure::story::TimedActivityEvent {
                at_epoch_ms: 0,
                event: activity_event("frame", ActivityKind::RunningTool, Some("{\"path\":\"a\"}")),
            }],
            step_summaries: Vec::new(),
            narrations: vec![
                ctx_traits_core::procedure::story::TimedNarration {
                    at_epoch_ms: 1,
                    frame_id: "frame".to_string(),
                    text: "Reading a.rs".to_string(),
                },
                ctx_traits_core::procedure::story::TimedNarration {
                    at_epoch_ms: 2,
                    frame_id: "frame".to_string(),
                    text: "Editing a.rs".to_string(),
                },
            ],
            skipped_lines: 0,
        };
        let frame = latest_frame_event_rows(&activity, None);
        assert!(frame.narrated);
        assert_eq!(frame.rows.len(), 2);
        assert_eq!(frame.rows[0].tail, "Reading a.rs");
        assert_eq!(frame.rows[1].tail, "Editing a.rs");
        for row in &frame.rows {
            assert!(!row.tail.contains("path"));
        }
    }

    #[test]
    fn latest_frame_fallback_never_shows_raw_tool_json() {
        let activity = ctx_traits_core::procedure::story::ActivityInput {
            events: vec![ctx_traits_core::procedure::story::TimedActivityEvent {
                at_epoch_ms: 0,
                event: {
                    let mut event = activity_event(
                        "frame",
                        ActivityKind::RunningTool,
                        Some("{\"path\":\"a\"}"),
                    );
                    event.tool = Some("edit".to_string());
                    event
                },
            }],
            step_summaries: Vec::new(),
            narrations: Vec::new(),
            skipped_lines: 0,
        };
        let frame = latest_frame_event_rows(&activity, None);
        assert!(!frame.narrated);
        assert_eq!(frame.rows.len(), 1);
        assert_eq!(frame.rows[0].tail, "edit");
        assert!(!frame.rows[0].tail.contains("path"));
    }

    #[test]
    fn reconstructed_untimed_sidecar_summary_has_no_timestamp_prefix() {
        let activity = ctx_traits_core::procedure::story::ActivityInput {
            events: Vec::new(),
            step_summaries: vec![ctx_traits_core::procedure::story::TimedStepSummary {
                at_epoch_ms: 0,
                key: "0:check:worker".to_string(),
                role: "worker".to_string(),
                text: "completed".to_string(),
            }],
            narrations: Vec::new(),
            skipped_lines: 0,
        };
        let (summaries, summary_at) = sidecar_step_summary_maps(&activity, None);
        let status = accepted_status("check", 0, Vec::new());
        let none = None;
        let empty_durations = BTreeMap::new();
        let empty_tokens = BTreeMap::new();
        let presentation = PresentationState {
            active_started: &none,
            finished_durations: &empty_durations,
            output_tokens: &empty_tokens,
            loop_elapsed: &empty_durations,
            loop_output_tokens: &empty_tokens,
            step_summaries: &summaries,
            step_summary_at: &summary_at,
            narrator_tokens: 0,
            guide_tokens: 0,
            run_started: Instant::now(),
            live_drive: false,
        };
        let row = story_row_line(&history_step_from_status(
            &status,
            None,
            None,
            &presentation,
        ));
        assert_eq!(row.at, None);
        let rendered = line_text(&event_row_line(&row, 20));
        assert_eq!(rendered, "check: completed");
        assert!(!rendered.contains("00:00:00"));
    }

    #[test]
    fn render_ledger_run_view_keeps_an_untimed_sidecar_summary_untimed() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let trait_ref: ctx_traits_core::Trait = toml::from_str(
            r#"
id = "history-test"
schema-version = "0.4"
version = "0.1.0"
name = "History Test"
description = "A test trait."
"#,
        )
        .expect("minimal trait parses");
        let plan = attribution_plan(vec![planned_item(
            "check",
            ctx_traits_core::procedure::run::PlannedSequenceKind::Check,
            0,
            0,
        )]);
        let mut session = session_with_history_revisions(Vec::new(), Vec::new());
        session.ledger.sequence_statuses = vec![accepted_status("check", 0, Vec::new())];
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let ledger_path = camino::Utf8PathBuf::from(format!(
            "/tmp/ctx-traits-run-view-{}-{nonce}.json",
            std::process::id(),
        ));
        let sidecar_path = ctx_traits_io::activity_sidecar::activity_path(&ledger_path);
        let summary = ctx_traits_io::activity_sidecar::ActivityRecord::StepSummary {
            at_epoch_ms: 0,
            key: "0:check:worker".to_string(),
            role: "worker".to_string(),
            text: "completed".to_string(),
        };
        std::fs::write(
            sidecar_path.as_std_path(),
            format!("{}\n", serde_json::to_string(&summary).unwrap()),
        )
        .expect("write activity sidecar");

        let projection = render_ledger_run_view(&trait_ref, &plan, &session, &ledger_path);
        let row = projection.history.first().expect("one reconstructed row");
        assert_eq!(row.at, None);
        let rendered = line_text(&event_row_line(row, 20));
        assert_eq!(rendered, "check: completed");
        assert!(!rendered.contains("00:00:00"));

        let _ = std::fs::remove_file(sidecar_path.as_std_path());
    }

    /// P081: `ledger_presentation_seed` is the observer panel's own state
    /// seed (`RunPanel::new_observer`) and re-derivation source
    /// (`RunPanel::refresh_from_ledger`) — this exercises it directly,
    /// independent of any live `RunPanel`/terminal, covering the draft's
    /// "observer panel seeds clock from start epoch" and "token rows" unit
    /// coverage.
    #[test]
    fn ledger_presentation_seed_back_dates_clock_and_reads_persisted_tokens() {
        let mut session = session_with_history_revisions(Vec::new(), Vec::new());
        session.ledger.elapsed_seconds = 90;
        session.last_drive_outcome = Some(ctx_traits_core::procedure::session::DriveOutcome {
            outcome: ctx_traits_core::procedure::session::DriveOutcomeKind::Running,
            recorded_at_epoch: 0,
            provider_credits_pause: None,
            effective_budget: None,
            token_usage: Some(ctx_traits_core::procedure::session::TokenUsageEvidence {
                work_tokens: Some(42),
                narrator_tokens: Some(7),
                narration_complete: None,
                guide_tokens: Some(3),
                guide_complete: None,
            }),
            exit_code: None,
            rate_limit: None,
            budget_pause: None,
            tokens_by_model: None,
        });
        let ledger_path = camino::Utf8PathBuf::from(format!(
            "/tmp/ctx-traits-run-view-ledger-seed-{}.json",
            std::process::id()
        ));

        let seed = ledger_presentation_seed(&session, &ledger_path);

        // Back-dated by the ledger's own persisted elapsed seconds, so the
        // panel's existing 1s clock timer reads a truthful header elapsed
        // immediately, before any tick.
        let observed_elapsed = seed.run_started.elapsed();
        assert!(
            observed_elapsed >= Duration::from_secs(90),
            "run_started must be back-dated by the ledger's elapsed_seconds, got {observed_elapsed:?}"
        );
        assert!(
            observed_elapsed < Duration::from_secs(95),
            "run_started must not be back-dated by more than the ledger records, got {observed_elapsed:?}"
        );
        assert_eq!(
            seed.output_tokens.get("__ledger-total__").copied(),
            Some(42)
        );
        assert_eq!(seed.narrator_tokens, 7);
        assert_eq!(seed.guide_tokens, 3);
        assert!(seed.step_summaries.is_empty());
        assert!(seed.activity.is_none());
    }

    /// P081 regression: `RunPanel::refresh_from_ledger`'s periodic
    /// `apply_ledger_seed` call must never move the header clock BACKWARD.
    /// `elapsed_seconds` is stepwise-constant between the drive's persisted
    /// call/advance boundaries, so re-deriving `run_started = now -
    /// elapsed_seconds` on every poll (without ratcheting against the
    /// state's own already-ticking `run_started`) produces a sawtooth: the
    /// displayed elapsed ticks up locally between polls, then snaps back to
    /// the stale persisted value on every poll. `state.run_started =
    /// state.run_started.min(seed.run_started)` fixes this — proven here by
    /// polling twice across a real sleep with an unchanged ledger and
    /// asserting the observed elapsed only ever grows.
    #[test]
    fn apply_ledger_seed_ratchets_run_started_and_never_reverses_the_clock() {
        let trait_ref: ctx_traits_core::Trait = toml::from_str(
            r#"
id = "clock-test"
schema-version = "0.4"
version = "0.1.0"
name = "Clock Test"
description = "A test trait."
"#,
        )
        .expect("minimal trait parses");
        let plan = attribution_plan(vec![planned_item(
            "check",
            ctx_traits_core::procedure::run::PlannedSequenceKind::Check,
            0,
            0,
        )]);
        let mut session = session_with_history_revisions(Vec::new(), Vec::new());
        session.ledger.elapsed_seconds = 90;
        let ledger_path = camino::Utf8PathBuf::from(format!(
            "/tmp/ctx-traits-run-view-clock-ratchet-{}.json",
            std::process::id()
        ));

        let panel = RunPanel::new_observer(
            "clock-test".to_string(),
            trait_ref,
            plan,
            session.clone(),
            ledger_path.clone(),
            RatatuiPane::new_detached_for_test(),
        );

        let elapsed_before = panel
            .state
            .lock()
            .expect("state lock")
            .run_started
            .elapsed();

        std::thread::sleep(Duration::from_millis(50));
        // The ledger itself is UNCHANGED — `elapsed_seconds` still reads 90.
        // Without the ratchet this poll would re-derive the exact same
        // `run_started` as construction and the observed elapsed would stay
        // pinned at ~90s instead of growing by the real sleep.
        panel.refresh_from_ledger(&session, &ledger_path);

        let elapsed_after = panel
            .state
            .lock()
            .expect("state lock")
            .run_started
            .elapsed();

        assert!(
            elapsed_after > elapsed_before + Duration::from_millis(10),
            "observed elapsed must grow across a poll of an unchanged ledger, \
             got before={elapsed_before:?} after={elapsed_after:?}"
        );
    }
}
