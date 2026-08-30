//! gpui-free projection from a loaded [`Session`] plus its tolerant activity
//! sidecar into a hierarchical detail model. No gpui types, no `App` — the
//! pattern `dashboard.rs`/`run_row.rs`/`detail.rs` already document, so this
//! module is unit-testable without any UI machinery.
//!
//! # Where the tree comes from
//!
//! The desktop has a `Session` and nothing else, and must not resolve a
//! trait/plan to get one (that is repository-relative work this crate does
//! not do — see `detail.rs`'s own doc comment). The durable frame evidence
//! the session already carries is sufficient:
//! `session.ledger.sequence_statuses` — one entry per reached frame
//! position, in execution order — plus `session.active_path` /
//! `session.next_frame` to find the current frame.
//!
//! A top-level sequence item's `position_path` is empty; it is normalized to
//! a synthetic single-segment path (`kind = "procedure"`) so one algorithm
//! covers both the top-level and nested cases. `parallel` and `for-each`
//! control kinds are covered by fixture here, not by observation against a
//! real ledger (only `procedure`/`loop`/`branch`/`item` were sampled) — say
//! so rather than implying otherwise.
//!
//! Grouping keys drop a segment's `index` only where it is a changing child
//! cursor: an intermediate loop/branch/for-each control segment. `index` is
//! retained for a path's *terminal* segment (the segment that actually lands
//! a `SequenceStatus`) and for the root `procedure` seat (always the same
//! value for a given container, whether that container's own status entry
//! treats the segment as terminal or a child treats it as a prefix) — see
//! [`GroupKey::from_segment`]. Dropping `index` everywhere, as an earlier
//! version of this module did, collapsed distinct anonymous leaves (no
//! `id`) at different declaration positions into one node.
//!
//! # Frame state
//!
//! A **structural** group node (a loop-iteration group, a branch-arm group,
//! a for-each item group, or an orphaned container placeholder) has no
//! `SequenceStatus` of its own and never fabricates one — a loop container's
//! own `pending` status is rendered verbatim even while its children read
//! `accepted`; inventing an aggregate would duplicate product semantics the
//! task forbids. The current frame's state word instead comes from
//! `SessionState::derive`, the same function `run_row::row_state` already
//! calls, so `Running`/`WaitingOnHuman`/`WaitingOnAgent`/`Blocked`/`Failed`/
//! `Cancelled` are decided in exactly one place shared with the list.
//! `live` is refreshed from every `RowChanged`/`Appeared`/`Ended` delta while
//! a selection is following the center stream (`detail::RunDetail::follow`)
//! — no longer the point-in-time capture from `RunRow::live` at selection
//! time it started as; `project` still just takes the current value as a
//! parameter and has nothing more to do with how it advances.
//!
//! # Activity: strictly embellishment
//!
//! [`ActivityOverlay`] is built by folding the sidecar's tolerant read and
//! applied only after the tree is fully built from durable ledger evidence,
//! so it has no path that can add, remove, reorder, or restate a node. A
//! sidecar event's `frame_id` is iteration-blind (`drive.rs` stamps it from
//! `frame.item_id.unwrap_or(frame.title)`), so the latest matching event is
//! attached to the *current* frame only — attaching it to every historical
//! iteration of a looped item would fabricate evidence.

use std::collections::HashMap;

use ctx_traits_core::procedure::activity::{ActivityKind, SessionState};
use ctx_traits_core::procedure::runtime::{PathSegment, SequenceStatus, SequenceStatusKind};
use ctx_traits_core::procedure::session::Session;
use ctx_traits_io::activity_sidecar::ActivityRecord;
use ctx_traits_io::run_summary::RunSummary;

/// A frame's durable or derived state. `Structural` is reserved for
/// synthesized group nodes that carry no `SequenceStatus` of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameState {
    Pending,
    Ready,
    Done,
    Rejected,
    Blocked,
    Skipped,
    Routed,
    Structural,
}

impl FrameState {
    fn from_status_kind(kind: &SequenceStatusKind) -> Self {
        match kind {
            SequenceStatusKind::Accepted => Self::Done,
            SequenceStatusKind::Rejected => Self::Rejected,
            SequenceStatusKind::Blocked => Self::Blocked,
            SequenceStatusKind::Skipped => Self::Skipped,
            SequenceStatusKind::Routed => Self::Routed,
            SequenceStatusKind::Pending | SequenceStatusKind::DependencyPending => Self::Pending,
            SequenceStatusKind::Ready => Self::Ready,
        }
    }
}

/// One bounded activity line attached to the current frame.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityLine {
    pub kind: ActivityKind,
    pub text: Option<String>,
}

/// One node in the projected frame tree: either a leaf frame landed from a
/// `SequenceStatus`, a top-level container, or a synthesized structural
/// group (loop iteration / branch arm / for-each item / orphan placeholder).
#[derive(Debug, Clone, PartialEq)]
pub struct DetailNode {
    key: GroupKey,
    pub kind: String,
    pub id: Option<String>,
    pub title: String,
    /// 1-based iteration/item ordinal for a structural group node; `None`
    /// for a landed leaf or container.
    pub ordinal: Option<usize>,
    pub state: FrameState,
    pub reason: String,
    /// The node whose normalized path equals the session's current frame
    /// path. At most one node in a tree carries `current = true`.
    pub current: bool,
    /// Set only on the current node, from `SessionState::derive` — never
    /// invented locally.
    pub session_state: Option<SessionState>,
    pub activity: Option<ActivityLine>,
    pub narration: Option<String>,
    pub children: Vec<DetailNode>,
}

