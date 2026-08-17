//! Planned-tree flattening: turn the dry-run planned sequence plus live
//! session status into the flat `RunStep` list the journey/render code
//! walks. Pure functions over `ctx_traits_core::procedure::run` types.

use std::collections::{BTreeMap, BTreeSet};

use super::{Activity, PortSlug, RunStep, StepState, session_status};
use crate::app::tui;

#[derive(Clone)]
pub(super) struct PlannedItemLocation {
    pub(super) position_path: Vec<ctx_traits_core::procedure::runtime::PathSegment>,
}

impl PlannedItemLocation {
    pub(super) fn root(item: &ctx_traits_core::procedure::run::PlannedSequenceItem) -> Self {
        Self {
            position_path: vec![ctx_traits_core::procedure::runtime::PathSegment {
                kind: "procedure".to_string(),
                id: item.item_id.clone(),
                index: item.run_index,
                iteration: None,
                item_index: None,
            }],
        }
    }
}

/// A loop's own `Accepted`/`Rejected` outcome is a one-time event; once
/// accepted, every descendant should paint `Done` even if a given descendant
/// wasn't part of the final iteration. `force_done` carries that verdict down
/// through the recursion — it starts `false` and latches `true` the moment a
/// `Loop`/`ForEach` ancestor's own step resolves to `StepState::Done`.
pub(super) fn flatten_step(
    item: &ctx_traits_core::procedure::run::PlannedSequenceItem,
    location: &PlannedItemLocation,
    session: &ctx_traits_core::procedure::session::Session,
    harness_by_role: &BTreeMap<String, Vec<(Option<u32>, String)>>,
    accepted: &BTreeSet<String>,
    force_done: bool,
    live_drive: bool,
) -> Vec<RunStep> {
    let step = step_from_item(
        item,
        location,
        session,
        harness_by_role,
        accepted,
        force_done,
        live_drive,
    );
    let child_force_done =
        force_done || (is_loop_kind(&item.kind) && step.state == StepState::Done);
    let mut steps = vec![step];
    let selected_arm =
        branch_decision_for(session, item, location).map(|decision| decision.selected_arm.as_str());
    let include_then = item.kind != ctx_traits_core::procedure::run::PlannedSequenceKind::Branch
        || selected_arm == Some("then");
    let include_otherwise = selected_arm == Some("otherwise");
    for child in item.children.iter().filter(|_| include_then) {
        let child_location = child_location(location, item, false, child);
        steps.extend(flatten_step(
            child,
            &child_location,
            session,
            harness_by_role,
            accepted,
            child_force_done,
            live_drive,
        ));
    }
    for child in item.otherwise_children.iter().filter(|_| include_otherwise) {
        let child_location = child_location(location, item, true, child);
        steps.extend(flatten_step(
            child,
            &child_location,
            session,
            harness_by_role,
            accepted,
            child_force_done,
            live_drive,
        ));
    }
    steps
}

fn is_loop_kind(kind: &ctx_traits_core::procedure::run::PlannedSequenceKind) -> bool {
    matches!(
        kind,
        ctx_traits_core::procedure::run::PlannedSequenceKind::Loop
            | ctx_traits_core::procedure::run::PlannedSequenceKind::ForEach
    )
}

pub(super) fn child_location(
    parent_location: &PlannedItemLocation,
    parent: &ctx_traits_core::procedure::run::PlannedSequenceItem,
    otherwise: bool,
    child: &ctx_traits_core::procedure::run::PlannedSequenceItem,
) -> PlannedItemLocation {
    let sequence_id = if otherwise {
        parent.otherwise_sequence_ref.as_ref()
    } else {
        parent.sequence_ref.as_ref()
    }
    .map(|reference| reference.id().to_string());
    let mut position_path = parent_location.position_path.clone();
    if position_path
        .last()
        .is_some_and(|segment| segment.kind == "item")
    {
        position_path.pop();
    }
    position_path.push(ctx_traits_core::procedure::runtime::PathSegment {
        kind: planned_control_kind(parent.kind.clone()).to_string(),
        id: sequence_id,
        index: child.sequence_index,
        iteration: None,
        item_index: None,
    });
    position_path.push(ctx_traits_core::procedure::runtime::PathSegment {
        kind: "item".to_string(),
        id: child.item_id.clone(),
        index: child.sequence_index,
        iteration: None,
        item_index: None,
    });
    PlannedItemLocation { position_path }
}

