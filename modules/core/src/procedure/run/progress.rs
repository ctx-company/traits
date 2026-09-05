// Shared counted-frame enumeration: one traversal of a resolved `Plan`
// joined to a live `Session`, consumed by both the CLI's `run_view`
// projection and the desktop's `frame N of M` counter. Complete over
// `parallel_branches` children, selected branch arms only, loop/for-each
// bodies enumerated once (the traversal walks the plan tree, which
// contains each body once regardless of live `iteration_index`).
//
// `container_progress_text` (`iteration k/max`, `item k/total`) stays in
// the CLI — it is a presentation fact about a container's live iteration,
// never folded into the counted-frame ordinal or total here.

use crate::procedure::runtime::{
    BranchDecision, ControlFrame, ControlKind, PathSegment, SequenceStatus, SequenceStatusKind,
};
use crate::procedure::session::Session;

/// Durable classification of a planned frame — the semantics `run_view`'s
/// `StepState` carries, minus every presentation concern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlannedFrameState {
    Pending,
    Running,
    Done,
    Failed,
}

/// Where a planned item sits relative to the frame the session is currently
/// serving. Mirrors `run_view`'s `Activity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameActivity {
    Current,
    Ancestor,
    Idle,
}

pub struct PlannedFrame<'a> {
    pub item: &'a PlannedSequenceItem,
    /// Plan-side, iteration-free path.
    pub location_path: Vec<PathSegment>,
    /// The same path with live iteration/item stamps applied.
    pub stamped_path: Vec<PathSegment>,
    pub state: PlannedFrameState,
    pub counts_progress: bool,
    /// `Activity::Current`.
    pub active: bool,
    /// `Activity::Ancestor`.
    pub on_active_path: bool,
    pub force_done: bool,
}

/// N/M, or a typed absence naming its reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum RunProgress {
    /// Invariant: `1 <= ordinal <= total`.
    Reached { ordinal: usize, total: usize },
    NoneReached { total: usize },
    NoCountedFrames,
}

impl RunProgress {
    fn reached(ordinal: usize, total: usize) -> Self {
        if ordinal == 0 || ordinal > total {
            return Self::NoneReached { total };
        }
        Self::Reached { ordinal, total }
    }
}

#[derive(Clone)]
struct FrameLocation {
    position_path: Vec<PathSegment>,
}

impl FrameLocation {
    fn root(item: &PlannedSequenceItem) -> Self {
        Self {
            position_path: vec![PathSegment {
                kind: "procedure".to_string(),
                id: item.item_id.clone(),
                index: item.run_index,
                iteration: None,
                item_index: None,
            }],
        }
    }
}

fn child_location(
    parent_location: &FrameLocation,
    parent: &PlannedSequenceItem,
    otherwise: bool,
    child: &PlannedSequenceItem,
) -> FrameLocation {
    let sequence_id = if otherwise {
        parent.otherwise_sequence_ref.as_ref()
    } else {
        parent.sequence_ref.as_ref()
    }
    .map(|reference| reference.id().to_string());
    push_control_and_item(
        parent_location,
        planned_control_kind(parent.kind.clone()),
        sequence_id,
        child,
    )
}

fn parallel_child_location(
    parent_location: &FrameLocation,
    parent: &PlannedSequenceItem,
    sequence_id: &str,
    child: &PlannedSequenceItem,
) -> FrameLocation {
    push_control_and_item(
        parent_location,
        planned_control_kind(parent.kind.clone()),
        Some(sequence_id.to_string()),
        child,
    )
}

fn push_control_and_item(
    parent_location: &FrameLocation,
    control_kind: &'static str,
    sequence_id: Option<String>,
    child: &PlannedSequenceItem,
) -> FrameLocation {
    let mut position_path = parent_location.position_path.clone();
    if position_path
        .last()
        .is_some_and(|segment| segment.kind == "item")
    {
        position_path.pop();
    }
    position_path.push(PathSegment {
        kind: control_kind.to_string(),
        id: sequence_id,
        index: child.sequence_index,
        iteration: None,
        item_index: None,
    });
    position_path.push(PathSegment {
        kind: "item".to_string(),
        id: child.item_id.clone(),
        index: child.sequence_index,
        iteration: None,
        item_index: None,
    });
    FrameLocation { position_path }
}

