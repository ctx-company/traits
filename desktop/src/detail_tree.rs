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

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use ctx_traits_core::procedure::activity::{ActivityEvent, FrameSpans, SessionState};
use ctx_traits_core::procedure::runtime::{self, PathSegment, SequenceStatus, SequenceStatusKind};
use ctx_traits_core::procedure::session::Session;
use ctx_traits_io::activity_sidecar::ActivityRecord;
use ctx_traits_io::run_summary::RunSummary;

use crate::tokens;

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
    /// Retained `Activity` events for this node's `frame_id`, newest first,
    /// capped at `tokens::ACTIVITY_LINE_CAP`. Whole events (not a trimmed
    /// line) are retained: an `ActivityEvent` carries `tool`, so a
    /// `RunningTool` line can render its tool label instead of its raw JSON
    /// `text`. See [`ActivityOverlay::activity_lines_for`] for the
    /// ambiguity rule that decides which entries a looped item's node sees.
    pub activity_lines: Vec<ActivityEvent>,
    /// One-based round(s) for this node's own position path, outermost
    /// first. Empty unless this node's path passes through at least one
    /// `loop` control segment. Populated wholesale, after the tree is fully
    /// built, by [`apply_narration_resolutions`] from
    /// `runtime::resolve_narration_markers`'s result — never computed
    /// per-node here, so this file never calls the core round/label
    /// functions directly.
    pub loop_rounds: Vec<usize>,
    /// `Some(label)` exactly when this node is the innermost loop-control
    /// boundary of its lineage — see
    /// `ctx_traits_core::procedure::runtime::NarrationResolution::marker`
    /// for the exact rule. Populated the same way as `loop_rounds`, by
    /// [`apply_narration_resolutions`], so a consumer
    /// (`frame_list::push_node`) never has to test this node's or a child's
    /// `kind`, or infer boundaries from structural adjacency, to find where
    /// a narration line belongs.
    pub narration_marker: Option<String>,
    pub narration: Option<String>,
    /// First→last durable `Activity` span for this node's `frame_id`, only
    /// when that id occurs exactly once in the whole reconstructed tree —
    /// see [`FrameSpans::span`]. `None` for a `Structural` group (no
    /// `frame_id`) and for any ambiguous or evidence-free frame.
    pub span: Option<Duration>,
    /// This node's own consumed position-path prefix, threaded through by
    /// [`insert`] and consulted only by [`project`] to build the
    /// `NarrationCandidate` tree `runtime::resolve_narration_markers`
    /// resolves — never read to test a `kind` or infer a boundary itself.
    own_path: Vec<PathSegment>,
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
            activity_lines: Vec::new(),
            loop_rounds: Vec::new(),
            narration_marker: None,
            narration: None,
            span: None,
            own_path: Vec::new(),
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
pub(crate) fn current_position_path(session: &Session) -> Vec<PathSegment> {
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
    consumed: &mut Vec<PathSegment>,
) {
    let (segment, rest) = path
        .split_first()
        .expect("a normalized position path is never empty");
    consumed.push(segment.clone());
    let retain_index = rest.is_empty();
    let key = GroupKey::from_segment(segment, retain_index);
    let index = match nodes.iter().position(|node| node.key == key) {
        Some(index) => index,
        None => {
            nodes.push(DetailNode::placeholder(segment, retain_index));
            nodes.len() - 1
        }
    };
    nodes[index].own_path = consumed.clone();
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
            consumed,
        );
    }
    consumed.pop();
}

fn attach_overlay(
    nodes: &mut [DetailNode],
    overlay: &ActivityOverlay,
    counts: &HashMap<String, usize>,
) {
    for node in nodes {
        if node.current
            && let Some(frame_id) = node.frame_id()
        {
            let executions = counts.get(&frame_id).copied().unwrap_or(0);
            node.activity_lines = overlay.activity_lines_for(&frame_id, executions);
            node.narration = overlay.narration_for(&frame_id);
        }
        attach_overlay(&mut node.children, overlay, counts);
    }
}

/// One pass over the whole reconstructed tree, counting how many nodes
/// resolve to each `frame_id` — ambiguity (an id shared by more than one
/// node, e.g. two executions of the same looped item) is a property of the
/// reconstruction, not of the sidecar, so it must be counted globally before
/// any span is assigned.
fn count_frame_ids(nodes: &[DetailNode], counts: &mut HashMap<String, usize>) {
    for node in nodes {
        if let Some(frame_id) = node.frame_id() {
            *counts.entry(frame_id).or_insert(0) += 1;
        }
        count_frame_ids(&node.children, counts);
    }
}

/// Builds the `runtime::NarrationCandidate` shadow of a `DetailNode` tree —
/// just its own consumed `position_path` and its children's shadows, the
/// only shape `runtime::resolve_narration_markers` needs. No `kind` is read
/// here; the core function is the one place a `kind` is tested against
/// `"loop"`.
fn narration_candidates(nodes: &[DetailNode]) -> Vec<runtime::NarrationCandidate> {
    nodes
        .iter()
        .map(|node| runtime::NarrationCandidate {
            position_path: node.own_path.clone(),
            children: narration_candidates(&node.children),
        })
        .collect()
}