pub(super) fn parallel_child_location(
    parent_location: &PlannedItemLocation,
    parent: &ctx_traits_core::procedure::run::PlannedSequenceItem,
    sequence_id: &str,
    child: &ctx_traits_core::procedure::run::PlannedSequenceItem,
) -> PlannedItemLocation {
    let mut position_path = parent_location.position_path.clone();
    if position_path
        .last()
        .is_some_and(|segment| segment.kind == "item")
    {
        position_path.pop();
    }
    position_path.push(ctx_traits_core::procedure::runtime::PathSegment {
        kind: planned_control_kind(parent.kind.clone()).to_string(),
        id: Some(sequence_id.to_string()),
        index: child.sequence_index,
        iteration: None,
        item_index: None,
    });
    position_path.push(ctx_traits_core::procedure::runtime::PathSegment {
        kind: "item".to_string(),
        id: child.item_id.clone(),
        index: child.sequence_index,
        iteration: None,
        item_index: None,
    });
    PlannedItemLocation { position_path }
}

fn planned_control_kind(
    kind: ctx_traits_core::procedure::run::PlannedSequenceKind,
) -> &'static str {
    match kind {
        ctx_traits_core::procedure::run::PlannedSequenceKind::Sequence => "sequence",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Branch => "branch",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Loop => "loop",
        ctx_traits_core::procedure::run::PlannedSequenceKind::ForEach => "for-each",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Parallel => "parallel",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Prompt
        | ctx_traits_core::procedure::run::PlannedSequenceKind::Ask
        | ctx_traits_core::procedure::run::PlannedSequenceKind::Command
        | ctx_traits_core::procedure::run::PlannedSequenceKind::Check
        | ctx_traits_core::procedure::run::PlannedSequenceKind::Project => "",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Terminal => "terminal",
    }
}

fn branch_decision_for<'a>(
    session: &'a ctx_traits_core::procedure::session::Session,
    item: &ctx_traits_core::procedure::run::PlannedSequenceItem,
    location: &PlannedItemLocation,
) -> Option<&'a ctx_traits_core::procedure::runtime::BranchDecision> {
    let item_id = item.item_id.as_deref()?;
    // Iteration-aware: a decision recorded for a prior iteration of an
    // enclosing loop must not satisfy the current iteration's lookup, else
    // rollover keeps painting the previous iteration's selected arm.
    let stamped_path = stamp_live_iterations(session, &location.position_path);
    session
        .ledger
        .branch_decisions
        .iter()
        .rev()
        .find(|decision| {
            decision.parent_run_index == item.run_index
                && decision.branch_id == item_id
                && iteration_aware_path_matches(&decision.position_path, &stamped_path)
        })
}

pub(super) fn structural_path_matches(
    actual: &[ctx_traits_core::procedure::runtime::PathSegment],
    expected: &[ctx_traits_core::procedure::runtime::PathSegment],
) -> bool {
    actual.len() == expected.len()
        && actual.iter().zip(expected).all(|(actual, expected)| {
            actual.kind == expected.kind
                && actual.id == expected.id
                && actual.index == expected.index
        })
}

fn structural_control_ancestor_matches(
    expected: &[ctx_traits_core::procedure::runtime::PathSegment],
    actual: &[ctx_traits_core::procedure::runtime::PathSegment],
) -> bool {
    let prefix = if expected
        .last()
        .is_some_and(|segment| segment.kind == "item")
    {
        &expected[..expected.len().saturating_sub(1)]
    } else {
        expected
    };
    actual.len() > prefix.len()
        && actual.iter().zip(prefix).all(|(actual, expected)| {
            actual.kind == expected.kind
                && actual.id == expected.id
                && actual.index == expected.index
        })
}

fn is_control_item(item: &ctx_traits_core::procedure::run::PlannedSequenceItem) -> bool {
    matches!(
        item.kind,
        ctx_traits_core::procedure::run::PlannedSequenceKind::Sequence
            | ctx_traits_core::procedure::run::PlannedSequenceKind::Branch
            | ctx_traits_core::procedure::run::PlannedSequenceKind::Loop
            | ctx_traits_core::procedure::run::PlannedSequenceKind::ForEach
            | ctx_traits_core::procedure::run::PlannedSequenceKind::Parallel
    )
}