fn planned_control_kind(kind: PlannedSequenceKind) -> &'static str {
    match kind {
        PlannedSequenceKind::Sequence => "sequence",
        PlannedSequenceKind::Branch => "branch",
        PlannedSequenceKind::Loop => "loop",
        PlannedSequenceKind::ForEach => "for-each",
        PlannedSequenceKind::Parallel => "parallel",
        PlannedSequenceKind::Prompt
        | PlannedSequenceKind::Ask
        | PlannedSequenceKind::Command
        | PlannedSequenceKind::Check
        | PlannedSequenceKind::Project => "",
        PlannedSequenceKind::Terminal => "terminal",
    }
}

fn is_loop_kind(kind: &PlannedSequenceKind) -> bool {
    matches!(kind, PlannedSequenceKind::Loop | PlannedSequenceKind::ForEach)
}

/// Counted-progress kinds: `Prompt`/`Command` only.
pub fn counts_progress(item: &PlannedSequenceItem) -> bool {
    matches!(item.kind, PlannedSequenceKind::Prompt | PlannedSequenceKind::Command)
}

fn structural_path_matches(actual: &[PathSegment], expected: &[PathSegment]) -> bool {
    actual.len() == expected.len()
        && actual.iter().zip(expected).all(|(actual, expected)| {
            actual.kind == expected.kind && actual.id == expected.id && actual.index == expected.index
        })
}

fn structural_control_ancestor_matches(expected: &[PathSegment], actual: &[PathSegment]) -> bool {
    let prefix = if expected.last().is_some_and(|segment| segment.kind == "item") {
        &expected[..expected.len().saturating_sub(1)]
    } else {
        expected
    };
    actual.len() > prefix.len()
        && actual.iter().zip(prefix).all(|(actual, expected)| {
            actual.kind == expected.kind && actual.id == expected.id && actual.index == expected.index
        })
}

fn is_control_item(item: &PlannedSequenceItem) -> bool {
    matches!(
        item.kind,
        PlannedSequenceKind::Sequence
            | PlannedSequenceKind::Branch
            | PlannedSequenceKind::Loop
            | PlannedSequenceKind::ForEach
            | PlannedSequenceKind::Parallel
    )
}

fn active_position_path(session: &Session) -> &[PathSegment] {
    session
        .next_frame
        .as_ref()
        .map_or(session.active_path.as_slice(), |frame| frame.position_path.as_slice())
}

fn runtime_control_kind_name(kind: ControlKind) -> &'static str {
    match kind {
        ControlKind::Sequence => "sequence",
        ControlKind::Branch => "branch",
        ControlKind::Loop => "loop",
        ControlKind::ForEach => "for-each",
        ControlKind::Parallel => "parallel",
    }
}

/// Stamps every validated control segment with its own control-stack
/// frame's `iteration_index`/`item_index`, and the trailing item segment
/// with the nearest validated `Loop`/`Parallel` (iteration) and nearest
/// validated `ForEach` (item_index) — mirroring
/// `core::procedure::runtime::readiness`'s `path_for_nested_item` exactly.
/// See `modules/cli/src/app/run_view/planned.rs`'s `stamp_control_stack_iterations`
/// doc comment (this is the same function, lifted verbatim) for the full
/// rationale.
fn stamp_control_stack_iterations(
    control_stack: &[ControlFrame],
    position_path: &[PathSegment],
) -> Vec<PathSegment> {
    let mut stamped = position_path.to_vec();
    if stamped.len() < 3 {
        return stamped;
    }
    let control_end = stamped.len() - 1;
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
    let innermost_iteration = validated_frames
        .iter()
        .rev()
        .find(|frame| matches!(frame.kind, ControlKind::Loop | ControlKind::Parallel))
        .and_then(|frame| frame.iteration_index);
    let innermost_item_index = validated_frames
        .iter()
        .rev()
        .find(|frame| frame.kind == ControlKind::ForEach)
        .and_then(|frame| frame.item_index);
    stamped[control_end].iteration = innermost_iteration;
    stamped[control_end].item_index = innermost_item_index;
    stamped
}