/// Populates [`DetailNode::loop_rounds`] and [`DetailNode::narration_marker`]
/// wholesale from `runtime::resolve_narration_markers`'s result — the shared
/// projection [`FrameRow`]'s narration placement is built from, so nothing in
/// this file (or downstream, in `frame_list::push_node`) tests a node's or a
/// child's `kind`, or infers a marker boundary from structural adjacency;
/// both live in exactly one place, the core function itself. `nodes` and
/// `resolutions` are the same shape by construction — both walked from
/// [`narration_candidates`]'s output.
///
/// [`FrameRow`]: crate::frame_list::FrameRow
fn apply_narration_resolutions(
    nodes: &mut [DetailNode],
    resolutions: Vec<runtime::NarrationResolution>,
) {
    for (node, resolution) in nodes.iter_mut().zip(resolutions) {
        node.loop_rounds = resolution.rounds;
        node.narration_marker = resolution.marker;
        apply_narration_resolutions(&mut node.children, resolution.children);
    }
}

fn assign_spans(
    nodes: &mut [DetailNode],
    overlay: &ActivityOverlay,
    counts: &HashMap<String, usize>,
) {
    for node in nodes {
        if let Some(frame_id) = node.frame_id() {
            let executions = counts.get(&frame_id).copied().unwrap_or(0);
            node.span = overlay.span_for(&frame_id, executions);
        }
        assign_spans(&mut node.children, overlay, counts);
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
    /// `RunSummary.verdict_rounds`, carried through unchanged — the frame
    /// list's current-row right side reuses this rather than re-deriving it
    /// from the session a second time.
    pub verdict_rounds: Option<u64>,
    /// `Session.current_agent.role`, carried through unchanged.
    pub current_agent_role: Option<String>,
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
        verdict_rounds: summary.verdict_rounds,
        current_agent_role: session
            .current_agent
            .as_ref()
            .map(|agent| agent.role.clone()),
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
/// comment. Up to `tokens::ACTIVITY_LINE_CAP` `Activity` records per
/// `frame_id` are retained (newest evicts oldest), so a long run's thousands
/// of records never survive past this fold.
#[derive(Debug, Clone, PartialEq)]
struct RetainedActivity {
    at_epoch_ms: u64,
    event: ActivityEvent,
    /// `false` for a record folded by the seed's historical read
    /// (`apply_record`); `true` for one folded live (`apply_live_record`/
    /// `apply_pending_record`). Consulted only by
    /// [`ActivityOverlay::activity_lines_for`]'s ambiguity rule: a delta
    /// arriving on the open subscription is by construction the currently
    /// executing frame's, while a seed-folded record sharing the same
    /// iteration-blind `frame_id` cannot be attributed to it.
    live: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActivityOverlay {
    session_title: Option<String>,
    /// Retained `Activity` events per frame id, in arrival order (oldest at
    /// the front), capped at `tokens::ACTIVITY_LINE_CAP` — never a sort on
    /// `ActivityEvent.sequence` (coalesced records persist sequence 0).
    lines_by_frame: HashMap<String, VecDeque<RetainedActivity>>,
    narration_by_frame: HashMap<String, String>,
    /// First/last `Activity` stamp per frame id — durable-only, folded from
    /// the same records `activity_by_frame` is, never from a process-local
    /// clock. See [`FrameSpans`].
    spans: FrameSpans,
    /// Bounded multiset of the seed's own `Activity` records — one
    /// `(at_epoch_ms, ActivityEvent)` entry per record — folded by this
    /// overlay's *historical* fold (`from_records`, i.e. `detail::load`'s
    /// seed read) and never advanced by `apply_live_record`. Consulted only
    /// by `consume_seed_activity_occurrence`, the seam
    /// `detail::RunDetail::apply` uses to tell a pending record that
    /// redelivers one of the seed's own records (a seed/pending race) from a
    /// genuinely new one, by matching the record's *full* content — not just
    /// `(frame_id, at_epoch_ms)`, which two distinct durable records can
    /// share (review-verdict-1 blocker `live-span-reopen-divergence`), and
    /// not `ActivityEvent::sequence` either (see [`FrameSpans::observe`] for
    /// why sequence is not a durable identity). Bounded by the number of
    /// `Activity` records the seed's own tolerant read contained — the same
    /// order of magnitude as the sidecar itself — and discarded wholesale by
    /// [`Self::clear_seed_occurrences`] once `detail::RunDetail::apply` has
    /// drained the pending queue against it, so the accepted overlay never
    /// carries this index past one reconciliation.
    activity_occurrences: HashMap<String, Vec<(u64, ActivityEvent)>>,
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
        if let Some((at_epoch_ms, event)) = record.as_activity() {
            self.spans.observe(&event.frame_id, at_epoch_ms);
            self.activity_occurrences
                .entry(event.frame_id.clone())
                .or_default()
                .push((at_epoch_ms, event.clone()));
        }
        self.apply_presentation(record, false);
    }

    /// Consumes one occurrence of `(at_epoch_ms, event)` from this overlay's
    /// own historical fold, if it folded that exact `Activity` record —
    /// reporting whether it did. Bounded record-occurrence reconciliation by
    /// full record content, not timestamp-endpoint matching and not a
    /// `(frame_id, at_epoch_ms)` key alone: two distinct durable records for
    /// one frame can share a millisecond stamp (review-verdict-1 blocker
    /// `live-span-reopen-divergence`), so matching on the pair alone would
    /// let a genuinely new record be mistaken for a replay of an unrelated
    /// one. A redelivered *interior* record (one that is neither the stored
    /// first nor last stamp once `FrameSpans` has collapsed the history to a
    /// pair) is still correctly recognized as "already accounted for",
    /// because this checks against every record the seed actually folded,
    /// not just the two stamps it retains. Each occurrence is consumable
    /// once, so a pending queue that (pathologically) redelivers the same
    /// record twice only reconciles the first replay; the second is treated
    /// as genuinely new, which is the safe direction (fold once more rather
    /// than silently drop evidence).
    pub(crate) fn consume_seed_activity_occurrence(
        &mut self,
        frame_id: &str,
        at_epoch_ms: u64,
        event: &ActivityEvent,
    ) -> bool {
        let Some(occurrences) = self.activity_occurrences.get_mut(frame_id) else {
            return false;
        };
        let Some(position) = occurrences
            .iter()
            .position(|(stamp, seeded_event)| *stamp == at_epoch_ms && seeded_event == event)
        else {
            return false;
        };
        occurrences.remove(position);
        true
    }

    /// Discards this overlay's seed-occurrence reconciliation index wholesale
    /// — called once by `detail::RunDetail::apply` immediately after it has
    /// drained the pending queue against it via
    /// [`Self::apply_pending_record`]. The accepted, retained overlay must
    /// carry only per-frame folded state (`spans`/`activity_by_frame`/
    /// `narration_by_frame`), never a resident index sized to the seed's
    /// historical record count (review-verdict-1 blocker
    /// `live-span-reopen-divergence`).
    pub(crate) fn clear_seed_occurrences(&mut self) {
        self.activity_occurrences.clear();
    }

    /// Test-only: whether the seed-occurrence reconciliation index is
    /// currently empty, so `detail`'s test module can assert
    /// [`Self::clear_seed_occurrences`] actually ran without exposing the
    /// index's shape.
    #[cfg(test)]
    pub(crate) fn seed_occurrences_is_empty(&self) -> bool {
        self.activity_occurrences.is_empty()
    }

    /// Folds one record captured into `Selection::pending` while a seed or
    /// resync read was in flight, reconciling a possible seed/pending
    /// overlap first via [`Self::consume_seed_activity_occurrence`]: when
    /// `record` is an `Activity` whose exact `(frame_id, at_epoch_ms)` stamp
    /// this overlay's own seed read already folded, that occurrence is
    /// consumed and the span is left untouched (the seed's fold already
    /// accounted for it) — this is what
    /// `RunDetail::apply` must call instead of [`Self::apply_live_record`]
    /// for its pending drain, since `FrameSpans::observe` itself is a plain,
    /// identity-free append-order fold with no way to recognize a replay of
    /// an interior record on its own (review-verdict-1 blocker
    /// `live-span-reopen-divergence`). A record with no matching occurrence
    /// is genuinely new and folds through the ordinary span path.
    /// A replayed `Activity` occurrence never re-enters `apply_presentation`:
    /// the retained deque is an ordered, bounded history rather than a
    /// last-write-wins slot, so appending it again would duplicate a visible
    /// line instead of idempotently overwriting one. Its `live` provenance is
    /// promoted in place on the still-retained entry instead (see the branch
    /// above). `Narration`/`SessionTitle` records have no such deque and
    /// remain genuinely last-write-wins, folded through the same
    /// watermark-gated `apply_presentation` path `apply_live_record` uses.
    /// Returns whether the overlay actually changed.
    pub(crate) fn apply_pending_record(&mut self, record: &ActivityRecord) -> bool {
        let mut changed = false;
        let mut is_replay = false;
        if let Some((at_epoch_ms, event)) = record.as_activity() {
            if self.consume_seed_activity_occurrence(&event.frame_id, at_epoch_ms, event) {
                is_replay = true;
                // The seed already retained this exact occurrence as a line;
                // promote its provenance to `live` in place rather than
                // appending a duplicate. A window eviction (the cap) may have
                // already dropped the entry — nothing to promote then, and
                // nothing to append either, since the visible window still
                // reflects at most one line per occurrence.
                if let Some(queue) = self.lines_by_frame.get_mut(&event.frame_id)
                    && let Some(entry) = queue
                        .iter_mut()
                        .find(|r| !r.live && r.at_epoch_ms == at_epoch_ms && r.event == *event)
                {
                    entry.live = true;
                    changed = true;
                }
            } else {
                changed |= self.spans.observe(&event.frame_id, at_epoch_ms);
            }
        }
        if record.at_epoch_ms() < self.watermark_ms {
            return changed;
        }
        self.watermark_ms = self.watermark_ms.max(record.at_epoch_ms());
        if is_replay {
            // A replayed seed occurrence already has its line (promoted
            // above, if still retained); `apply_presentation`'s other
            // branches (narration/session title) never fire for `Activity`
            // records, so there is nothing left to fold for this case.
            return changed;
        }
        self.apply_presentation(record, true);
        true
    }

    /// The presentation-only half of [`Self::apply_record`] — everything
    /// except span currency, which [`Self::apply_live_record`] folds through
    /// a separate, per-frame-monotonic path (see its doc comment). `live`
    /// tags the retained event with its provenance — see [`RetainedActivity`].
    fn apply_presentation(&mut self, record: &ActivityRecord, live: bool) {
        if let Some((at_epoch_ms, event)) = record.as_activity() {
            let queue = self
                .lines_by_frame
                .entry(event.frame_id.clone())
                .or_default();
            queue.push_back(RetainedActivity {
                at_epoch_ms,
                event: event.clone(),
                live,
            });
            while queue.len() > tokens::ACTIVITY_LINE_CAP {
                queue.pop_front();
            }
            return;
        }
        match record {
            ActivityRecord::Activity { .. } => {
                unreachable!("Activity records are routed above via ActivityRecord::as_activity")
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
            ActivityRecord::CommandAttemptStarted { .. }
            | ActivityRecord::CommandAttemptEnded { .. }
            | ActivityRecord::Verdict { .. } => {}
        }
    }

    /// Fold one record that arrived live. Presentation fields
    /// (`activity_by_frame`/`narration_by_frame`/`session_title`) are
    /// rejected wholesale when the record is strictly older than the
    /// overlay's global watermark: a `pending` record replayed from before
    /// the seed's sidecar read must never roll a newer, already-folded line
    /// back to a superseded one. Equal timestamps are kept — for
    /// `Narration`/`SessionTitle` that is an idempotent last-write-wins
    /// overwrite; for `Activity` the retained deque instead appends a bounded
    /// history entry per call, which is why *this* path (ordinary live
    /// deltas, never a seed/pending replay) is safe to call unconditionally,
    /// while [`Self::apply_pending_record`] must reconcile a replay first.
    ///
    /// Span currency (`FrameSpans`) is folded through `FrameSpans::observe`,
    /// unconditionally and never gated on the presentation watermark: the
    /// watermark advances on every record type (a title, a narration line),
    /// so gating an `Activity`'s span on it would let an unrelated
    /// later-stamped record suppress a genuinely new, not-yet-folded
    /// `Activity` for a different frame — exactly the ordering a
    /// cross-writer stream does not guarantee. This is the same call
    /// `apply_record` makes, so a live fold and a historical fold of the
    /// same, non-overlapping records always agree.
    ///
    /// This method is for the *ordinary* live-delta path
    /// (`detail::RunDetail::follow_delta`'s already-`Loaded` case), where
    /// every record is genuinely new evidence, never a redelivery of
    /// something already folded. It deliberately does **not** reconcile a
    /// seed/pending overlap (the same durable record folded once by a seed's
    /// historical read and once more as a replayed live delta while that
    /// read was in flight) — `FrameSpans::observe` is a plain, sequence-free
    /// append-order fold with no identity of its own (see its doc comment),
    /// so that reconciliation belongs at `detail::RunDetail::apply`'s pending
    /// drain, via [`Self::apply_pending_record`], where the seed's full set
    /// of folded stamps is still available to match against by bounded
    /// occurrence (review-verdict-1 blocker `live-span-reopen-divergence`).
    ///
    /// Wall-clock is the stamp source for presentation freshness, so a
    /// backwards clock jump can drop one derived line; tolerable for derived
    /// evidence. Returns whether the overlay actually changed.
    pub(crate) fn apply_live_record(&mut self, record: &ActivityRecord) -> bool {
        let mut changed = false;
        if let Some((at_epoch_ms, event)) = record.as_activity() {
            changed |= self.spans.observe(&event.frame_id, at_epoch_ms);
        }
        if record.at_epoch_ms() < self.watermark_ms {
            return changed;
        }
        self.watermark_ms = self.watermark_ms.max(record.at_epoch_ms());
        self.apply_presentation(record, true);
        true
    }

    /// Newest-first, at most `tokens::ACTIVITY_LINE_CAP` events for
    /// `frame_id`. Mirrors [`Self::span_for`]'s ambiguity posture: when
    /// `executions == 1` the whole retained window is shown; when the id
    /// resolves to more than one node (a looped item), only the entries
    /// folded live are — a seed-folded record with the same iteration-blind
    /// `frame_id` cannot be attributed to whichever occurrence is currently
    /// selected as `current`.
    fn activity_lines_for(&self, frame_id: &str, executions: usize) -> Vec<ActivityEvent> {
        let Some(queue) = self.lines_by_frame.get(frame_id) else {
            return Vec::new();
        };
        let ambiguous = executions > 1;
        queue
            .iter()
            .filter(|retained| !ambiguous || retained.live)
            .rev()
            .map(|retained| retained.event.clone())
            .collect()
    }

    fn narration_for(&self, frame_id: &str) -> Option<String> {
        self.narration_by_frame.get(frame_id).cloned()
    }

    /// Delegates to [`FrameSpans::span`] — `None` unless `frame_id` occurs
    /// exactly once in the caller's reconstructed tree and its stamps
    /// actually advance.
    fn span_for(&self, frame_id: &str, executions: usize) -> Option<Duration> {
        self.spans.span(frame_id, executions)
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
            &mut Vec::new(),
        );
    }

    let mut frame_id_counts = HashMap::new();
    count_frame_ids(&roots, &mut frame_id_counts);
    // Executions must be counted globally before the overlay is attached: the
    // ambiguity rule `activity_lines_for` applies needs to know, per node,
    // whether its `frame_id` is shared by more than one node in the whole
    // reconstruction.
    attach_overlay(&mut roots, overlay, &frame_id_counts);
    assign_spans(&mut roots, overlay, &frame_id_counts);
    let resolutions = runtime::resolve_narration_markers(&narration_candidates(&roots));
    apply_narration_resolutions(&mut roots, resolutions);

    DetailTree { header, roots }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_core::procedure::activity::{ActivityEvent, ActivityKind};

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
    fn loop_rounds_only_come_from_loop_segments_and_stay_distinguishable_when_nested() {
        // No loop at all: empty.
        let no_loop = session_from(base_session_json(serde_json::Value::Array(vec![status(
            0,
            "solo",
            "Solo",
            "accepted",
            serde_json::json!([]),
        )])));
        let tree = project(&no_loop, &ActivityOverlay::default(), false);
        assert!(tree.roots[0].loop_rounds.is_empty());

        // One loop, recorded iteration 1 (zero-based) -> displayed round 2.
        let one_loop = session_from(base_session_json(serde_json::Value::Array(vec![status(
            0,
            "item",
            "Item",
            "accepted",
            serde_json::json!([
                {"kind": "procedure", "id": "the-loop", "index": 0},
                {"kind": "loop", "id": "the-loop-body", "index": 0, "iteration": 1},
                {"kind": "item", "id": "item", "index": 0, "iteration": 1},
            ]),
        )])));
        let tree = project(&one_loop, &ActivityOverlay::default(), false);
        let loop_group = &tree.roots[0].children[0];
        assert_eq!(loop_group.kind, "loop");
        assert_eq!(loop_group.loop_rounds, vec![2]);

        // A non-loop control segment (`item`) carrying its own `iteration`
        // must not contribute a round — only `kind == "loop"` segments do.
        let item_leaf = &loop_group.children[0];
        assert_eq!(item_leaf.kind, "item");
        assert_eq!(
            item_leaf.loop_rounds,
            vec![2],
            "the leaf still inherits its enclosing loop's round, but its own \
             `item` iteration contributes nothing extra"
        );

        // Nested loops keep both rounds, outermost first, not flattened.
        let nested = session_from(base_session_json(serde_json::Value::Array(vec![status(
            0,
            "item",
            "Item",
            "accepted",
            serde_json::json!([
                {"kind": "procedure", "id": "outer", "index": 0},
                {"kind": "loop", "id": "outer-body", "index": 0, "iteration": 1},
                {"kind": "loop", "id": "inner-body", "index": 0, "iteration": 2},
                {"kind": "item", "id": "item", "index": 0, "iteration": 2},
            ]),
        )])));
        let tree = project(&nested, &ActivityOverlay::default(), false);
        let outer_group = &tree.roots[0].children[0];
        let inner_group = &outer_group.children[0];
        assert_eq!(inner_group.kind, "loop");
        assert_eq!(inner_group.loop_rounds, vec![2, 3], "outermost round first");
    }

    /// Review-verdict-1 blocker `desktop-loop-marker-rederived`:
    /// `narration_marker` is `Some` only at the innermost loop-control
    /// boundary — never on a solo loop's ancestor-free case's absence, never
    /// duplicated on both levels of a direct nesting, and still correctly
    /// suppressed on the outer loop when a non-loop structural node (a
    /// `branch`) separates it from a nested loop.
    #[test]
    fn narration_marker_is_set_only_at_the_innermost_loop_boundary() {
        // A single loop with no nested loop: it is its own innermost
        // boundary.
        let one_loop = session_from(base_session_json(serde_json::Value::Array(vec![status(
            0,
            "item",
            "Item",
            "accepted",
            serde_json::json!([
                {"kind": "procedure", "id": "the-loop", "index": 0},
                {"kind": "loop", "id": "the-loop-body", "index": 0, "iteration": 1},
                {"kind": "item", "id": "item", "index": 0, "iteration": 1},
            ]),
        )])));
        let tree = project(&one_loop, &ActivityOverlay::default(), false);
        let loop_group = &tree.roots[0].children[0];
        assert_eq!(loop_group.narration_marker.as_deref(), Some("2"));

        // Directly nested loops: only the inner one is a marker.
        let nested = session_from(base_session_json(serde_json::Value::Array(vec![status(
            0,
            "item",
            "Item",
            "accepted",
            serde_json::json!([
                {"kind": "procedure", "id": "outer", "index": 0},
                {"kind": "loop", "id": "outer-body", "index": 0, "iteration": 1},
                {"kind": "loop", "id": "inner-body", "index": 0, "iteration": 2},
                {"kind": "item", "id": "item", "index": 0, "iteration": 2},
            ]),
        )])));
        let tree = project(&nested, &ActivityOverlay::default(), false);
        let outer_group = &tree.roots[0].children[0];
        let inner_group = &outer_group.children[0];
        assert_eq!(
            outer_group.narration_marker, None,
            "the outer loop suppresses its own marker when it has a loop descendant"
        );
        assert_eq!(inner_group.narration_marker.as_deref(), Some("2/3"));

        // Loops separated by a non-loop structural node (a `branch`): the
        // outer loop must still be suppressed even though the nested loop
        // is not its immediate child.
        let separated = session_from(base_session_json(serde_json::Value::Array(vec![status(
            0,
            "item",
            "Item",
            "accepted",
            serde_json::json!([
                {"kind": "procedure", "id": "outer", "index": 0},
                {"kind": "loop", "id": "outer-body", "index": 0, "iteration": 1},
                {"kind": "branch", "id": "arm-a", "index": 0},
                {"kind": "loop", "id": "inner-body", "index": 0, "iteration": 2},
                {"kind": "item", "id": "item", "index": 0, "iteration": 2},
            ]),
        )])));
        let tree = project(&separated, &ActivityOverlay::default(), false);
        let outer_group = &tree.roots[0].children[0];
        let branch_group = &outer_group.children[0];
        let inner_group = &branch_group.children[0];
        assert_eq!(
            outer_group.narration_marker, None,
            "an intervening branch node must not hide the nested loop from the outer level"
        );
        assert_eq!(branch_group.narration_marker, None);
        assert_eq!(inner_group.narration_marker.as_deref(), Some("2/3"));
    }

    /// Review-verdict-1 blocker `desktop-loop-marker-rederived`'s mixed
    /// reconstruction: one path ends its loop nesting at the outer loop (a
    /// plain sibling item with no nested loop), while another path under the
    /// *same* outer-loop iteration enters a nested loop. The outer loop must
    /// keep its own marker for the plain path's sake, even though the
    /// nested-loop branch also carries its own (joined) innermost marker —
    /// an any-descendant-has-loop suppression would wrongly hide the outer
    /// marker just because a sibling branch happens to nest a loop.
    #[test]
    fn mixed_outer_only_and_nested_branches_both_get_their_own_marker() {
        let mixed = session_from(base_session_json(serde_json::Value::Array(vec![
            status(
                0,
                "nested-item",
                "Nested Item",
                "accepted",
                serde_json::json!([
                    {"kind": "procedure", "id": "outer", "index": 0},
                    {"kind": "loop", "id": "outer-body", "index": 0, "iteration": 1},
                    {"kind": "branch", "id": "arm-a", "index": 0},
                    {"kind": "loop", "id": "inner-body", "index": 0, "iteration": 2},
                    {"kind": "item", "id": "nested-item", "index": 0, "iteration": 2},
                ]),
            ),
            status(
                1,
                "plain-item",
                "Plain Item",
                "accepted",
                serde_json::json!([
                    {"kind": "procedure", "id": "outer", "index": 0},
                    {"kind": "loop", "id": "outer-body", "index": 0, "iteration": 1},
                    {"kind": "item", "id": "plain-item", "index": 1, "iteration": 1},
                ]),
            ),
        ])));
        let tree = project(&mixed, &ActivityOverlay::default(), false);
        let outer_group = &tree.roots[0].children[0];
        assert_eq!(outer_group.kind, "loop");
        assert_eq!(
            outer_group.narration_marker.as_deref(),
            Some("2"),
            "the outer loop keeps its own marker: the plain-item path has no \
             nested loop to carry it instead"
        );

        let branch_group = outer_group
            .children
            .iter()
            .find(|child| child.kind == "branch")
            .expect("the nested-item path's branch arm");
        let inner_group = &branch_group.children[0];
        assert_eq!(inner_group.kind, "loop");
        assert_eq!(
            inner_group.narration_marker.as_deref(),
            Some("2/3"),
            "the nested branch still carries its own, separately joined marker"
        );

        let plain_item = outer_group
            .children
            .iter()
            .find(|child| child.kind == "item")
            .expect("the plain-item leaf, a direct child of the outer loop");
        assert_eq!(
            plain_item.narration_marker, None,
            "a leaf is never a marker"
        );
    }

    #[test]
    fn activity_lines_cap_at_eight_newest_first_and_ambiguous_ids_show_only_live_entries() {
        use ctx_traits_core::procedure::activity::ActivityEvent;

        fn event(sequence: u64, text: &str) -> ActivityEvent {
            ActivityEvent {
                sequence,
                frame_id: "item".to_string(),
                kind: ActivityKind::Thinking,
                text: Some(text.to_string()),
                tool: None,
                tokens: None,
                rate_limit: None,
            }
        }

        let mut overlay = ActivityOverlay::default();
        for at_epoch_ms in 0..10u64 {
            overlay.apply_record(&ActivityRecord::Activity {
                at_epoch_ms,
                event: event(at_epoch_ms, &format!("line-{at_epoch_ms}")),
            });
        }
        let lines = overlay.activity_lines_for("item", 1);
        assert_eq!(lines.len(), 8, "at most eight lines are retained");
        assert_eq!(
            lines.first().and_then(|e| e.text.as_deref()),
            Some("line-9"),
            "the newest accepted line is first"
        );
        assert_eq!(
            lines.last().and_then(|e| e.text.as_deref()),
            Some("line-2"),
            "the ninth and older lines are dropped, not the newest"
        );

        // An ambiguous frame_id (executions > 1, i.e. two executions of one
        // looped item) must show only entries folded live, never a seed line.
        let mut ambiguous = ActivityOverlay::default();
        ambiguous.apply_record(&ActivityRecord::Activity {
            at_epoch_ms: 1,
            event: event(1, "seed-line"),
        });
        ambiguous.apply_live_record(&ActivityRecord::Activity {
            at_epoch_ms: 2,
            event: event(2, "live-line"),
        });
        let unambiguous = ambiguous.activity_lines_for("item", 1);
        assert_eq!(unambiguous.len(), 2, "one execution sees the whole window");
        let filtered = ambiguous.activity_lines_for("item", 2);
        assert_eq!(
            filtered.len(),
            1,
            "more than one execution sees only the live-folded entries"
        );
        assert_eq!(filtered[0].text.as_deref(), Some("live-line"));

        // Zero lines render nothing.
        assert!(
            ActivityOverlay::default()
                .activity_lines_for("no-such-frame", 1)
                .is_empty()
        );
    }

    /// Review-verdict-1 blocker `seed-pending-activity-replay-duplicates-line`:
    /// a seed/pending replay of the exact same `Activity` occurrence must
    /// promote the retained line's provenance to `live` in place, never
    /// append a second visible line for it. A genuinely distinct event
    /// sharing the replayed record's timestamp must still land as its own
    /// line, and an ambiguous (looped) `frame_id` must show exactly the one
    /// promoted line, not the seed line plus a duplicate.
    #[test]
    fn seed_pending_replay_promotes_the_retained_line_instead_of_duplicating_it() {
        use ctx_traits_core::procedure::activity::ActivityEvent;

        fn event(sequence: u64, text: &str) -> ActivityEvent {
            ActivityEvent {
                sequence,
                frame_id: "item".to_string(),
                kind: ActivityKind::Thinking,
                text: Some(text.to_string()),
                tool: None,
                tokens: None,
                rate_limit: None,
            }
        }

        let seed_line = ActivityRecord::Activity {
            at_epoch_ms: 1,
            event: event(1, "seed-line"),
        };

        // Unambiguous frame_id: replaying the seed's own occurrence through
        // `apply_pending_record` must leave exactly one visible line.
        let mut overlay = ActivityOverlay::from_records(std::slice::from_ref(&seed_line));
        overlay.apply_pending_record(&seed_line);
        let lines = overlay.activity_lines_for("item", 1);
        assert_eq!(
            lines.len(),
            1,
            "a seed/pending replay of one occurrence must not duplicate its line"
        );
        assert_eq!(lines[0].text.as_deref(), Some("seed-line"));

        // A genuinely distinct event sharing the replayed record's stamp
        // must remain a second, separate line.
        let distinct = ActivityRecord::Activity {
            at_epoch_ms: 1,
            event: event(2, "distinct-line"),
        };
        overlay.apply_pending_record(&distinct);
        let lines = overlay.activity_lines_for("item", 1);
        assert_eq!(
            lines.len(),
            2,
            "a distinct event at the same timestamp is not mistaken for a replay"
        );
        assert_eq!(lines[0].text.as_deref(), Some("distinct-line"));
        assert_eq!(lines[1].text.as_deref(), Some("seed-line"));

        // Ambiguous frame_id (executions > 1): the replayed occurrence's
        // promotion to `live` must be exactly what makes it visible.
        let mut ambiguous = ActivityOverlay::from_records(std::slice::from_ref(&seed_line));
        assert!(
            ambiguous.activity_lines_for("item", 2).is_empty(),
            "before any replay, an ambiguous id shows no seed-only line"
        );
        ambiguous.apply_pending_record(&seed_line);
        let promoted = ambiguous.activity_lines_for("item", 2);
        assert_eq!(
            promoted.len(),
            1,
            "the replay promotes exactly one line, never a duplicate, even when ambiguous"
        );
        assert_eq!(promoted[0].text.as_deref(), Some("seed-line"));
    }

    /// Review-verdict-1 blocker `seed-pending-activity-replay-duplicates-line`,
    /// the exact repeated-payload scenario the root-cause names: a seed deque
    /// `E@1, F@2, E@3` where `E` occurs twice with an identical full
    /// `ActivityEvent` payload, separated by a distinct `F@2`. Replaying `F@2`
    /// then `E@3` (the newer duplicate) must promote each occurrence at its
    /// own retained timestamp, never the wrong (older) `E@1` entry — an
    /// event-only match would find `E@1` first and leave the visible,
    /// ambiguous order reversed.
    #[test]
    fn seed_pending_replay_matches_the_exact_timestamped_occurrence_not_the_first_equal_event() {
        use ctx_traits_core::procedure::activity::ActivityEvent;

        fn event(sequence: u64, text: &str) -> ActivityEvent {
            ActivityEvent {
                sequence,
                frame_id: "item".to_string(),
                kind: ActivityKind::Thinking,
                text: Some(text.to_string()),
                tool: None,
                tokens: None,
                rate_limit: None,
            }
        }

        let e = event(1, "e");
        let f = event(2, "f");
        let seed_records = [
            ActivityRecord::Activity {
                at_epoch_ms: 1,
                event: e.clone(),
            },
            ActivityRecord::Activity {
                at_epoch_ms: 2,
                event: f.clone(),
            },
            ActivityRecord::Activity {
                at_epoch_ms: 3,
                event: e.clone(),
            },
        ];

        let mut overlay = ActivityOverlay::from_records(&seed_records);
        // Replay the distinct event first, then the newer duplicate — the
        // order review-verdict-1 specified.
        overlay.apply_pending_record(&seed_records[1]);
        overlay.apply_pending_record(&seed_records[2]);

        let unambiguous = overlay.activity_lines_for("item", 1);
        assert_eq!(
            unambiguous.len(),
            3,
            "no replay of an already-retained occurrence may duplicate a line"
        );
        assert_eq!(
            unambiguous
                .iter()
                .map(|e| e.text.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("e"), Some("f"), Some("e")],
            "newest-first order over all three retained occurrences is unaffected by promotion"
        );

        // Ambiguous: only the two promoted (live) occurrences are visible,
        // newest first — the newer duplicate (E@3), then the distinct event
        // (F@2) — never the un-replayed, older E@1.
        let ambiguous = overlay.activity_lines_for("item", 2);
        assert_eq!(
            ambiguous.len(),
            2,
            "exactly the two replayed occurrences are visible, each exactly once"
        );
        assert_eq!(
            ambiguous[0].text.as_deref(),
            Some("e"),
            "the newer duplicate (E@3) is promoted at its own timestamp, not E@1"
        );
        assert_eq!(
            ambiguous[1].text.as_deref(),
            Some("f"),
            "the distinct event (F@2) is the second-newest promoted occurrence"
        );
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
    fn activity_overlay_retains_both_records_newest_first() {
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
        let lines = overlay.activity_lines_for("the-frame", 1);
        assert_eq!(
            lines.len(),
            2,
            "both retained events survive, bounded window"
        );
        assert_eq!(
            lines[0].kind,
            ActivityKind::RunningTool,
            "newest line is first"
        );
        assert_eq!(lines[0].text.as_deref(), Some("second activity"));
        assert_eq!(lines[1].text.as_deref(), Some("first activity"));
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

    fn activity_record(at_epoch_ms: u64, frame_id: &str, sequence: u64) -> ActivityRecord {
        ActivityRecord::Activity {
            at_epoch_ms,
            event: ActivityEvent {
                sequence,
                frame_id: frame_id.to_string(),
                kind: ActivityKind::Thinking,
                text: None,
                tool: None,
                tokens: None,
                rate_limit: None,
            },
        }
    }

    /// A durable `Activity` below an unrelated presentation watermark must
    /// still contribute to the live span — and seed/delta overlap (the same
    /// record folded twice) must not double-apply it. `apply_live_record`'s
    /// live-fold result must match `from_records`' historical fold of the
    /// exact same append-ordered records, so a settled run reopened renders
    /// the same span it showed live (review-verdict-1 blocker
    /// `live-span-reopen-divergence`).
    #[test]
    fn live_span_matches_reopened_span() {
        let records = vec![
            activity_record(1_000, "the-frame", 1),
            // An unrelated record with a later stamp: this alone must not
            // suppress the still-pending later Activity for `the-frame`.
            ActivityRecord::Narration {
                at_epoch_ms: 9_000,
                frame_id: "another-frame".to_string(),
                text: "unrelated, later-stamped".to_string(),
            },
            // The frame's second (and last) Activity, delivered live after
            // the watermark already advanced past its own stamp.
            activity_record(4_000, "the-frame", 2),
        ];

        let mut live = ActivityOverlay::default();
        for record in &records[..2] {
            live.apply_live_record(record);
        }
        // Seed/delta overlap: the same watermark-advancing record above is
        // redelivered before the frame's real second Activity arrives.
        live.apply_live_record(&records[1]);
        let changed = live.apply_live_record(&records[2]);
        assert!(
            changed,
            "a durable Activity below the presentation watermark must still be observed"
        );

        let reopened = ActivityOverlay::from_records(&records);

        assert_eq!(
            live.span_for("the-frame", 1),
            Some(Duration::from_millis(3_000)),
            "the live fold must not drop the second Activity"
        );
        assert_eq!(
            live.span_for("the-frame", 1),
            reopened.span_for("the-frame", 1),
            "live and reopened projection of the same durable records must produce identical spans"
        );
    }

    /// Same-frame append order `1_000, 3_000, 2_000` — a non-monotonic final
    /// stamp. The final accepted live state and a freshly reopened load of
    /// the identical append-ordered records must both render the
    /// first-to-last 1-second span, not the maximum-timestamp 2-second span
    /// a timestamp-monotonic fold would produce (review-verdict-1 blocker
    /// `live-span-reopen-divergence`).
    #[test]
    fn live_span_matches_reopened_span_on_a_non_monotonic_final_stamp() {
        let records = vec![
            activity_record(1_000, "the-frame", 1),
            activity_record(3_000, "the-frame", 2),
            activity_record(2_000, "the-frame", 3),
        ];

        let mut live = ActivityOverlay::default();
        for record in &records {
            live.apply_live_record(record);
        }
        let reopened = ActivityOverlay::from_records(&records);

        assert_eq!(
            live.span_for("the-frame", 1),
            Some(Duration::from_millis(1_000)),
            "append order 1_000 -> 2_000 (the last folded stamp) is a 1-second span"
        );
        assert_eq!(
            live.span_for("the-frame", 1),
            reopened.span_for("the-frame", 1),
            "final accepted live state must match a fresh reopened load byte-for-byte"
        );
    }

    /// Seed/pending overlap: a durable record already included by the seed's
    /// historical read, then redelivered as a replayed live delta, must be
    /// reconciled by content (its `(frame_id, at_epoch_ms)` pair, not a
    /// `sequence` identity production does not reliably provide) rather than
    /// double-applied — every genuinely new `Activity` must still update the
    /// fold exactly once.
    #[test]
    fn live_span_matches_reopened_span_across_a_seed_pending_overlap() {
        let seed_records = vec![
            activity_record(1_000, "the-frame", 1),
            activity_record(3_000, "the-frame", 2),
        ];
        let mut overlay = ActivityOverlay::from_records(&seed_records);

        // The seed's own last record is redelivered live (a seed/delta race)
        // before the frame's genuinely new third Activity arrives.
        // `apply_live_record`'s return value covers presentation fields too
        // (an equal-timestamp redelivery is still folded there, unrelated to
        // span currency), so this proof asserts on the span directly: it
        // must not double-apply the overlapping record's stamp.
        overlay.apply_live_record(&seed_records[1]);
        assert_eq!(
            overlay.span_for("the-frame", 1),
            Some(Duration::from_millis(2_000))
        );

        let advanced = overlay.apply_live_record(&activity_record(2_000, "the-frame", 3));
        assert!(
            advanced,
            "a genuinely new record must still fold, even with an earlier stamp"
        );
        assert_eq!(
            overlay.span_for("the-frame", 1),
            Some(Duration::from_millis(1_000)),
            "append order 1_000 -> 2_000 (the new last-folded stamp) is a 1-second span"
        );
    }

    /// Production-shaped: `ActivityRecorder::flush_pending` persists every
    /// coalesced `Activity` with `sequence: 0`, and each drive's recorder
    /// restarts its own sequence counter, so two durable records for the
    /// same frame commonly share `sequence: 0`. A seed/pending overlap of
    /// such a record must still leave the unique frame with a positive
    /// first-to-last span, not an omitted one (review-verdict-1 blocker
    /// `live-span-reopen-divergence`).
    #[test]
    fn live_span_matches_reopened_span_when_flush_pending_reuses_sequence_zero() {
        let seed_records = vec![
            activity_record(1_000, "the-frame", 0),
            activity_record(3_000, "the-frame", 0),
        ];
        let mut live = ActivityOverlay::from_records(&seed_records[..1]);
        // The seed's read already landed the second flush too (a
        // seed/pending race); the caller replays it live regardless.
        for record in &seed_records {
            live.apply_live_record(record);
        }
        let reopened = ActivityOverlay::from_records(&seed_records);

        assert_eq!(
            live.span_for("the-frame", 1),
            Some(Duration::from_millis(2_000)),
            "both sequence-0 records must fold; sequence is not a durable identity"
        );
        assert_eq!(
            live.span_for("the-frame", 1),
            reopened.span_for("the-frame", 1)
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
            past.activity_lines.is_empty(),
            "a stale frame_id never attaches to a non-current node"
        );
        assert!(current.current);
        assert_eq!(
            current
                .activity_lines
                .first()
                .and_then(|e| e.text.as_deref()),
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