impl DetailNode {
    fn placeholder(segment: &PathSegment, retain_index: bool) -> Self {
        Self {
            key: GroupKey::from_segment(segment, retain_index),
            kind: segment.kind.clone(),
            id: segment.id.clone(),
            title: group_title(segment),
            ordinal: group_ordinal(segment),
            state: FrameState::Structural,
            reason: String::new(),
            current: false,
            session_state: None,
            activity: None,
            narration: None,
            children: Vec::new(),
        }
    }

    fn apply_status(&mut self, status: &SequenceStatus, segment: &PathSegment) {
        self.kind = segment.kind.clone();
        self.id = segment.id.clone().or_else(|| status.item_id.clone());
        if !status.title.is_empty() {
            self.title = status.title.clone();
        }
        self.reason = status.reason.clone();
        self.state = FrameState::from_status_kind(&status.status);
        // `ordinal` is reserved for synthesized structural groups (loop
        // iterations, for-each items). A landed `SequenceStatus` is a real
        // frame, not a group, even when its terminal segment happens to
        // carry an enclosing iteration/item index inherited by
        // `DetailNode::placeholder` — clear it so rendering never appends a
        // fabricated ordinal to a leaf title.
        self.ordinal = None;
    }

    fn frame_id(&self) -> Option<String> {
        if matches!(self.state, FrameState::Structural) {
            return None;
        }
        self.id.clone().or_else(|| Some(self.title.clone()))
    }
}

/// Grouping/lookup identity for one path segment. `index` is retained only
/// when `retain_index` is set by the caller — see the module doc comment for
/// which segments that is.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GroupKey {
    kind: String,
    id: Option<String>,
    iteration: Option<usize>,
    item_index: Option<usize>,
    index: Option<usize>,
}

impl GroupKey {
    /// `retain_index` must be `true` for a path's terminal segment (the one
    /// that lands a `SequenceStatus`) and for the root `procedure` seat,
    /// `false` for every other prefix segment (a loop/branch/for-each
    /// control cursor). A `procedure` segment is always terminal-or-root, so
    /// its own index is load-bearing identity, never a changing cursor.
    fn from_segment(segment: &PathSegment, retain_index: bool) -> Self {
        let retain_index = retain_index || segment.kind == "procedure";
        Self {
            kind: segment.kind.clone(),
            id: segment.id.clone(),
            iteration: segment.iteration,
            item_index: segment.item_index,
            index: retain_index.then_some(segment.index),
        }
    }
}

fn group_title(segment: &PathSegment) -> String {
    match segment.kind.as_str() {
        "loop" if segment.iteration.is_some() => {
            format!("Iteration {}", segment.iteration.unwrap() + 1)
        }
        "for-each" if segment.item_index.is_some() => {
            format!("Item {}", segment.item_index.unwrap() + 1)
        }
        "branch" => segment
            .id
            .clone()
            .unwrap_or_else(|| "branch arm".to_string()),
        _ => segment.id.clone().unwrap_or_else(|| segment.kind.clone()),
    }
}

fn group_ordinal(segment: &PathSegment) -> Option<usize> {
    segment
        .iteration
        .or(segment.item_index)
        .map(|value| value + 1)
}

/// A top-level entry's `position_path` is empty; give it the synthetic path
/// `PlannedItemLocation::root` produces on the CLI side, so every entry has
/// a non-empty path and one algorithm covers both cases.
fn normalize_path(status: &SequenceStatus) -> Vec<PathSegment> {
    if status.position_path.is_empty() {
        vec![PathSegment {
            kind: "procedure".to_string(),
            id: status.item_id.clone(),
            index: status.run_index,
            iteration: None,
            item_index: None,
        }]
    } else {
        status.position_path.clone()
    }
}

/// The position path of whatever frame the session is currently serving,
/// mirroring the CLI's `active_position_path` resolution order: the next
/// queued frame's path first, the ledger's own active path otherwise. An
/// empty `next_frame.position_path` (a top-level current frame) is
/// synthesized the same way a top-level status entry is.
fn current_position_path(session: &Session) -> Vec<PathSegment> {
    if let Some(frame) = &session.next_frame {
        if frame.position_path.is_empty() {
            return vec![PathSegment {
                kind: "procedure".to_string(),
                id: frame.item_id.clone(),
                index: frame.run_index.unwrap_or(0),
                iteration: None,
                item_index: None,
            }];
        }
        return frame.position_path.clone();
    }
    session.active_path.clone()
}

fn insert(
    nodes: &mut Vec<DetailNode>,
    path: &[PathSegment],
    status: &SequenceStatus,
    current: bool,
    session_state: Option<SessionState>,
) {
    let (segment, rest) = path
        .split_first()
        .expect("a normalized position path is never empty");
    let retain_index = rest.is_empty();
    let key = GroupKey::from_segment(segment, retain_index);
    let index = match nodes.iter().position(|node| node.key == key) {
        Some(index) => index,
        None => {
            nodes.push(DetailNode::placeholder(segment, retain_index));
            nodes.len() - 1
        }
    };
    if rest.is_empty() {
        nodes[index].apply_status(status, segment);
        if current {
            nodes[index].current = true;
            nodes[index].session_state = session_state;
        }
    } else {
        insert(
            &mut nodes[index].children,
            rest,
            status,
            current,
            session_state,
        );
    }
}