fn iteration_aware_path_matches(actual: &[PathSegment], expected: &[PathSegment]) -> bool {
    actual.len() == expected.len()
        && actual.iter().zip(expected).all(|(actual, expected)| {
            actual.kind == expected.kind
                && actual.id == expected.id
                && actual.index == expected.index
                && (expected.iteration.is_none() || actual.iteration == expected.iteration)
                && (expected.item_index.is_none() || actual.item_index == expected.item_index)
        })
}

fn branch_decision_for<'a>(
    session: &'a Session,
    item: &PlannedSequenceItem,
    location: &FrameLocation,
) -> Option<&'a BranchDecision> {
    let item_id = item.item_id.as_deref()?;
    let stamped_path = stamp_control_stack_iterations(&session.control_stack, &location.position_path);
    session.ledger.branch_decisions.iter().rev().find(|decision| {
        decision.parent_run_index == item.run_index
            && decision.branch_id == item_id
            && iteration_aware_path_matches(&decision.position_path, &stamped_path)
    })
}

fn item_activity(session: &Session, item: &PlannedSequenceItem, location: &FrameLocation) -> FrameActivity {
    let active_path = active_position_path(session);
    let current = if location.position_path.len() == 1 && active_path.is_empty() {
        session.current_run_index == item.run_index && session.current_sequence_item_id == item.item_id
    } else {
        structural_path_matches(active_path, &location.position_path)
    };
    if current {
        return FrameActivity::Current;
    }
    let on_active_path =
        is_control_item(item) && structural_control_ancestor_matches(&location.position_path, active_path);
    if on_active_path {
        FrameActivity::Ancestor
    } else {
        FrameActivity::Idle
    }
}

fn frame_state(
    session: &Session,
    runtime_status: Option<&SequenceStatus>,
    activity: FrameActivity,
) -> PlannedFrameState {
    if let Some(status) = runtime_status {
        match status.status {
            SequenceStatusKind::Accepted => return PlannedFrameState::Done,
            SequenceStatusKind::Rejected => return PlannedFrameState::Failed,
            SequenceStatusKind::Blocked if activity == FrameActivity::Idle => {
                return PlannedFrameState::Pending;
            }
            _ => {}
        }
    }
    if activity == FrameActivity::Current && session.completion.is_none() && session.stop_reason.is_some() {
        return PlannedFrameState::Failed;
    }
    if activity != FrameActivity::Idle && session.completion.is_none() {
        PlannedFrameState::Running
    } else {
        PlannedFrameState::Pending
    }
}

fn frame_from_item<'a>(
    item: &'a PlannedSequenceItem,
    location: &FrameLocation,
    session: &Session,
    force_done: bool,
) -> PlannedFrame<'a> {
    let stamped_path = stamp_control_stack_iterations(&session.control_stack, &location.position_path);
    let runtime_status = session.ledger.sequence_statuses.iter().rev().find(|status| {
        status.run_index == item.run_index
            && if location.position_path.len() > 1 {
                !status.position_path.is_empty()
                    && iteration_aware_path_matches(&status.position_path, &stamped_path)
            } else {
                status.position_path.is_empty()
            }
    });
    let activity = item_activity(session, item, location);
    let active = activity == FrameActivity::Current;
    let mut state = frame_state(session, runtime_status, activity);
    if force_done {
        state = PlannedFrameState::Done;
    }
    PlannedFrame {
        item,
        location_path: location.position_path.clone(),
        stamped_path,
        state,
        counts_progress: counts_progress(item),
        active,
        on_active_path: activity == FrameActivity::Ancestor,
        force_done,
    }
}

fn walk_item<'a>(
    item: &'a PlannedSequenceItem,
    location: &FrameLocation,
    session: &Session,
    force_done: bool,
    out: &mut Vec<PlannedFrame<'a>>,
) {
    let frame = frame_from_item(item, location, session, force_done);
    let child_force_done = force_done || (is_loop_kind(&item.kind) && frame.state == PlannedFrameState::Done);
    out.push(frame);
    let selected_arm = branch_decision_for(session, item, location).map(|decision| decision.selected_arm.as_str());
    let include_then = item.kind != PlannedSequenceKind::Branch || selected_arm == Some("then");
    let include_otherwise = selected_arm == Some("otherwise");
    for child in item.children.iter().filter(|_| include_then) {
        let child_location = child_location(location, item, false, child);
        walk_item(child, &child_location, session, child_force_done, out);
    }
    for child in item.otherwise_children.iter().filter(|_| include_otherwise) {
        let child_location = child_location(location, item, true, child);
        walk_item(child, &child_location, session, child_force_done, out);
    }
    for branch in &item.parallel_branches {
        for child in &branch.children {
            let child_location = parallel_child_location(location, item, branch.sequence_ref.id(), child);
            walk_item(child, &child_location, session, child_force_done, out);
        }
    }
}