/// Joins a position path's segments into one string, one `kind:id:index:
/// iteration:item_index` field per segment. Used by `structural_step_key`,
/// which wants the iteration in the key (so distinct loop iterations of the
/// same body position get distinct step keys). NOT used by
/// `loop_container_key`, which needs an iteration-independent encoding
/// instead — see that function's doc comment.
pub(super) fn structural_path_key(
    position_path: &[ctx_traits_core::procedure::runtime::PathSegment],
) -> String {
    position_path
        .iter()
        .map(|segment| {
            format!(
                "{}:{}:{}:{}:{}",
                segment.kind,
                segment.id.as_deref().unwrap_or(""),
                segment.index,
                segment
                    .iteration
                    .map_or_else(String::new, |it| it.to_string()),
                segment
                    .item_index
                    .map_or_else(String::new, |it| it.to_string()),
            )
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub(super) fn structural_step_key(
    run_index: usize,
    item_id: &str,
    position_path: &[ctx_traits_core::procedure::runtime::PathSegment],
    role: &str,
) -> String {
    if position_path.len() <= 1 {
        return format!("{run_index}:{item_id}:{role}");
    }
    format!("{run_index}:{}:{role}", structural_path_key(position_path))
}

/// The position path of whatever frame the session is currently serving —
/// the next dispatch frame's path when one is queued, otherwise the active
/// path the ledger already recorded. Shared by iteration stamping, activity
/// classification, and loop-aggregate crediting so all three agree on what
/// "the current frame" means.
fn active_position_path(
    session: &ctx_traits_core::procedure::session::Session,
) -> &[ctx_traits_core::procedure::runtime::PathSegment] {
    session
        .next_frame
        .as_ref()
        .map_or(session.active_path.as_slice(), |frame| {
            frame.position_path.as_slice()
        })
}

/// The plan-side string tag for a runtime `ControlKind`, matching
/// `core::procedure::runtime::readiness`'s private `control_kind_name` (and
/// this file's own `planned_control_kind` for the plan-side enum). Kept
/// separate rather than shared because the two sides use different enums.
fn runtime_control_kind_name(
    kind: ctx_traits_core::procedure::runtime::ControlKind,
) -> &'static str {
    match kind {
        ctx_traits_core::procedure::runtime::ControlKind::Sequence => "sequence",
        ctx_traits_core::procedure::runtime::ControlKind::Branch => "branch",
        ctx_traits_core::procedure::runtime::ControlKind::Loop => "loop",
        ctx_traits_core::procedure::runtime::ControlKind::ForEach => "for-each",
        ctx_traits_core::procedure::runtime::ControlKind::Parallel => "parallel",
    }
}

/// Resolves enclosing loop identity/current iteration for a presentation
/// location, reproducing `path_for_nested_item` in
/// `core::procedure::runtime::readiness` exactly rather than approximating
/// it: every validated control segment (`loop`/`for-each`/`parallel`, any
/// control frame that structurally matches by kind+id) is stamped with THAT
/// frame's own `iteration_index`/`item_index` — core does this unconditionally
/// for every control-stack frame, not just loop/for-each, so a matched
/// `parallel` segment must carry its frame's iteration too. The trailing item
/// segment's iteration/item_index are resolved independently, exactly as core
/// does: walking backward from the innermost validated frame for the nearest
/// validated `Loop`/`Parallel` (iteration) and, separately, the nearest
/// validated `ForEach` (item_index). Neither search lets the other's kind
/// overwrite what it already found — a `Loop -> ForEach -> item` path keeps
/// the loop's iteration on the trailing item even though the for-each frame
/// (which sits closer) has no iteration of its own to contribute.
///
/// Each pure control segment at path index `i` (1..control_end) is
/// ordinal-mapped 1:1 to `session.control_stack[i - 1]` by construction (both
/// this file's `child_location` and core's `path_for_nested_item` push one
/// control segment per nesting level, in order) — depth alone identifies the
/// candidate frame. It's confirmed a genuine match, not a coincidentally
/// same-depth sibling from an inactive branch arm, only when that frame's
/// `kind`/`sequence_id` also match the segment's `kind`/`id`; a location
/// whose ancestors never validate this way is a stale/foreign path and is
/// deliberately left unstamped (iteration remains `None`) rather than
/// borrowing whatever loop happens to be live elsewhere.
///
/// Reused for both the iteration-aware status/branch-decision lookups and
/// for building this item's own (iteration-aware) presentation key, so prior
/// and current iterations of the same body position never collide.
fn stamp_live_iterations(
    session: &ctx_traits_core::procedure::session::Session,
    position_path: &[ctx_traits_core::procedure::runtime::PathSegment],
) -> Vec<ctx_traits_core::procedure::runtime::PathSegment> {
    stamp_control_stack_iterations(&session.control_stack, position_path)
}

pub(super) fn stamp_control_stack_iterations(
    control_stack: &[ctx_traits_core::procedure::runtime::ControlFrame],
    position_path: &[ctx_traits_core::procedure::runtime::PathSegment],
) -> Vec<ctx_traits_core::procedure::runtime::PathSegment> {
    let mut stamped = position_path.to_vec();
    if stamped.len() < 3 {
        return stamped;
    }
    let control_end = stamped.len() - 1;
    // Every validated control segment gets its OWN frame's iteration/item_index
    // stamped directly — no shared tracker, so one control kind can never
    // erase what a different-kind frame already established on its segment.
    let mut validated_frames = Vec::with_capacity(control_end - 1);
    for (depth, segment) in stamped[1..control_end].iter_mut().enumerate() {
        let Some(frame) = control_stack.get(depth) else {
            continue;
        };
        if runtime_control_kind_name(frame.kind.clone()) != segment.kind
            || Some(frame.sequence_id.as_str()) != segment.id.as_deref()
        {
            continue;
        }
        segment.iteration = frame.iteration_index;
        segment.item_index = frame.item_index;
        validated_frames.push(frame);
    }
    // The trailing item's iteration/item_index are resolved independently:
    // nearest validated Loop|Parallel for iteration, nearest validated
    // ForEach for item_index — mirroring core's two separate backward finds.
    let innermost_iteration = validated_frames
        .iter()
        .rev()
        .find(|frame| {
            matches!(
                frame.kind,
                ctx_traits_core::procedure::runtime::ControlKind::Loop
                    | ctx_traits_core::procedure::runtime::ControlKind::Parallel
            )
        })
        .and_then(|frame| frame.iteration_index);
    let innermost_item_index = validated_frames
        .iter()
        .rev()
        .find(|frame| frame.kind == ctx_traits_core::procedure::runtime::ControlKind::ForEach)
        .and_then(|frame| frame.item_index);
    stamped[control_end].iteration = innermost_iteration;
    stamped[control_end].item_index = innermost_item_index;
    stamped
}

/// Like `structural_path_matches`, but additionally requires the `iteration`/
/// `item_index` on `expected` (when present) to match `actual` — used to
/// stop a stale prior-iteration status from satisfying a current-iteration
/// lookup on loop rollover.
fn iteration_aware_path_matches(
    actual: &[ctx_traits_core::procedure::runtime::PathSegment],
    expected: &[ctx_traits_core::procedure::runtime::PathSegment],
) -> bool {
    actual.len() == expected.len()
        && actual.iter().zip(expected).all(|(actual, expected)| {
            actual.kind == expected.kind
                && actual.id == expected.id
                && actual.index == expected.index
                && (expected.iteration.is_none() || actual.iteration == expected.iteration)
                && (expected.item_index.is_none() || actual.item_index == expected.item_index)
        })
}

/// Presentation-only identity for a loop/for-each container's aggregate
/// elapsed/token totals: `loop:` plus the container's own structural
/// position path, encoding ONLY `kind`/`id`/`index` per segment — never
/// `iteration`/`item_index`. This is deliberately its own encoder rather than
/// a call into `structural_path_key` (which embeds live `iteration`/
/// `item_index` in its formatted string): `step_from_item` keys a container
/// from the plan-side path, where those fields are always `None`, while
/// `active_loop_container_keys` keys the same container from a live path
/// whose copied segments may carry a live iteration. Routing either through
/// `structural_path_key` would make the two callers' keys diverge on every
/// iteration past the first; this encoder is canonical and iteration-
/// independent by construction, so both sides always agree.
///
/// Using the full path — not just `run_index`/`item_id` — matters because
/// nested items inherit their root ancestor's `run_index`, and a local item
/// id is only unique within its own named sequence: a loop reused with the
/// same id under two different branch arms gets two different `id`s at the
/// ancestor "selected sequence" segment, so the paths (and therefore the
/// keys) stay distinct.
pub(super) fn loop_container_key(
    position_path: &[ctx_traits_core::procedure::runtime::PathSegment],
) -> String {
    let structural = position_path
        .iter()
        .map(|segment| {
            format!(
                "{}:{}:{}",
                segment.kind,
                segment.id.as_deref().unwrap_or(""),
                segment.index,
            )
        })
        .collect::<Vec<_>>()
        .join("/");
    format!("loop:{structural}")
}

/// The aggregate keys of every loop/for-each container presently open on the
/// session's control stack — i.e. the containers enclosing whatever item is
/// active right now. Used to fan an elapsed interval or a token credit out
/// to every enclosing loop at once.
///
/// Reconstructs each open container's own structural position path from
/// `active_position_path` + `control_stack` so it lines up with the same
/// container step's plan-side `location.position_path` (see
/// `step_from_item`'s `loop_key`): `active_position_path`'s segment at array
/// index `u` already carries that container's index within *its own*
/// parent (readiness's `path_for_nested_item` stamps a control segment's
/// `index` from the enclosing frame's `next_index`, i.e. "which child of
/// that frame's body is current" — which, at the frame the child itself
/// opens, is this container's own position). `u == 0` is the top-level case:
/// the procedure-anchor segment already *is* that container's full
/// identity, mirroring `PlannedItemLocation::root`.
pub(super) fn active_loop_container_keys(
    session: &ctx_traits_core::procedure::session::Session,
) -> BTreeSet<String> {
    let path = active_position_path(session);
    let mut keys = BTreeSet::new();
    for (depth, frame) in session.control_stack.iter().enumerate() {
        if !matches!(
            frame.kind,
            ctx_traits_core::procedure::runtime::ControlKind::Loop
                | ctx_traits_core::procedure::runtime::ControlKind::ForEach
        ) {
            continue;
        }
        let Some(own_segment) = path.get(depth) else {
            continue;
        };
        let container_path = if depth == 0 {
            vec![own_segment.clone()]
        } else {
            let mut container_path = path[..=depth].to_vec();
            container_path.push(ctx_traits_core::procedure::runtime::PathSegment {
                kind: "item".to_string(),
                id: frame.control_item_id.clone(),
                index: own_segment.index,
                iteration: None,
                item_index: None,
            });
            container_path
        };
        keys.insert(loop_container_key(&container_path));
    }
    keys
}

fn step_from_item(
    item: &ctx_traits_core::procedure::run::PlannedSequenceItem,
    location: &PlannedItemLocation,
    session: &ctx_traits_core::procedure::session::Session,
    harness_by_role: &BTreeMap<String, Vec<(Option<u32>, String)>>,
    accepted: &BTreeSet<String>,
    force_done: bool,
    live_drive: bool,
) -> RunStep {
    let stamped_path = stamp_live_iterations(session, &location.position_path);
    let runtime_status = session
        .ledger
        .sequence_statuses
        .iter()
        .rev()
        .find(|status| {
            status.run_index == item.run_index
                && if location.position_path.len() > 1 {
                    !status.position_path.is_empty()
                        && iteration_aware_path_matches(&status.position_path, &stamped_path)
                } else {
                    status.position_path.is_empty()
                }
        });
    let activity = item_activity(session, item, location);
    let active = activity == Activity::Current;
    let mut state = step_state(session, runtime_status, activity);
    let mut status_text = step_status_text(
        session,
        runtime_status,
        activity,
        item,
        location,
        live_drive,
    );
    if force_done {
        state = StepState::Done;
        status_text = "done".to_string();
    }
    let role = role_for_item(item, session, activity);
    let rows = harness_by_role.get(&role);
    // Every reachable declaration site — top-level or nested — carries its
    // own structural seat (see `PlannedSequenceItem::structural_seat`), so
    // this always selects the one harness actually bound to this row (P456).
    let harness = harness_for_seat(rows, item.structural_seat);
    let mut inputs = port_slugs(item.input_refs.iter().map(ToString::to_string), accepted);
    let mut outputs = port_slugs(item.output_refs.iter().map(ToString::to_string), accepted);
    if active && let Some(frame) = session.next_frame.as_deref() {
        extend_port_slugs(
            &mut inputs,
            frame
                .available_inputs
                .iter()
                .map(|input| input.ref_text.clone()),
            accepted,
        );
        extend_port_slugs(
            &mut outputs,
            frame
                .requested_outputs
                .iter()
                .map(|output| output.slot_ref.to_string()),
            accepted,
        );
    }
    let item_id = item.item_id.as_deref().unwrap_or("");
    let loop_key = is_loop_kind(&item.kind).then(|| loop_container_key(&location.position_path));
    RunStep {
        key: structural_step_key(item.run_index, item_id, &stamped_path, &role),
        label: item.title.clone(),
        role,
        harness,
        tags: step_tags(item, session, location),
        status: status_text,
        state,
        active,
        counts_progress: counts_progress(item),
        inputs,
        outputs,
        elapsed: None,
        output_tokens: None,
        loop_key,
        on_active_path: activity == Activity::Ancestor,
        position_path: stamped_path,
        run_index: item.run_index,
        structured_count: 0,
        summary: None,
        summary_at: None,
    }
}

pub(super) fn counts_progress(item: &ctx_traits_core::procedure::run::PlannedSequenceItem) -> bool {
    matches!(
        item.kind,
        ctx_traits_core::procedure::run::PlannedSequenceKind::Prompt
            | ctx_traits_core::procedure::run::PlannedSequenceKind::Command
    )
}

fn item_activity(
    session: &ctx_traits_core::procedure::session::Session,
    item: &ctx_traits_core::procedure::run::PlannedSequenceItem,
    location: &PlannedItemLocation,
) -> Activity {
    let active_path = active_position_path(session);
    let current = if location.position_path.len() == 1 && active_path.is_empty() {
        session.current_run_index == item.run_index
            && session.current_sequence_item_id == item.item_id
    } else {
        structural_path_matches(active_path, &location.position_path)
    };
    if current {
        return Activity::Current;
    }
    let on_active_path = is_control_item(item)
        && structural_control_ancestor_matches(&location.position_path, active_path);
    if on_active_path {
        Activity::Ancestor
    } else {
        Activity::Idle
    }
}

/// Progress text for a container mid-execution, from the control stack entry
/// it opened (1-based for display): `iteration k/max` for a loop, `item
/// k/total` for a for-each (total is the resolved list length, not the
/// structural `max-items` bound — the run iterates the list, so the list is
/// what progress is measured against).
fn container_progress_text(
    session: &ctx_traits_core::procedure::session::Session,
    item: &ctx_traits_core::procedure::run::PlannedSequenceItem,
    location: &PlannedItemLocation,
) -> Option<String> {
    let item_id = item.item_id.as_deref()?;
    let frame_index = location.position_path.len().saturating_sub(2);
    let frame = session.control_stack.get(frame_index)?;
    if frame.control_item_id.as_deref() != Some(item_id) || frame.parent_run_index != item.run_index
    {
        return None;
    }
    if frame.kind == ctx_traits_core::procedure::runtime::ControlKind::ForEach {
        let index = frame.item_index? + 1;
        return match frame.item_total {
            Some(total) => Some(format!("item {index}/{total}")),
            None => Some(format!("item {index}")),
        };
    }
    let iteration = frame.iteration_index? + 1;
    match frame.max_iterations {
        Some(max) => Some(format!("iteration {iteration}/{max}")),
        None => Some(format!("iteration {iteration}")),
    }
}

fn role_for_item(
    item: &ctx_traits_core::procedure::run::PlannedSequenceItem,
    session: &ctx_traits_core::procedure::session::Session,
    activity: Activity,
) -> String {
    // Only the exactly-current item wears the dispatched agent; ancestors keep
    // their own column so a loop header never impersonates its running child.
    if activity == Activity::Current
        && let Some(agent) = &session.current_agent
    {
        return agent.role.clone();
    }
    item.agent_ref
        .as_ref()
        .map(|agent| agent.id().to_string())
        .unwrap_or_else(|| ctx_traits_io::harness_config::DEFAULT_SEAT.to_string())
}

fn step_tags(
    item: &ctx_traits_core::procedure::run::PlannedSequenceItem,
    session: &ctx_traits_core::procedure::session::Session,
    location: &PlannedItemLocation,
) -> Vec<String> {
    let kind = match item.kind {
        ctx_traits_core::procedure::run::PlannedSequenceKind::Prompt => "prompt",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Ask => "ask",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Command => "command",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Check => "check",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Project => "project",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Sequence => "sequence",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Branch => "branch",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Loop => "loop",
        ctx_traits_core::procedure::run::PlannedSequenceKind::ForEach => "for-each",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Parallel => "parallel",
        ctx_traits_core::procedure::run::PlannedSequenceKind::Terminal => "terminal",
    };
    let mut tags = vec![kind.to_string()];
    if item.kind == ctx_traits_core::procedure::run::PlannedSequenceKind::Branch
        && let Some(decision) = branch_decision_for(session, item, location)
    {
        tags.push(format!("selected:{}", decision.selected_arm));
    }
    tags
}

fn step_state(
    session: &ctx_traits_core::procedure::session::Session,
    runtime_status: Option<&ctx_traits_core::procedure::runtime::SequenceStatus>,
    activity: Activity,
) -> StepState {
    if let Some(status) = runtime_status {
        match status.status {
            ctx_traits_core::procedure::runtime::SequenceStatusKind::Accepted => {
                return StepState::Done;
            }
            ctx_traits_core::procedure::runtime::SequenceStatusKind::Rejected => {
                return StepState::Failed;
            }
            ctx_traits_core::procedure::runtime::SequenceStatusKind::Blocked
                if activity == Activity::Idle =>
            {
                return StepState::Pending;
            }
            _ => {}
        }
    }
    if activity == Activity::Current
        && session.completion.is_none()
        && session.stop_reason.is_some()
    {
        // The item the run stopped at wears the failure mark, not a spinner.
        return StepState::Failed;
    }
    if activity != Activity::Idle && session.completion.is_none() {
        StepState::Running
    } else {
        StepState::Pending
    }
}

fn step_status_text(
    session: &ctx_traits_core::procedure::session::Session,
    runtime_status: Option<&ctx_traits_core::procedure::runtime::SequenceStatus>,
    activity: Activity,
    item: &ctx_traits_core::procedure::run::PlannedSequenceItem,
    location: &PlannedItemLocation,
    live_drive: bool,
) -> String {
    if session.completion.is_none() {
        match activity {
            Activity::Current => {
                // A structured stop names the real state; "blocked" alone
                // reads like a wait, not an ended run.
                if let Some(stop) = session.stop_reason.as_ref() {
                    return stop
                        .message
                        .as_deref()
                        .filter(|message| !message.trim().is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| stop.reason.clone());
                }
                // In the in-process drive, a command frame at the cursor is
                // executing right now — the derived Blocked* status names
                // the frame kind, not a wait for a permission grant.
                if live_drive
                    && item.kind == ctx_traits_core::procedure::run::PlannedSequenceKind::Command
                    && matches!(
                        session.status,
                        ctx_traits_core::procedure::session::Status::BlockedCommandPermissionRequired
                    )
                {
                    return "running".to_string();
                }
                return session_status(&session.status).to_string();
            }
            Activity::Ancestor => {
                return container_progress_text(session, item, location)
                    .unwrap_or_else(|| "running".to_string());
            }
            Activity::Idle => {}
        }
    }
    match runtime_status.map(|status| &status.status) {
        Some(ctx_traits_core::procedure::runtime::SequenceStatusKind::Accepted) => "done",
        Some(ctx_traits_core::procedure::runtime::SequenceStatusKind::Ready) => "ready",
        Some(ctx_traits_core::procedure::runtime::SequenceStatusKind::Blocked) => "blocked",
        Some(ctx_traits_core::procedure::runtime::SequenceStatusKind::Rejected) => "rejected",
        Some(ctx_traits_core::procedure::runtime::SequenceStatusKind::Routed) => "routed",
        Some(ctx_traits_core::procedure::runtime::SequenceStatusKind::Skipped) => "skipped",
        Some(ctx_traits_core::procedure::runtime::SequenceStatusKind::DependencyPending) => {
            "dependency-pending"
        }
        Some(ctx_traits_core::procedure::runtime::SequenceStatusKind::Pending) | None => "pending",
    }
    .to_string()
}

pub(super) fn status_tone(step: &RunStep) -> tui::Tone {
    match step.state {
        StepState::Done => tui::Tone::Pass,
        StepState::Failed => tui::Tone::Fail,
        StepState::Running => tui::Tone::Warn,
        StepState::Pending => tui::Tone::Muted,
    }
}

pub(super) fn step_key(step: &RunStep) -> String {
    step.key.clone()
}

#[allow(dead_code)] // Retained helper for presentation callers outside compact rows.
fn role_harness_text(step: &RunStep) -> String {
    match step.harness.as_deref() {
        Some(harness) => format!("{}·{}", step.role, harness),
        None => step.role.clone(),
    }
}

fn port_slugs(refs: impl Iterator<Item = String>, accepted: &BTreeSet<String>) -> Vec<PortSlug> {
    let mut slugs = Vec::new();
    extend_port_slugs(&mut slugs, refs, accepted);
    slugs
}

fn extend_port_slugs(
    slugs: &mut Vec<PortSlug>,
    refs: impl Iterator<Item = String>,
    accepted: &BTreeSet<String>,
) {
    let mut seen = slugs
        .iter()
        .map(|slug| slug.slug.clone())
        .collect::<BTreeSet<_>>();
    for ref_text in refs {
        let slug = ref_slug(&ref_text);
        if seen.insert(slug.clone()) {
            slugs.push(PortSlug {
                slug,
                satisfied: accepted.contains(&ref_text),
            });
        }
    }
}

pub(super) fn ref_slug(ref_text: &str) -> String {
    ctx_traits_core::reference::Reference::parse(ref_text)
        .map(|parsed| parsed.id().to_string())
        .unwrap_or_else(|_| {
            ref_text
                .split_once(':')
                .map_or(ref_text, |(_, id)| id)
                .to_string()
        })
}

/// Every configured seat's harness per role, in seat order: one `(None,
/// harness)` row for a legacy single-table role, or one `(Some(seat_index),
/// harness)` row per authored `[[agent.role.<role>]]` entry for a list-backed
/// role (P456). Kept as rows (not pre-joined into one display string) so a
/// planned step can select its OWN seat's exact harness
/// (`harness_for_seat`) instead of every row of a two-seat role showing the
/// same ambiguous `"seat1/seat2"` text, and so the header's harness count can
/// be computed from actual distinct harness ids across every row.
pub(super) fn harness_by_role(
    session: &ctx_traits_core::procedure::session::Session,
) -> BTreeMap<String, Vec<(Option<u32>, String)>> {
    let mut by_role: BTreeMap<String, Vec<(Option<u32>, String)>> = BTreeMap::new();
    for assignment in session
        .provenance
        .agent_assignments
        .as_ref()
        .into_iter()
        .flat_map(|assignments| assignments.iter())
    {
        by_role
            .entry(assignment.role.clone())
            .or_default()
            .push((assignment.seat_index, assignment.harness.clone()));
    }
    for rows in by_role.values_mut() {
        rows.sort_by_key(|(seat, _)| *seat);
    }
    by_role
}

/// The exact harness for `role`'s `structural_seat` (0-based, per
/// [`ctx_traits_core::procedure::runtime::AgentRole::structural_seat`]):
/// `entries[structural_seat % len]`, mirroring the same selection
/// `assignment_for_role`/`select_agent_assignment` apply at dispatch time.
/// A single legacy row (no seat info) is returned regardless of
/// `structural_seat`; `None` when the role has no configured rows at all or
/// `structural_seat` could not be determined for a list-backed role (an
/// unparseable/non-local agent ref — every reachable declared site resolves
/// a seat, see `PlannedSequenceItem::structural_seat`).
fn harness_for_seat(
    rows: Option<&Vec<(Option<u32>, String)>>,
    structural_seat: Option<u32>,
) -> Option<String> {
    let rows = rows?;
    match rows.len() {
        0 => None,
        1 if rows[0].0.is_none() => Some(rows[0].1.clone()),
        len => {
            let seat = structural_seat? as usize % len;
            rows.get(seat).map(|(_, harness)| harness.clone())
        }
    }
}

pub(super) fn accepted_refs(
    session: &ctx_traits_core::procedure::session::Session,
) -> BTreeSet<String> {
    session
        .accepted_port_values
        .iter()
        .chain(session.accepted_slot_values.iter())
        .chain(session.accepted_output_port_values.iter())
        .filter(|value| {
            value.acceptance == ctx_traits_core::procedure::runtime::AcceptanceStatus::Accepted
        })
        .map(|value| value.ref_text.clone())
        .collect()
}
