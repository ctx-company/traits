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
    pub(crate) post_run: Vec<tui::Line>,
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
    let current_rows = seed
        .activity
        .as_ref()
        .map(|activity| latest_frame_event_rows(activity, seed.started_at_epoch))
        .unwrap_or_default();
    state.current_stream = current_rows
        .into_iter()
        .map(|row| StreamRow {
            at: row.at.unwrap_or_default(),
            kind: StreamRowKind::ModelText,
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
        post_run: post_run_lines_from_frames(
            view.header.completed,
            &session.provenance.merge_frames,
        ),
        history,
        current,
        activity_available: seed.activity.is_some(),
        activity_degraded,
        trait_name: trait_ref.name.as_str().to_string(),
        started_at_epoch: seed.started_at_epoch,
    }
}

/// Attached sessions retain merge frames rather than a live `RunPanel`'s
/// folded events. Those frames are sufficient evidence that post-run work was
/// observed, and avoid inventing another persisted projection.
pub(crate) fn post_run_lines_from_frames(
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
        .map(|activity| latest_frame_event_rows(activity, started_at_epoch))
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

/// P552 attached CURRENT-activity source (item 8 of the implementation
/// draft): the most recently observed frame's own events, in order — not
/// every event ever recorded, and never a debug-trace tail.
pub(super) fn latest_frame_event_rows(
    activity: &ctx_traits_core::procedure::story::ActivityInput,
    started_at_epoch: Option<u64>,
) -> Vec<EventRow> {
    let Some(latest_frame_id) = activity
        .events
        .last()
        .map(|event| event.event.frame_id.clone())
    else {
        return Vec::new();
    };
    activity
        .events
        .iter()
        .filter(|event| event.event.frame_id == latest_frame_id)
        .map(|event| {
            EventRow::new(
                Some(epoch_ms_to_duration(started_at_epoch, event.at_epoch_ms)),
                activity_event_tail(&event.event),
                activity_event_tone(&event.event.kind),
            )
        })
        .collect()
}

fn activity_event_tail(event: &ActivityEvent) -> String {
    event
        .text
        .clone()
        .or_else(|| event.tool.clone())
        .unwrap_or_else(|| activity_kind_label(&event.kind).to_string())
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
    }
}

fn activity_event_tone(kind: &ActivityKind) -> tui::Tone {
    match kind {
        ActivityKind::ValidatingOutput => tui::Tone::Pass,
        ActivityKind::Stalled => tui::Tone::Fail,
        _ => tui::Tone::Default,
    }
}

pub(super) fn completed_narration(view: &RunView) -> Option<&RunNarration> {
    view.header
        .completed
        .then_some(view.narration.as_ref())
        .flatten()
        .filter(|narration| narration.finished)
}

/// The narration to paint under the active step. While the run is live this is
/// the in-progress line (passthrough or narrator text, placeholder included) —
/// gating it on completion killed all live output (Group 44 regression). After
/// completion only the finished settle line remains, rendered by the tail.
pub(super) fn display_narration(view: &RunView) -> Option<&RunNarration> {
    if view.header.completed {
        view.narration
            .as_ref()
            .filter(|narration| narration.finished)
    } else if view.header.stopped.is_some() {
        // A stopped run's "live" narration is stale passthrough from the last
        // frame; the header's stop line carries the truth instead.
        None
    } else {
        view.narration.as_ref()
    }
}

pub(super) fn active_step_index(view: &RunView) -> Option<usize> {
    view.steps
        .iter()
        .position(|step| step.active)
        .or_else(|| {
            view.steps
                .iter()
                .position(|step| step.state == StepState::Running)
        })
        .or_else(|| {
            view.steps
                .iter()
                .position(|step| step.state == StepState::Failed)
        })
        .or_else(|| {
            view.steps
                .iter()
                .rposition(|step| step.state == StepState::Done)
        })
}