fn attach_overlay(nodes: &mut [DetailNode], overlay: &ActivityOverlay) {
    for node in nodes {
        if node.current
            && let Some(frame_id) = node.frame_id()
        {
            node.activity = overlay.activity_for(&frame_id);
            node.narration = overlay.narration_for(&frame_id);
        }
        attach_overlay(&mut node.children, overlay);
    }
}

/// The run-level facts shown above the frame tree. Every fact is reused
/// verbatim from `RunSummary::from_session`/`run_row`'s formatting helpers —
/// nothing here is re-derived.
#[derive(Debug, Clone, PartialEq)]
pub struct DetailHeader {
    pub title: String,
    pub task_value: Option<String>,
    pub elapsed_text: String,
    pub tokens_text: String,
    pub current_sequence_title: Option<String>,
    pub landing: Option<String>,
    pub stop_reason_text: Option<String>,
    pub next_frame_kind: Option<String>,
    pub run_state: SessionState,
}

/// `SequenceFrameKind`'s own `#[serde(rename_all = "kebab-case")]` wire
/// word, not `Debug` formatting baked into a displayed artifact — the same
/// discipline `run_summary.rs`'s `merge_status_str` follows for
/// `MergeStatus`. `serde_json` is a dev-only dependency of this crate, so
/// this is a plain match rather than a round trip through it.
fn next_frame_kind_str(
    kind: &ctx_traits_core::procedure::runtime::SequenceFrameKind,
) -> Option<String> {
    use ctx_traits_core::procedure::runtime::SequenceFrameKind;
    Some(
        match kind {
            SequenceFrameKind::Intro => "intro",
            SequenceFrameKind::Step => "step",
            SequenceFrameKind::Ask => "ask",
            SequenceFrameKind::Command => "command",
            SequenceFrameKind::Check => "check",
        }
        .to_string(),
    )
}

fn build_header(
    session: &Session,
    summary: &RunSummary,
    overlay: &ActivityOverlay,
    live: bool,
) -> DetailHeader {
    let outcome_kind = session
        .last_drive_outcome
        .as_ref()
        .map(|outcome| &outcome.outcome);
    DetailHeader {
        title: crate::run_row::summary_title(summary, overlay.session_title.as_deref()),
        task_value: summary.task_value.clone(),
        elapsed_text: crate::run_row::elapsed_text(summary.elapsed_seconds),
        tokens_text: crate::run_row::tokens_text(
            summary.work_tokens,
            summary.narrator_tokens,
            summary.guide_tokens,
        ),
        current_sequence_title: summary.current_sequence_title.clone(),
        landing: summary.landing.clone(),
        stop_reason_text: session.stop_reason.as_ref().map(|reason| {
            reason
                .message
                .clone()
                .unwrap_or_else(|| reason.reason.clone())
        }),
        next_frame_kind: summary
            .next_frame_kind
            .as_ref()
            .and_then(next_frame_kind_str),
        run_state: SessionState::derive(&session.status, outcome_kind, live),
    }
}

/// A hierarchical projection of a loaded session, ready for native
/// rendering. See the module doc comment for how it is built.
#[derive(Debug, Clone, PartialEq)]
pub struct DetailTree {
    pub header: DetailHeader,
    pub roots: Vec<DetailNode>,
}

/// Folded, bounded view of a session's `.activity.jsonl` sidecar. Built once
/// and applied after the durable tree is complete — see the module doc
/// comment. Only the last matching record per `frame_id` is retained, so a
/// long run's thousands of records never survive past this fold.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActivityOverlay {
    session_title: Option<String>,
    activity_by_frame: HashMap<String, ActivityLine>,
    narration_by_frame: HashMap<String, String>,
    /// The latest `at_epoch_ms` folded in so far. Used only by
    /// `apply_live_record` to reject a record older than what has already
    /// been folded — a live-stream boundary guard, not a durable-evidence
    /// concern, so `from_records`' historical fold never needs it.
    watermark_ms: u64,
}

impl ActivityOverlay {
    pub fn from_records(records: &[ActivityRecord]) -> Self {
        let mut overlay = Self::default();
        for record in records {
            overlay.apply_record(record);
        }
        overlay
    }

    /// `pub(crate)` rather than private: this is the exact fold seam
    /// `detail::load` uses to build the retained overlay once, and the seam
    /// `detail::follow` feeds every `CenterDelta::ActivityLine` into —
    /// never rebuilt on each projection.
    pub(crate) fn apply_record(&mut self, record: &ActivityRecord) {
        self.watermark_ms = self.watermark_ms.max(record.at_epoch_ms());
        match record {
            ActivityRecord::Activity { event, .. } => {
                self.activity_by_frame.insert(
                    event.frame_id.clone(),
                    ActivityLine {
                        kind: event.kind.clone(),
                        text: event.text.clone(),
                    },
                );
            }
            ActivityRecord::Narration { frame_id, text, .. } => {
                self.narration_by_frame
                    .insert(frame_id.clone(), text.clone());
            }
            ActivityRecord::SessionTitle { title, .. } => {
                self.session_title = Some(title.clone());
            }
            // Step summaries need the CLI's private structural step key to
            // attach to a node; deliberately not reused here (see the task's
            // scope decision) and so not attached to anything below.
            ActivityRecord::StepSummary { .. } => {}
        }
    }