/// Every reachable planned frame in structural order: selected branch arms
/// only, `parallel_branches` children included, loop/for-each bodies
/// enumerated once.
pub fn walk_planned_frames<'a>(plan: &'a Plan, session: &Session) -> Vec<PlannedFrame<'a>> {
    let mut frames = Vec::new();
    for item in &plan.sequence_items {
        walk_item(item, &FrameLocation::root(item), session, false, &mut frames);
    }
    frames
}

/// N/M for the given already-walked frames, or a typed absence naming its
/// reason. Never `Reached { ordinal: 0, .. }`.
pub fn run_progress(frames: &[PlannedFrame<'_>], session: &Session) -> RunProgress {
    let total = frames.iter().filter(|frame| frame.counts_progress).count();
    if total == 0 {
        return RunProgress::NoCountedFrames;
    }
    if session.completion.is_some() {
        return RunProgress::reached(total, total);
    }
    if let Some(current_index) = frames.iter().position(|frame| frame.active) {
        let ordinal = if frames[current_index].counts_progress {
            frames[..=current_index].iter().filter(|frame| frame.counts_progress).count()
        } else {
            frames[..current_index].iter().filter(|frame| frame.counts_progress).count()
        };
        return if ordinal == 0 {
            RunProgress::NoneReached { total }
        } else {
            RunProgress::reached(ordinal, total)
        };
    }
    // No frame is active (stale or foreign path): fall back to the durable
    // high-water mark rather than inventing a number the ledger doesn't
    // carry — the highest ordinal among counted frames that finished.
    let high_water = frames
        .iter()
        .filter(|frame| frame.counts_progress)
        .filter(|frame| matches!(frame.state, PlannedFrameState::Done | PlannedFrameState::Failed))
        .count();
    if high_water == 0 {
        RunProgress::NoneReached { total }
    } else {
        RunProgress::reached(high_water, total)
    }
}

/// Convenience for a caller that needs only the ratio, not the frame
/// enumeration itself (the desktop's `load()`).
pub fn run_progress_for(plan: &Plan, session: &Session) -> RunProgress {
    let frames = walk_planned_frames(plan, session);
    run_progress(&frames, session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::Digest;
    use crate::procedure::run::{
        AcceptanceState, Id, Plan, PlannedParallelBranch, SequenceItemStatus,
    };
    use crate::procedure::runtime::{
        EffectBuffer, FinalState, SequenceStatus, State, StopReason,
    };
    use crate::procedure::session::{
        CallerProvenance, CompletionNotification, Provenance, Session, SessionId, Status,
    };
    use crate::reference::Reference;

    fn item(
        title: &str,
        kind: PlannedSequenceKind,
        run_index: usize,
        sequence_index: usize,
    ) -> PlannedSequenceItem {
        PlannedSequenceItem {
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
            status: SequenceItemStatus::Planned,
        }
    }

    fn plan(items: Vec<PlannedSequenceItem>) -> Plan {
        Plan {
            run_id: Id::new("test-run").unwrap(),
            trait_id: "test".to_string(),
            sequence_items: items,
            slots: Vec::new(),
            producer_edges: Vec::new(),
            port_requirements: Vec::new(),
            output_ports: Vec::new(),
            session_title_sink: None,
            acceptance: AcceptanceState::Pending,
        }
    }

    fn base_session() -> Session {
        Session {
            schema_version: "1".to_string(),
            session_id: SessionId::new("session-test").unwrap(),
            run_id: Id::new("test-run").unwrap(),
            trait_id: "test".to_string(),
            source_digest: None,
            canonical_digest: None,
            current_run_index: 0,
            current_source_index: None,
            current_sequence_item_id: None,
            current_sequence_title: None,
            current_agent: None,
            status: Status::AwaitingAgentOutput,
            warnings: Vec::new(),
            accepted_port_values: Vec::new(),
            accepted_slot_values: Vec::new(),
            accepted_output_port_values: Vec::new(),
            slot_revisions: Vec::new(),
            emitted_signals: Vec::new(),
            rejected_submissions: Vec::new(),
            unresolved_inputs: Vec::new(),
            resource_evidence: Vec::new(),
            provider_capability_reports: Vec::new(),
            output_ports: Vec::new(),
            resolved_settings: Vec::new(),
            resolved_budgets: Vec::new(),
            active_path: Vec::new(),
            control_stack: Vec::new(),
            stop_reason: None,
            final_output_summary: Vec::new(),
            next_frame: None,
            last_validation_report: None,
            completion: None,
            last_drive_outcome: None,
            provenance: Provenance {
                started_by: CallerProvenance {
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
            ledger: State {
                run_id: Id::new("test-run").unwrap(),
                trait_id: "test".to_string(),
                strict_loops: false,
                source_digest: None,
                canonical_digest: None,
                current_run_index: 0,
                sequence_statuses: Vec::new(),
                accepted_port_values: Vec::new(),
                accepted_slot_values: Vec::new(),
                accepted_output_port_values: Vec::new(),
                slot_revisions: Vec::new(),
                resource_evidence: Vec::new(),
                emitted_signals: Vec::new(),
                rejected_attempts: Vec::new(),
                provider_capability_reports: Vec::new(),
                output_ports: Vec::new(),
                resolved_settings: Vec::new(),
                resolved_budgets: Vec::new(),
                active_path: Vec::new(),
                control_stack: Vec::new(),
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

    fn status(
        run_index: usize,
        position_path: Vec<PathSegment>,
        kind: SequenceStatusKind,
    ) -> SequenceStatus {
        SequenceStatus {
            sequence_index: run_index,
            run_index,
            item_id: None,
            title: "step".to_string(),
            status: kind,
            reason: String::new(),
            position_path,
        }
    }

    #[test]
    fn counts_progress_classifies_prompt_and_command_only() {
        let counting = [PlannedSequenceKind::Prompt, PlannedSequenceKind::Command];
        let non_counting = [
            PlannedSequenceKind::Ask,
            PlannedSequenceKind::Check,
            PlannedSequenceKind::Project,
            PlannedSequenceKind::Sequence,
            PlannedSequenceKind::Branch,
            PlannedSequenceKind::Loop,
            PlannedSequenceKind::ForEach,
            PlannedSequenceKind::Parallel,
            PlannedSequenceKind::Terminal,
        ];
        for kind in counting {
            assert!(counts_progress(&item("x", kind, 0, 0)));
        }
        for kind in non_counting {
            assert!(!counts_progress(&item("x", kind, 0, 0)));
        }
    }

    #[test]
    fn zero_counted_frames_is_the_typed_no_counted_frames_absence() {
        let plan = plan(vec![item("only-ask", PlannedSequenceKind::Ask, 0, 0)]);
        let session = base_session();
        let frames = walk_planned_frames(&plan, &session);
        assert_eq!(run_progress(&frames, &session), RunProgress::NoCountedFrames);
    }

    #[test]
    fn first_step_active_yields_ordinal_one() {
        let plan = plan(vec![
            item("a", PlannedSequenceKind::Prompt, 0, 0),
            item("b", PlannedSequenceKind::Prompt, 1, 1),
        ]);
        let mut session = base_session();
        session.current_run_index = 0;
        session.current_sequence_item_id = Some("a".to_string());
        let frames = walk_planned_frames(&plan, &session);
        assert_eq!(
            run_progress(&frames, &session),
            RunProgress::Reached { ordinal: 1, total: 2 }
        );
    }

    #[test]
    fn a_completed_run_reaches_the_full_total() {
        let plan = plan(vec![
            item("a", PlannedSequenceKind::Prompt, 0, 0),
            item("b", PlannedSequenceKind::Prompt, 1, 1),
        ]);
        let mut session = base_session();
        session.completion = Some(CompletionNotification {
            status: Status::Completed,
            event_code: "test".to_string(),
            final_outputs: Vec::new(),
            final_session_digest: Digest::source("test"),
        });
        let frames = walk_planned_frames(&plan, &session);
        assert_eq!(
            run_progress(&frames, &session),
            RunProgress::Reached { ordinal: 2, total: 2 }
        );
    }

    /// Correction-1 trap: a stopped/failed current counted frame reports its
    /// OWN ordinal, not one less — the `done` count (0 accepted here) must
    /// differ from `ordinal` (1, the frame it died on).
    #[test]
    fn a_stopped_current_frame_reports_its_own_ordinal_not_one_less() {
        let plan = plan(vec![item("a", PlannedSequenceKind::Prompt, 0, 0)]);
        let mut session = base_session();
        session.current_run_index = 0;
        session.current_sequence_item_id = Some("a".to_string());
        session.stop_reason = Some(StopReason {
            reason: "failed".to_string(),
            at: Vec::new(),
            last_check: None,
            message: None,
        });
        let frames = walk_planned_frames(&plan, &session);
        let done = frames
            .iter()
            .filter(|frame| frame.counts_progress && frame.state == PlannedFrameState::Done)
            .count();
        assert_eq!(done, 0);
        assert_eq!(
            run_progress(&frames, &session),
            RunProgress::Reached { ordinal: 1, total: 1 }
        );
    }

    /// Correction-2 load-bearing test: current position inside a
    /// `parallel_branches` child yields an ordinal — impossible before this
    /// cut, since `flatten_step` never walked branch children.
    #[test]
    fn current_position_inside_a_parallel_branch_child_yields_an_ordinal() {
        let branch_child = item("in-branch", PlannedSequenceKind::Prompt, 0, 0);
        let mut parallel_item = item("parallel", PlannedSequenceKind::Parallel, 0, 0);
        parallel_item.parallel_branches = vec![PlannedParallelBranch {
            sequence_ref: Reference::parse("sequence:branch-a").unwrap(),
            children: vec![branch_child],
        }];
        let plan = plan(vec![parallel_item]);
        let mut session = base_session();
        session.control_stack = vec![ControlFrame {
            kind: ControlKind::Parallel,
            parent_run_index: 0,
            control_item_id: Some("parallel".to_string()),
            sequence_id: "parallel".to_string(),
            next_index: 0,
            iteration_index: Some(0),
            max_iterations: None,
            unbounded: false,
            loop_started_elapsed_seconds: None,
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
            parallel_buffer: EffectBuffer::default(),
            parallel_committed_branches: Vec::new(),
            branch_decisions_watermark: 0,
            guard_evaluations_watermark: 0,
            join: None,
            branch_failure: Vec::new(),
            parallel_branch_refs: Vec::new(),
            parallel_branch_outcomes: Vec::new(),
        }];
        session.active_path = vec![
            PathSegment {
                kind: "procedure".to_string(),
                id: Some("parallel".to_string()),
                index: 0,
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
                id: Some("in-branch".to_string()),
                index: 0,
                iteration: None,
                item_index: None,
            },
        ];
        let frames = walk_planned_frames(&plan, &session);
        assert_eq!(
            run_progress(&frames, &session),
            RunProgress::Reached { ordinal: 1, total: 1 }
        );
    }

    #[test]
    fn no_active_frame_falls_back_to_the_durable_high_water_mark() {
        let plan = plan(vec![
            item("a", PlannedSequenceKind::Prompt, 0, 0),
            item("b", PlannedSequenceKind::Prompt, 1, 1),
        ]);
        let mut session = base_session();
        session.ledger.sequence_statuses = vec![status(0, Vec::new(), SequenceStatusKind::Accepted)];
        let frames = walk_planned_frames(&plan, &session);
        assert_eq!(
            run_progress(&frames, &session),
            RunProgress::Reached { ordinal: 1, total: 2 }
        );
    }

    #[test]
    fn ordinal_invariant_never_produces_zero_or_over_total() {
        let plan = plan(vec![item("a", PlannedSequenceKind::Prompt, 0, 0)]);
        let session = base_session();
        let frames = walk_planned_frames(&plan, &session);
        match run_progress(&frames, &session) {
            RunProgress::Reached { ordinal, total } => {
                assert!(ordinal >= 1 && ordinal <= total);
            }
            RunProgress::NoneReached { .. } | RunProgress::NoCountedFrames => {}
        }
    }
}