    /// Fold one record that arrived live, rejecting it if it is strictly
    /// older than the watermark: a `pending` record replayed from before the
    /// seed's sidecar read must never roll a newer, already-folded line back
    /// to a superseded one. Equal timestamps are kept — millisecond-
    /// resolution collisions are idempotent under `apply_record`'s
    /// last-write-wins fold. Wall-clock is the stamp source, so a backwards
    /// clock jump can drop one derived line; tolerable for derived evidence.
    /// Returns whether the overlay actually changed.
    pub(crate) fn apply_live_record(&mut self, record: &ActivityRecord) -> bool {
        if record.at_epoch_ms() < self.watermark_ms {
            return false;
        }
        self.apply_record(record);
        true
    }

    fn activity_for(&self, frame_id: &str) -> Option<ActivityLine> {
        self.activity_by_frame.get(frame_id).cloned()
    }

    fn narration_for(&self, frame_id: &str) -> Option<String> {
        self.narration_by_frame.get(frame_id).cloned()
    }
}

/// Project `session` (plus its already-folded, bounded activity overlay)
/// into a [`DetailTree`]. `live` is the row's kernel-backed liveness flag, as
/// currently tracked by the caller's selection (`detail::RunDetail` keeps it
/// refreshed from every `RowChanged`/`Appeared`/`Ended` delta, not just its
/// value at selection time) — this function itself just takes whatever value
/// it is handed. The overlay is folded once by the caller (`detail::load`)
/// — this function never re-folds a raw sidecar, so it stays cheap to call
/// on every render regardless of a long run's sidecar size.
pub fn project(session: &Session, overlay: &ActivityOverlay, live: bool) -> DetailTree {
    let summary = RunSummary::from_session(session);
    let header = build_header(session, &summary, overlay, live);

    let current_path = current_position_path(session);
    let outcome_kind = session
        .last_drive_outcome
        .as_ref()
        .map(|outcome| &outcome.outcome);
    let current_session_state = SessionState::derive(&session.status, outcome_kind, live);

    let mut roots: Vec<DetailNode> = Vec::new();
    for status in &session.ledger.sequence_statuses {
        let path = normalize_path(status);
        let is_current = !current_path.is_empty() && path == current_path;
        insert(
            &mut roots,
            &path,
            status,
            is_current,
            is_current.then_some(current_session_state),
        );
    }
    attach_overlay(&mut roots, overlay);

    DetailTree { header, roots }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_core::procedure::activity::ActivityEvent;

    fn session_from(value: serde_json::Value) -> Session {
        serde_json::from_value(value).expect("fixture session")
    }

    fn base_session_json(sequence_statuses: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "schema-version": "0.1.0",
            "session-id": "session-fixture",
            "run-id": "run-fixture",
            "trait-id": "fixture-trait",
            "current-run-index": 0,
            "status": "completed",
            "provenance": {
                "started-by": {"surface": "test", "caller": "detail-tree-fixture"},
                "state-source": "test",
            },
            "ledger": {
                "run-id": "run-fixture",
                "trait-id": "fixture-trait",
                "current-run-index": 0,
                "final-state": "completed",
                "sequence-statuses": sequence_statuses,
            },
            "state-digest": "sha256:fixture",
        })
    }

    fn status(
        run_index: usize,
        item_id: &str,
        title: &str,
        status: &str,
        position_path: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({
            "sequence-index": run_index,
            "run-index": run_index,
            "item-id": item_id,
            "title": title,
            "status": status,
            "reason": "",
            "position-path": position_path,
        })
    }

    fn loop_body_segment(iteration: usize, body_index: usize) -> serde_json::Value {
        serde_json::json!([
            {"kind": "procedure", "id": "the-loop", "index": 0},
            {
                "kind": "loop",
                "id": "the-loop-body",
                "index": body_index,
                "iteration": iteration,
            },
        ])
    }

    #[test]
    fn nesting_groups_loop_iterations_and_body_items() {
        let mut statuses = vec![status(
            0,
            "the-loop",
            "The loop",
            "pending",
            serde_json::json!([]),
        )];
        for iteration in 0..3usize {
            for body_index in 0..2usize {
                let mut path = loop_body_segment(iteration, body_index)
                    .as_array()
                    .unwrap()
                    .clone();
                path.push(serde_json::json!({
                    "kind": "item",
                    "id": format!("item-{body_index}"),
                    "index": body_index,
                    "iteration": iteration,
                }));
                statuses.push(status(
                    1 + iteration * 2 + body_index,
                    &format!("item-{body_index}"),
                    &format!("Item {body_index}"),
                    "accepted",
                    serde_json::Value::Array(path),
                ));
            }
        }
        let session = session_from(base_session_json(serde_json::Value::Array(statuses)));
        let tree = project(&session, &ActivityOverlay::default(), false);

        assert_eq!(tree.roots.len(), 1, "one top-level container");
        let container = &tree.roots[0];
        assert_eq!(container.state, FrameState::Pending);
        assert_eq!(container.children.len(), 3, "three iteration groups");
        for (index, iteration_group) in container.children.iter().enumerate() {
            assert_eq!(iteration_group.state, FrameState::Structural);
            assert_eq!(iteration_group.ordinal, Some(index + 1));
            assert_eq!(
                iteration_group.children.len(),
                2,
                "two body items per iteration"
            );
            for item in &iteration_group.children {
                assert_eq!(item.state, FrameState::Done);
            }
        }
    }

    #[test]
    fn branch_arm_nests_inside_the_correct_iteration_group() {
        let branch_path = serde_json::json!([
            {"kind": "procedure", "id": "container", "index": 4},
            {"kind": "loop", "id": "container-body", "index": 7, "iteration": 1},
            {"kind": "branch", "id": "arm-a", "index": 0},
            {"kind": "item", "id": "owner-approval-gate", "index": 0, "iteration": 1},
        ]);
        let sibling_path = serde_json::json!([
            {"kind": "procedure", "id": "container", "index": 4},
            {"kind": "loop", "id": "container-body", "index": 2, "iteration": 1},
            {"kind": "item", "id": "sibling-item", "index": 0, "iteration": 1},
        ]);
        let statuses = serde_json::Value::Array(vec![
            status(
                4,
                "container",
                "Container",
                "pending",
                serde_json::json!([]),
            ),
            status(5, "sibling-item", "Sibling", "accepted", sibling_path),
            status(
                6,
                "owner-approval-gate",
                "Owner gate",
                "blocked",
                branch_path,
            ),
        ]);
        let session = session_from(base_session_json(statuses));
        let tree = project(&session, &ActivityOverlay::default(), false);

        let container = &tree.roots[0];
        assert_eq!(container.children.len(), 1, "one iteration group");
        let iteration_group = &container.children[0];
        assert_eq!(
            iteration_group.ordinal,
            Some(2),
            "1-based iteration ordinal"
        );
        assert_eq!(
            iteration_group.children.len(),
            2,
            "sibling item + branch arm group"
        );
        let branch_group = iteration_group
            .children
            .iter()
            .find(|node| node.kind == "branch")
            .expect("branch arm group present");
        assert_eq!(branch_group.state, FrameState::Structural);
        assert_eq!(branch_group.children.len(), 1);
        assert_eq!(branch_group.children[0].state, FrameState::Blocked);
    }

    #[test]
    fn no_fabricated_rollup_container_keeps_its_own_pending_state() {
        let statuses = serde_json::Value::Array(vec![
            status(0, "the-loop", "The loop", "pending", serde_json::json!([])),
            status(1, "child", "Child", "accepted", loop_body_child_path()),
        ]);
        let session = session_from(base_session_json(statuses));
        let tree = project(&session, &ActivityOverlay::default(), false);
        assert_eq!(tree.roots[0].state, FrameState::Pending);
    }

    #[test]
    fn anonymous_leaf_positions_remain_distinct() {
        let anonymous_status =
            |run_index: usize, status_kind: &str, position_path: serde_json::Value| {
                serde_json::json!({
                    "sequence-index": run_index,
                    "run-index": run_index,
                    "item-id": null,
                    "title": "",
                    "status": status_kind,
                    "reason": "",
                    "position-path": position_path,
                })
            };
        let anonymous_item_in_iteration = |body_index: usize| {
            serde_json::json!([
                {"kind": "procedure", "id": "the-loop", "index": 0},
                {"kind": "loop", "id": "the-loop-body", "index": body_index, "iteration": 0},
                {"kind": "item", "id": null, "index": body_index, "iteration": 0},
            ])
        };
        let statuses = serde_json::Value::Array(vec![
            // Two anonymous top-level positions, distinct run indices.
            anonymous_status(0, "accepted", serde_json::json!([])),
            anonymous_status(1, "rejected", serde_json::json!([])),
            // Two anonymous nested leaves in the same loop iteration.
            anonymous_status(2, "accepted", anonymous_item_in_iteration(0)),
            anonymous_status(3, "rejected", anonymous_item_in_iteration(1)),
        ]);
        let session = session_from(base_session_json(statuses));
        let tree = project(&session, &ActivityOverlay::default(), false);

        assert_eq!(
            tree.roots.len(),
            3,
            "two distinct anonymous top-level nodes plus the loop container"
        );
        let top_level_states: Vec<_> = tree
            .roots
            .iter()
            .filter(|node| node.id.is_none())
            .map(|node| node.state)
            .collect();
        assert_eq!(
            top_level_states,
            vec![FrameState::Done, FrameState::Rejected],
            "distinct anonymous top-level positions must not overwrite each other"
        );

        let container = tree
            .roots
            .iter()
            .find(|node| node.id.as_deref() == Some("the-loop"))
            .expect("loop container present");
        assert_eq!(container.children.len(), 1, "one iteration group");
        let iteration_group = &container.children[0];
        assert_eq!(
            iteration_group.children.len(),
            2,
            "two distinct anonymous leaves in the same iteration"
        );
        let leaf_states: Vec<_> = iteration_group
            .children
            .iter()
            .map(|node| node.state)
            .collect();
        assert_eq!(leaf_states, vec![FrameState::Done, FrameState::Rejected]);
    }

    #[test]
    fn landed_leaf_does_not_inherit_structural_ordinal() {
        let statuses = serde_json::Value::Array(vec![
            status(0, "the-loop", "The loop", "pending", serde_json::json!([])),
            status(
                1,
                "item-0",
                "First item",
                "accepted",
                loop_body_segment(0, 0)
                    .as_array()
                    .unwrap()
                    .iter()
                    .cloned()
                    .chain(std::iter::once(serde_json::json!({
                        "kind": "item",
                        "id": "item-0",
                        "index": 0,
                        "iteration": 0,
                    })))
                    .collect::<Vec<_>>()
                    .into(),
            ),
        ]);
        let session = session_from(base_session_json(statuses));
        let tree = project(&session, &ActivityOverlay::default(), false);

        let container = &tree.roots[0];
        let iteration_group = &container.children[0];
        assert_eq!(
            iteration_group.ordinal,
            Some(1),
            "the structural iteration group keeps its ordinal"
        );
        let leaf = &iteration_group.children[0];
        assert_eq!(
            leaf.ordinal, None,
            "a landed leaf must not inherit the enclosing iteration's ordinal"
        );

        let for_each_statuses = serde_json::Value::Array(vec![
            status(
                0,
                "the-for-each",
                "The for-each",
                "pending",
                serde_json::json!([]),
            ),
            status(
                1,
                "item-0",
                "First item",
                "accepted",
                serde_json::json!([
                    {"kind": "procedure", "id": "the-for-each", "index": 0},
                    {
                        "kind": "for-each",
                        "id": "the-for-each-body",
                        "index": 0,
                        "item-index": 0,
                    },
                    {
                        "kind": "item",
                        "id": "item-0",
                        "index": 0,
                        "item-index": 0,
                    },
                ]),
            ),
        ]);
        let for_each_session = session_from(base_session_json(for_each_statuses));
        let for_each_tree = project(&for_each_session, &ActivityOverlay::default(), false);

        let for_each_container = &for_each_tree.roots[0];
        let item_group = &for_each_container.children[0];
        assert_eq!(
            item_group.ordinal,
            Some(1),
            "the structural for-each item group keeps its ordinal"
        );
        let for_each_leaf = &item_group.children[0];
        assert_eq!(
            for_each_leaf.ordinal, None,
            "a landed leaf must not inherit the enclosing for-each item's ordinal"
        );
    }

    fn loop_body_child_path() -> serde_json::Value {
        serde_json::json!([
            {"kind": "procedure", "id": "the-loop", "index": 0},
            {"kind": "loop", "id": "the-loop-body", "index": 0, "iteration": 0},
            {"kind": "item", "id": "child", "index": 0, "iteration": 0},
        ])
    }

    #[test]
    fn for_each_and_parallel_paths_group_by_runtime_ordinals() {
        // for-each: the control segment's `item_index` is the changing
        // runtime cursor, distinct from a `loop`'s `iteration`. Two body
        // items land in the same for-each item group even though the
        // *intermediate* control segment's own `index` field differs per
        // child (mirroring the loop-body case), while two different
        // for-each items (different `item_index`) must stay distinct.
        let for_each_leaf =
            |for_each_control_index: usize, item_index: usize, child_index: usize| {
                serde_json::json!([
                    {"kind": "procedure", "id": "the-for-each", "index": 0},
                    {
                        "kind": "for-each",
                        "id": "the-for-each-body",
                        "index": for_each_control_index,
                        "item-index": item_index,
                    },
                    {
                        "kind": "item",
                        "id": format!("child-{child_index}"),
                        "index": child_index,
                        "item-index": item_index,
                    },
                ])
            };
        let for_each_statuses = serde_json::Value::Array(vec![
            status(
                0,
                "the-for-each",
                "The for-each",
                "pending",
                serde_json::json!([]),
            ),
            status(1, "child-0", "Child 0", "accepted", for_each_leaf(0, 0, 0)),
            status(2, "child-1", "Child 1", "accepted", for_each_leaf(1, 0, 1)),
            status(
                3,
                "child-0",
                "Child 0 again",
                "accepted",
                for_each_leaf(2, 1, 0),
            ),
        ]);
        let for_each_session = session_from(base_session_json(for_each_statuses));
        let for_each_tree = project(&for_each_session, &ActivityOverlay::default(), false);
        let for_each_container = &for_each_tree.roots[0];
        assert_eq!(
            for_each_container.children.len(),
            2,
            "two distinct for-each items despite three differing control-segment indices"
        );
        assert_eq!(for_each_container.children[0].ordinal, Some(1));
        assert_eq!(for_each_container.children[0].children.len(), 2);
        assert_eq!(for_each_container.children[1].ordinal, Some(2));
        assert_eq!(for_each_container.children[1].children.len(), 1);

        // parallel: the control segment's `iteration` field is the runtime
        // ordinal, exactly like `loop`, so the same grouping rule applies.
        let parallel_leaf =
            |parallel_control_index: usize, iteration: usize, child_index: usize| {
                serde_json::json!([
                    {"kind": "procedure", "id": "the-parallel", "index": 0},
                    {
                        "kind": "parallel",
                        "id": "the-parallel-body",
                        "index": parallel_control_index,
                        "iteration": iteration,
                    },
                    {
                        "kind": "item",
                        "id": format!("branch-{child_index}"),
                        "index": child_index,
                        "iteration": iteration,
                    },
                ])
            };
        let parallel_statuses = serde_json::Value::Array(vec![
            status(
                0,
                "the-parallel",
                "The parallel",
                "pending",
                serde_json::json!([]),
            ),
            status(
                1,
                "branch-0",
                "Branch 0",
                "accepted",
                parallel_leaf(0, 0, 0),
            ),
            status(
                2,
                "branch-1",
                "Branch 1",
                "accepted",
                parallel_leaf(1, 0, 1),
            ),
            status(
                3,
                "branch-0",
                "Branch 0, second iteration",
                "accepted",
                parallel_leaf(2, 1, 0),
            ),
        ]);
        let parallel_session = session_from(base_session_json(parallel_statuses));
        let parallel_tree = project(&parallel_session, &ActivityOverlay::default(), false);
        let parallel_container = &parallel_tree.roots[0];
        assert_eq!(
            parallel_container.children.len(),
            2,
            "two distinct parallel-iteration groups for two distinct iteration values"
        );
        assert_eq!(
            parallel_container.children[0].ordinal,
            Some(1),
            "differing intermediate control-segment indices still consolidate within one iteration"
        );
        assert_eq!(parallel_container.children[0].children.len(), 2);
        assert_eq!(parallel_container.children[1].ordinal, Some(2));
        assert_eq!(parallel_container.children[1].children.len(), 1);
    }

    #[test]
    fn activity_overlay_last_record_wins() {
        let mut overlay = ActivityOverlay::default();
        overlay.apply_record(&ActivityRecord::SessionTitle {
            at_epoch_ms: 1,
            title: "first title".to_string(),
        });
        overlay.apply_record(&ActivityRecord::Activity {
            at_epoch_ms: 2,
            event: ActivityEvent {
                sequence: 1,
                frame_id: "the-frame".to_string(),
                kind: ActivityKind::Thinking,
                text: Some("first activity".to_string()),
                tool: None,
                tokens: None,
                rate_limit: None,
            },
        });
        overlay.apply_record(&ActivityRecord::Narration {
            at_epoch_ms: 3,
            frame_id: "the-frame".to_string(),
            text: "first narration".to_string(),
        });
        overlay.apply_record(&ActivityRecord::SessionTitle {
            at_epoch_ms: 4,
            title: "second title".to_string(),
        });
        overlay.apply_record(&ActivityRecord::Activity {
            at_epoch_ms: 5,
            event: ActivityEvent {
                sequence: 2,
                frame_id: "the-frame".to_string(),
                kind: ActivityKind::RunningTool,
                text: Some("second activity".to_string()),
                tool: Some("edit".to_string()),
                tokens: None,
                rate_limit: None,
            },
        });
        overlay.apply_record(&ActivityRecord::Narration {
            at_epoch_ms: 6,
            frame_id: "the-frame".to_string(),
            text: "second narration".to_string(),
        });

        assert_eq!(overlay.session_title.as_deref(), Some("second title"));
        let activity = overlay.activity_for("the-frame").expect("activity");
        assert_eq!(activity.kind, ActivityKind::RunningTool);
        assert_eq!(activity.text.as_deref(), Some("second activity"));
        assert_eq!(
            overlay.narration_for("the-frame").as_deref(),
            Some("second narration")
        );
    }

    #[test]
    fn apply_live_record_rejects_a_record_older_than_the_watermark() {
        let mut overlay = ActivityOverlay::default();
        overlay.apply_record(&ActivityRecord::Narration {
            at_epoch_ms: 10,
            frame_id: "the-frame".to_string(),
            text: "newer".to_string(),
        });
        let changed = overlay.apply_live_record(&ActivityRecord::Narration {
            at_epoch_ms: 5,
            frame_id: "the-frame".to_string(),
            text: "stale".to_string(),
        });
        assert!(!changed, "an older record must be rejected, not applied");
        assert_eq!(
            overlay.narration_for("the-frame").as_deref(),
            Some("newer"),
            "the watermark must protect the already-folded newer line"
        );

        let changed = overlay.apply_live_record(&ActivityRecord::Narration {
            at_epoch_ms: 10,
            frame_id: "the-frame".to_string(),
            text: "equal-timestamp".to_string(),
        });
        assert!(changed, "an equal timestamp is kept, not rejected");
        assert_eq!(
            overlay.narration_for("the-frame").as_deref(),
            Some("equal-timestamp")
        );
    }

    #[test]
    fn orphan_nested_entry_synthesizes_its_container() {
        let statuses = serde_json::Value::Array(vec![status(
            1,
            "child",
            "Child",
            "accepted",
            loop_body_child_path(),
        )]);
        let session = session_from(base_session_json(statuses));
        let tree = project(&session, &ActivityOverlay::default(), false);

        assert_eq!(
            tree.roots.len(),
            1,
            "container synthesized from the prefix alone"
        );
        let container = &tree.roots[0];
        assert_eq!(container.state, FrameState::Structural);
        assert_eq!(container.children.len(), 1);
        assert_eq!(container.children[0].children[0].state, FrameState::Done);
    }

    #[test]
    fn empty_sequence_statuses_yields_an_empty_tree_not_a_panic() {
        let session = session_from(base_session_json(serde_json::json!([])));
        let tree = project(&session, &ActivityOverlay::default(), false);
        assert!(tree.roots.is_empty());
    }

    #[test]
    fn current_frame_state_word_comes_from_shared_session_state_derivation() {
        let mut json = base_session_json(serde_json::Value::Array(vec![status(
            0,
            "ready-item",
            "Ready item",
            "ready",
            serde_json::json!([]),
        )]));
        json["status"] = serde_json::json!("awaiting-agent-output");
        json["active-path"] =
            serde_json::json!([{"kind": "procedure", "id": "ready-item", "index": 0}]);
        json["ledger"]["final-state"] = serde_json::json!("running");

        let live_session = session_from(json.clone());
        let live_tree = project(&live_session, &ActivityOverlay::default(), true);
        let live_node = &live_tree.roots[0];
        assert!(live_node.current);
        assert_eq!(live_node.session_state, Some(SessionState::Running));

        let waiting_session = session_from(json);
        let waiting_tree = project(&waiting_session, &ActivityOverlay::default(), false);
        let waiting_node = &waiting_tree.roots[0];
        assert!(waiting_node.current);
        assert_eq!(
            waiting_node.session_state,
            Some(SessionState::WaitingOnAgent)
        );
    }

    #[test]
    fn activity_and_narration_attach_only_to_the_current_node() {
        let mut json = base_session_json(serde_json::Value::Array(vec![
            status(
                0,
                "past-item",
                "Past item",
                "accepted",
                serde_json::json!([]),
            ),
            status(
                1,
                "current-item",
                "Current item",
                "ready",
                serde_json::json!([]),
            ),
        ]));
        json["active-path"] =
            serde_json::json!([{"kind": "procedure", "id": "current-item", "index": 1}]);
        let session = session_from(json);

        let records = vec![
            ActivityRecord::Activity {
                at_epoch_ms: 1,
                event: ActivityEvent {
                    sequence: 1,
                    frame_id: "current-item".to_string(),
                    kind: ActivityKind::RunningTool,
                    text: Some("editing".to_string()),
                    tool: Some("edit".to_string()),
                    tokens: None,
                    rate_limit: None,
                },
            },
            ActivityRecord::Narration {
                at_epoch_ms: 2,
                frame_id: "current-item".to_string(),
                text: "on it".to_string(),
            },
            ActivityRecord::Activity {
                at_epoch_ms: 3,
                event: ActivityEvent {
                    sequence: 2,
                    frame_id: "past-item".to_string(),
                    kind: ActivityKind::Thinking,
                    text: None,
                    tool: None,
                    tokens: None,
                    rate_limit: None,
                },
            },
        ];

        let tree = project(&session, &ActivityOverlay::from_records(&records), false);
        let past = tree
            .roots
            .iter()
            .find(|n| n.id.as_deref() == Some("past-item"))
            .unwrap();
        let current = tree
            .roots
            .iter()
            .find(|n| n.id.as_deref() == Some("current-item"))
            .unwrap();
        assert!(
            past.activity.is_none(),
            "a stale frame_id never attaches to a non-current node"
        );
        assert!(current.current);
        assert_eq!(
            current.activity.as_ref().unwrap().text.as_deref(),
            Some("editing")
        );
        assert_eq!(current.narration.as_deref(), Some("on it"));
    }

    #[test]
    fn activity_tolerance_leaves_the_tree_structurally_and_state_wise_equal() {
        let session = session_from(base_session_json(serde_json::Value::Array(vec![status(
            0,
            "solo-item",
            "Solo item",
            "accepted",
            serde_json::json!([]),
        )])));
        let empty = project(&session, &ActivityOverlay::default(), false);
        let with_activity = project(
            &session,
            &ActivityOverlay::from_records(&[ActivityRecord::Activity {
                at_epoch_ms: 1,
                event: ActivityEvent {
                    sequence: 1,
                    frame_id: "no-such-frame".to_string(),
                    kind: ActivityKind::Thinking,
                    text: None,
                    tool: None,
                    tokens: None,
                    rate_limit: None,
                },
            }]),
            false,
        );
        assert_eq!(empty.roots, with_activity.roots);
        assert_eq!(empty.header.title, with_activity.header.title);
    }

    #[test]
    fn header_reuses_run_summary_and_never_re_derives_it() {
        let session = session_from(base_session_json(serde_json::json!([])));
        let tree = project(&session, &ActivityOverlay::default(), false);
        assert_eq!(tree.header.title, "fixture-trait");
        assert_eq!(tree.header.elapsed_text, "00:00:00");
        assert_eq!(tree.header.tokens_text, "-");
    }

    #[test]
    fn header_run_state_covers_completed_blocked_and_failed() {
        let cases = [
            ("completed", "completed", SessionState::Completed),
            ("blocked", "blocked", SessionState::Blocked),
            ("failed", "failed", SessionState::Failed),
        ];
        for (status_wire, final_state_wire, expected) in cases {
            let mut json = base_session_json(serde_json::json!([]));
            json["status"] = serde_json::json!(status_wire);
            json["ledger"]["final-state"] = serde_json::json!(final_state_wire);
            let session = session_from(json);
            let tree = project(&session, &ActivityOverlay::default(), false);
            assert_eq!(
                tree.header.run_state, expected,
                "status {status_wire:?} must derive {expected:?}"
            );
        }
    }

    #[test]
    fn skipped_frame_kind_maps_to_frame_state_skipped() {
        let session = session_from(base_session_json(serde_json::Value::Array(vec![status(
            0,
            "skipped-item",
            "Skipped item",
            "skipped",
            serde_json::json!([]),
        )])));
        let tree = project(&session, &ActivityOverlay::default(), false);
        assert_eq!(tree.roots[0].state, FrameState::Skipped);
    }

    #[test]
    fn session_title_overlay_is_used_only_when_the_authoritative_title_is_absent() {
        let session = session_from(base_session_json(serde_json::json!([])));

        let overlay = ActivityOverlay::from_records(&[ActivityRecord::SessionTitle {
            at_epoch_ms: 1,
            title: "sidecar title".to_string(),
        }]);
        let tree = project(&session, &overlay, false);
        assert_eq!(
            tree.header.title, "sidecar title",
            "the sidecar title fills in when the summary carries none"
        );

        let mut json = base_session_json(serde_json::json!([]));
        json["provenance"]["session-title"] = serde_json::json!({
            "state": "resolved",
            "attempts": 1,
            "title": "authoritative title",
            "source": "narrator-default",
        });
        let session_with_title = session_from(json);
        let tree_with_authoritative_title = project(&session_with_title, &overlay, false);
        assert_eq!(
            tree_with_authoritative_title.header.title, "authoritative title",
            "an authoritative summary title always wins over the sidecar fallback"
        );
    }
}
