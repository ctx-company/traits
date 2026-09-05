//! `ctx traits sessions report <session>` (0281.6 goals 1 and 3): a
//! read-only, per-loop, per-iteration account of one run, reconstructed from
//! records the run already wrote — the session ledger (slot revisions with
//! their position paths, sequence titles, the drive outcome) and the
//! activity sidecar (dispatch/tool/verdict/command-attempt events with
//! timestamps). No model is involved and nothing is written: the report is a
//! projection of existing evidence, and nothing acts on its numbers.
//!
//! The shape is generic across traits. A loop is identified by its control
//! item id, an iteration by its one-based round, a frame by its sequence
//! item; the numbers per frame are the frame's wall time, tool calls,
//! retries, rejections and outcome, plus a compact summary of every slot
//! value the frame submitted (top-level scalar fields of a JSON object,
//! status histograms for arrays of objects, a short preview of text).

use std::collections::BTreeMap;
use std::time::Duration;

use camino::Utf8Path;
use ctx_traits_core::procedure::activity::ActivityKind;
use ctx_traits_core::procedure::runtime::{PathSegment, is_loop_control_kind};
use ctx_traits_core::procedure::session::{DriveOutcomeKind, Session};
use ctx_traits_core::response::CommandOutput;
use ctx_traits_io::activity_sidecar::ActivityRecord;
use serde::Serialize;
use serde_json::Value as JsonValue;

use crate::app::command_handlers::print_json_report;
use crate::app::presentation::{
    HumanOutputMode, Panel, PanelRow, PanelSection, PanelStatus, RowTone, emit_human, wire_name,
};
use crate::app::tui::human_elapsed_text;

pub(crate) const REPORT_SCHEMA_VERSION: &str = "0.1.0";

/// Characters kept of a text slot value in a frame row.
const TEXT_PREVIEW_CHARS: usize = 60;
/// Characters kept of a string field inside a JSON object summary.
const FIELD_PREVIEW_CHARS: usize = 24;
/// A narrator step summary is attached to a frame only when it lands within
/// this window after the frame's verdict — summaries resolve asynchronously,
/// but never long after the step they describe.
const STEP_SUMMARY_WINDOW_MS: u64 = 60_000;

/// The whole report: header facts, the ordered entry tree, and run totals.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct SessionReport {
    pub(crate) schema_version: &'static str,
    pub(crate) session_id: String,
    pub(crate) run_id: String,
    pub(crate) trait_id: String,
    pub(crate) status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) drive_outcome: Option<String>,
    /// A driver holds the session's lock right now.
    pub(crate) live: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) started_at_epoch_ms: Option<u64>,
    /// The ledger's own cumulative active-drive seconds.
    pub(crate) ledger_elapsed_seconds: u64,
    pub(crate) entries: Vec<Entry>,
    pub(crate) totals: Totals,
    /// Sidecar lines that could not be parsed (a truncated tail, an
    /// unknown record); counted, never fatal.
    pub(crate) skipped_activity_lines: usize,
}

/// One node of the entry tree: a frame at this level, or a loop whose
/// iterations hold their own entries. Order is chronological.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub(crate) enum Entry {
    Frame(Box<FrameRow>),
    Loop(LoopSection),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct LoopSection {
    /// The loop's control item id (`reviewed-refinement`), not its body
    /// sequence id.
    pub(crate) id: String,
    pub(crate) iterations: Vec<Iteration>,
    pub(crate) totals: Totals,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Iteration {
    /// One-based, as the runtime displays rounds.
    pub(crate) round: usize,
    pub(crate) entries: Vec<Entry>,
    pub(crate) totals: Totals,
    /// A frame inside this iteration is still running.
    pub(crate) running: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Totals {
    pub(crate) frames: usize,
    pub(crate) iterations: usize,
    /// Sum of frame wall seconds (dispatch to verdict).
    pub(crate) seconds: u64,
    pub(crate) tool_calls: usize,
    pub(crate) rejections: usize,
    pub(crate) retries: usize,
}

impl Totals {
    fn add(&mut self, other: Totals) {
        self.frames += other.frames;
        self.iterations += other.iterations;
        self.seconds += other.seconds;
        self.tool_calls += other.tool_calls;
        self.rejections += other.rejections;
        self.retries += other.retries;
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FrameKind {
    Agent,
    Command,
}

/// How a frame ended. `Accepted`/`Rejected` come from the runtime's own
/// verdict record; `Running` is a frame without a verdict on a live run;
/// the drive-outcome variants explain a verdict-less frame on a run whose
/// driver exited; `Unfinished` is a verdict-less frame nothing explains.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FrameOutcome {
    Accepted,
    Rejected,
    Running,
    Killed,
    Interrupted,
    Unfinished,
    Other(String),
}

impl FrameOutcome {
    fn label(&self) -> String {
        match self {
            FrameOutcome::Accepted => "accepted".to_string(),
            FrameOutcome::Rejected => "rejected".to_string(),
            FrameOutcome::Running => "running".to_string(),
            FrameOutcome::Killed => "killed".to_string(),
            FrameOutcome::Interrupted => "interrupted".to_string(),
            FrameOutcome::Unfinished => "unfinished".to_string(),
            FrameOutcome::Other(text) => text.clone(),
        }
    }

    fn tone(&self) -> RowTone {
        match self {
            FrameOutcome::Accepted => RowTone::Default,
            FrameOutcome::Running => RowTone::Warn,
            FrameOutcome::Rejected
            | FrameOutcome::Killed
            | FrameOutcome::Interrupted
            | FrameOutcome::Unfinished
            | FrameOutcome::Other(_) => RowTone::Fail,
        }
    }
}

/// Bounded facts of a local command frame, from the command-attempt
/// journal.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct CommandFacts {
    pub(crate) attempts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) signal: Option<i32>,
    pub(crate) killed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) timeout_kind: Option<String>,
}

/// One slot value a frame submitted, summarized.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct SlotWrite {
    pub(crate) slot: String,
    pub(crate) summary: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct LoopPosition {
    pub(crate) id: String,
    pub(crate) round: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct FrameRow {
    pub(crate) item_id: String,
    pub(crate) title: String,
    pub(crate) frame_kind: FrameKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) agent: Option<String>,
    pub(crate) started_at_epoch_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) ended_at_epoch_ms: Option<u64>,
    pub(crate) seconds: u64,
    pub(crate) tool_calls: usize,
    pub(crate) retries: usize,
    pub(crate) rejections: usize,
    pub(crate) outcome: FrameOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) verdict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) command: Option<CommandFacts>,
    /// The narrator's finished-step summary, when one landed for this frame.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) summary: Option<String>,
    pub(crate) slots: Vec<SlotWrite>,
    /// Enclosing loops, outermost first.
    pub(crate) loops: Vec<LoopPosition>,
}

/// A frame that has started (an activity event, a command attempt, or a
/// verdict named it) and has not yet been closed by an accepting verdict.
#[derive(Debug, Clone)]
struct PendingFrame {
    item_id: String,
    frame_kind: FrameKind,
    started_at: u64,
    last_at: u64,
    tool_calls: usize,
    retries: usize,
    rejections: usize,
    position_path: Vec<PathSegment>,
    agent: Option<String>,
    command: Option<CommandFacts>,
    verdict: Option<String>,
    reason: Option<String>,
}

impl PendingFrame {
    fn open(item_id: &str, at: u64, frame_kind: FrameKind) -> Self {
        Self {
            item_id: item_id.to_string(),
            frame_kind,
            started_at: at,
            last_at: at,
            tool_calls: 0,
            retries: 0,
            rejections: 0,
            position_path: Vec::new(),
            agent: None,
            command: None,
            verdict: None,
            reason: None,
        }
    }
}

/// The loop chain a position path passes through, outermost first: each
/// loop control segment yields (control item id, one-based round). The
/// control item id is the `procedure` segment directly before the loop
/// body segment when there is one (`plan-approval` before
/// `plan-approval-body`); otherwise the body segment's own id.
fn loop_chain(path: &[PathSegment]) -> Vec<LoopPosition> {
    let mut chain = Vec::new();
    for (index, segment) in path.iter().enumerate() {
        if !is_loop_control_kind(&segment.kind) {
            continue;
        }
        let control = index
            .checked_sub(1)
            .and_then(|previous| path.get(previous))
            .filter(|previous| previous.kind == "procedure")
            .and_then(|previous| previous.id.clone());
        let id = control
            .or_else(|| segment.id.clone())
            .unwrap_or_else(|| "loop".to_string());
        chain.push(LoopPosition {
            id,
            round: segment.iteration.unwrap_or(0).saturating_add(1),
        });
    }
    chain
}

fn chain_key(chain: &[LoopPosition]) -> String {
    chain
        .iter()
        .map(|position| format!("{}#{}", position.id, position.round))
        .collect::<Vec<_>>()
        .join("/")
}

fn item_id_of(path: &[PathSegment]) -> Option<&str> {
    path.iter()
        .rev()
        .find(|segment| segment.kind == "item")
        .and_then(|segment| segment.id.as_deref())
}

fn classify_verdict(verdict: &str) -> Option<FrameOutcome> {
    if verdict.starts_with("accepted") {
        Some(FrameOutcome::Accepted)
    } else if verdict.starts_with("rejected") {
        Some(FrameOutcome::Rejected)
    } else {
        None
    }
}

/// Compact one-line summary of a submitted slot value: text as a short
/// quoted preview; a JSON object as its top-level scalar fields, with
/// arrays of objects rendered as a count plus a `status` histogram (one
/// nesting level deep, so `blockers=1 (open 4) steps: open 4, done 1`
/// reads off a review verdict); anything else as its JSON size.
pub(crate) fn summarize_value(value: &JsonValue) -> String {
    match value {
        JsonValue::String(text) => format!("\"{}\"", preview(text, TEXT_PREVIEW_CHARS)),
        JsonValue::Object(map) => {
            // Deterministic regardless of the map's own ordering: `status`
            // first (the field a reader looks for), the rest alphabetical.
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by_key(|key| (key.as_str() != "status", key.as_str()));
            let mut parts = Vec::new();
            for key in keys {
                let field = &map[key];
                match field {
                    JsonValue::Null => parts.push(format!("{key}=null")),
                    JsonValue::Bool(flag) => parts.push(format!("{key}={flag}")),
                    JsonValue::Number(number) => parts.push(format!("{key}={number}")),
                    // An empty string says nothing worth a column.
                    JsonValue::String(text) if text.trim().is_empty() => {}
                    JsonValue::String(text) => {
                        parts.push(format!("{key}={}", scalar_text(text)));
                    }
                    JsonValue::Array(items) => {
                        let mut part = format!("{key}={}", items.len());
                        if let Some(histogram) = status_histogram(items) {
                            part.push_str(&format!(" ({histogram})"));
                        }
                        parts.push(part);
                        for (nested_key, histogram) in nested_status_histograms(items) {
                            parts.push(format!("{nested_key}: {histogram}"));
                        }
                    }
                    JsonValue::Object(nested) => {
                        if let Some(JsonValue::String(status)) = nested.get("status") {
                            parts.push(format!("{key}.status={}", scalar_text(status)));
                        } else {
                            parts.push(format!("{key}={{{}}}", nested.len()));
                        }
                    }
                }
            }
            parts.join(" ")
        }
        JsonValue::Array(items) => format!("[{}]", items.len()),
        JsonValue::Null => "null".to_string(),
        JsonValue::Bool(flag) => flag.to_string(),
        JsonValue::Number(number) => number.to_string(),
    }
}

fn scalar_text(text: &str) -> String {
    let short = preview(text, FIELD_PREVIEW_CHARS);
    if short.chars().any(char::is_whitespace) || short.is_empty() {
        format!("\"{short}\"")
    } else {
        short
    }
}

/// Whitespace-collapsed prefix of `text`, with an ellipsis when cut.
fn preview(text: &str, chars: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = collapsed.chars().count();
    if count <= chars {
        collapsed
    } else {
        let mut cut: String = collapsed.chars().take(chars.saturating_sub(1)).collect();
        cut.push('…');
        cut
    }
}

/// `open 4, done 1` over the `status` string field of object items, or
/// `None` when no item carries one.
fn status_histogram(items: &[JsonValue]) -> Option<String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for item in items {
        let Some(JsonValue::String(status)) = item.get("status") else {
            continue;
        };
        match counts.iter_mut().find(|(name, _)| name == status) {
            Some((_, count)) => *count += 1,
            None => counts.push((status.clone(), 1)),
        }
    }
    if counts.is_empty() {
        return None;
    }
    Some(
        counts
            .iter()
            .map(|(name, count)| format!("{name} {count}"))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

/// For every array-of-objects field inside `items`, the status histogram
/// aggregated across all items — `steps` inside `blockers`.
fn nested_status_histograms(items: &[JsonValue]) -> Vec<(String, String)> {
    let mut nested: BTreeMap<String, Vec<JsonValue>> = BTreeMap::new();
    for item in items {
        let Some(map) = item.as_object() else {
            continue;
        };
        for (key, field) in map {
            if let JsonValue::Array(children) = field {
                nested
                    .entry(key.clone())
                    .or_default()
                    .extend(children.iter().cloned());
            }
        }
    }
    nested
        .into_iter()
        .filter_map(|(key, children)| status_histogram(&children).map(|histogram| (key, histogram)))
        .collect()
}

/// Reconstruct the report from a session and its sidecar records. Pure: the
/// caller supplies liveness (the driver-lock probe) and the clock.
pub(crate) fn build_report(
    session: &Session,
    records: &[ActivityRecord],
    skipped_activity_lines: usize,
    live: bool,
    now_epoch_ms: u64,
) -> SessionReport {
    let titles: BTreeMap<String, String> = session
        .ledger
        .sequence_statuses
        .iter()
        .filter_map(|status| {
            status
                .item_id
                .as_ref()
                .map(|id| (id.clone(), status.title.clone()))
        })
        .collect();

    let drive_outcome = session
        .last_drive_outcome
        .as_ref()
        .map(|outcome| outcome.outcome.clone());
    let frames = reconstruct_frames(session, records, live, drive_outcome.as_ref());

    // Slot revisions keyed by (loop chain, item id), in acceptance order.
    let mut revisions: BTreeMap<(String, String), Vec<SlotWrite>> = BTreeMap::new();
    for revision in &session.slot_revisions {
        let Some(item_id) = item_id_of(&revision.position_path) else {
            continue;
        };
        let key = (
            chain_key(&loop_chain(&revision.position_path)),
            item_id.to_string(),
        );
        let summary = revision
            .submitted_payload
            .as_ref()
            .map(|payload| summarize_value(&payload.value))
            .unwrap_or_else(|| "(no payload)".to_string());
        revisions.entry(key).or_default().push(SlotWrite {
            slot: revision.slot_ref.id().to_string(),
            summary,
        });
    }

    let mut entries: Vec<Entry> = Vec::new();
    for pending in frames {
        let chain = loop_chain(&pending.position_path);
        let key = (chain_key(&chain), pending.item_id.clone());
        let slots = revisions.remove(&key).unwrap_or_default();
        let title = titles
            .get(&pending.item_id)
            .cloned()
            .unwrap_or_else(|| pending.item_id.clone());
        let row = FrameRow {
            item_id: pending.item_id,
            title,
            frame_kind: pending.frame_kind,
            agent: pending.agent,
            started_at_epoch_ms: pending.started_at,
            ended_at_epoch_ms: pending.ended_at,
            seconds: pending
                .ended_at
                .unwrap_or(now_epoch_ms)
                .saturating_sub(pending.started_at)
                / 1000,
            tool_calls: pending.tool_calls,
            retries: pending.retries,
            rejections: pending.rejections,
            outcome: pending.outcome,
            verdict: pending.verdict,
            reason: pending.reason,
            command: pending.command,
            summary: pending.summary,
            slots,
            loops: chain.clone(),
        };
        insert_frame(&mut entries, &chain, row);
    }

    let totals = fold_totals(&mut entries);
    let started_at_epoch_ms = session
        .provenance
        .started_at_epoch
        .map(|seconds| seconds.saturating_mul(1000))
        .or_else(|| first_frame_start(&entries));

    SessionReport {
        schema_version: REPORT_SCHEMA_VERSION,
        session_id: session.session_id.as_str().to_string(),
        run_id: session.run_id.as_str().to_string(),
        trait_id: session.trait_id.clone(),
        status: wire_name(&session.status),
        drive_outcome: drive_outcome.as_ref().map(wire_name),
        live,
        started_at_epoch_ms,
        ledger_elapsed_seconds: session.ledger.elapsed_seconds,
        // (a u64 on every ledger: the runtime stamps it on each rebuild)
        entries,
        totals,
        skipped_activity_lines,
    }
}

/// A closed frame before its slot values and title are resolved.
struct ClosedFrame {
    item_id: String,
    frame_kind: FrameKind,
    started_at: u64,
    ended_at: Option<u64>,
    tool_calls: usize,
    retries: usize,
    rejections: usize,
    position_path: Vec<PathSegment>,
    agent: Option<String>,
    command: Option<CommandFacts>,
    verdict: Option<String>,
    reason: Option<String>,
    outcome: FrameOutcome,
    summary: Option<String>,
}

fn close(pending: PendingFrame, ended_at: Option<u64>, outcome: FrameOutcome) -> ClosedFrame {
    ClosedFrame {
        item_id: pending.item_id,
        frame_kind: pending.frame_kind,
        started_at: pending.started_at,
        ended_at,
        tool_calls: pending.tool_calls,
        retries: pending.retries,
        rejections: pending.rejections,
        position_path: pending.position_path,
        agent: pending.agent,
        command: pending.command,
        verdict: pending.verdict,
        reason: pending.reason,
        outcome,
        summary: None,
    }
}

/// Walk the sidecar once. A frame opens on the first record naming its item
/// (an activity event, a command attempt, or the verdict itself), collects
/// tool calls and retries, and closes on an accepting verdict; a rejecting
/// verdict counts against the still-open frame, since the runtime re-asks
/// the same frame. Frames left open at the end are running on a live run
/// and otherwise explained by the drive outcome.
fn reconstruct_frames(
    session: &Session,
    records: &[ActivityRecord],
    live: bool,
    drive_outcome: Option<&DriveOutcomeKind>,
) -> Vec<ClosedFrame> {
    let mut open: Vec<PendingFrame> = Vec::new();
    let mut closed: Vec<ClosedFrame> = Vec::new();

    fn find_or_open<'a>(
        open: &'a mut Vec<PendingFrame>,
        item_id: &str,
        at: u64,
        frame_kind: FrameKind,
    ) -> &'a mut PendingFrame {
        let index = match open.iter().position(|frame| frame.item_id == item_id) {
            Some(index) => index,
            None => {
                open.push(PendingFrame::open(item_id, at, frame_kind));
                open.len() - 1
            }
        };
        &mut open[index]
    }

    for record in records {
        match record {
            ActivityRecord::Activity { at_epoch_ms, event } => {
                // Only a dispatch opens a frame. A harness keeps streaming
                // for a moment after the runtime accepted its output, and
                // those trailing events must not open a phantom frame for
                // the same item that the next real dispatch would then
                // inherit (an early start and an inflated duration).
                let frame = if event.kind == ActivityKind::Dispatching {
                    find_or_open(&mut open, &event.frame_id, *at_epoch_ms, FrameKind::Agent)
                } else {
                    match open
                        .iter_mut()
                        .find(|frame| frame.item_id == event.frame_id)
                    {
                        Some(frame) => frame,
                        None => continue,
                    }
                };
                frame.last_at = *at_epoch_ms;
                match event.kind {
                    ActivityKind::RunningTool => frame.tool_calls += 1,
                    ActivityKind::Retrying => frame.retries += 1,
                    _ => {}
                }
            }
            ActivityRecord::CommandAttemptStarted {
                at_epoch_ms,
                item_id: Some(item_id),
                position_path,
                attempt,
                ..
            } => {
                let frame = find_or_open(&mut open, item_id, *at_epoch_ms, FrameKind::Command);
                frame.last_at = *at_epoch_ms;
                if !position_path.is_empty() {
                    frame.position_path = position_path.clone();
                }
                let facts = frame.command.get_or_insert_with(CommandFacts::default);
                facts.attempts = facts.attempts.max(*attempt);
            }
            ActivityRecord::CommandAttemptEnded {
                at_epoch_ms,
                item_id: Some(item_id),
                position_path,
                attempt,
                exit_code,
                signal,
                killed,
                timeout_kind,
                ..
            } => {
                let frame = find_or_open(&mut open, item_id, *at_epoch_ms, FrameKind::Command);
                frame.last_at = *at_epoch_ms;
                if !position_path.is_empty() {
                    frame.position_path = position_path.clone();
                }
                frame.command = Some(CommandFacts {
                    attempts: frame
                        .command
                        .as_ref()
                        .map_or(*attempt, |facts| facts.attempts.max(*attempt)),
                    exit_code: *exit_code,
                    signal: *signal,
                    killed: *killed,
                    timeout_kind: timeout_kind.clone(),
                });
            }
            ActivityRecord::Verdict {
                at_epoch_ms,
                verdict,
                reason,
                item_id: Some(item_id),
                position_path,
                agent,
                ..
            } => {
                let index = match open.iter().position(|frame| frame.item_id == *item_id) {
                    Some(index) => index,
                    None => {
                        open.push(PendingFrame::open(item_id, *at_epoch_ms, FrameKind::Agent));
                        open.len() - 1
                    }
                };
                let frame = &mut open[index];
                if !position_path.is_empty() {
                    frame.position_path = position_path.clone();
                }
                if agent.is_some() {
                    frame.agent = agent.clone();
                }
                frame.verdict = Some(verdict.clone());
                frame.reason = Some(reason.clone());
                match classify_verdict(verdict) {
                    Some(FrameOutcome::Rejected) => {
                        frame.rejections += 1;
                        frame.last_at = *at_epoch_ms;
                    }
                    Some(outcome) => {
                        let pending = open.remove(index);
                        closed.push(close(pending, Some(*at_epoch_ms), outcome));
                    }
                    None => {
                        let pending = open.remove(index);
                        closed.push(close(
                            pending,
                            Some(*at_epoch_ms),
                            FrameOutcome::Other(verdict.clone()),
                        ));
                    }
                }
            }
            ActivityRecord::StepSummary {
                at_epoch_ms,
                key,
                text,
                ..
            } => {
                if let Some(frame) = closed.iter_mut().rev().find(|frame| {
                    frame.summary.is_none()
                        && key.contains(&format!("item:{}:", frame.item_id))
                        && frame.ended_at.is_some_and(|ended| {
                            at_epoch_ms.saturating_sub(ended) <= STEP_SUMMARY_WINDOW_MS
                        })
                }) {
                    frame.summary = Some(text.clone());
                }
            }
            _ => {}
        }
    }

    let current_item = session.current_sequence_item_id.as_deref();
    for pending in open {
        let is_current = current_item == Some(pending.item_id.as_str());
        let mut pending = pending;
        if pending.position_path.is_empty() && is_current {
            pending.position_path = current_position_path(session, &pending.item_id);
        }
        let outcome = if live && (is_current || current_item.is_none()) {
            FrameOutcome::Running
        } else if pending.command.as_ref().is_some_and(|facts| facts.killed) {
            FrameOutcome::Killed
        } else if pending.rejections > 0 {
            FrameOutcome::Rejected
        } else {
            match drive_outcome {
                Some(DriveOutcomeKind::Killed) if is_current => FrameOutcome::Killed,
                Some(DriveOutcomeKind::Interrupted) if is_current => FrameOutcome::Interrupted,
                _ => FrameOutcome::Unfinished,
            }
        };
        let ended_at = match outcome {
            FrameOutcome::Running => None,
            _ => Some(pending.last_at.max(pending.started_at)),
        };
        closed.push(close(pending, ended_at, outcome));
    }

    closed.sort_by_key(|frame| frame.started_at);
    closed
}

/// The position of the frame the session is currently on, synthesized from
/// the control stack: an agent frame only learns its position path from
/// its verdict, so a frame still running (or killed mid-flight) has none of
/// its own, while the ledger's control stack names every enclosing loop and
/// its iteration.
fn current_position_path(session: &Session, item_id: &str) -> Vec<PathSegment> {
    let mut path = Vec::new();
    for frame in &session.control_stack {
        if !is_loop_control_kind(&wire_name(&frame.kind)) {
            continue;
        }
        path.push(PathSegment {
            kind: "procedure".to_string(),
            id: frame.control_item_id.clone(),
            index: frame.parent_run_index,
            iteration: None,
            item_index: None,
        });
        path.push(PathSegment {
            kind: "loop".to_string(),
            id: Some(frame.sequence_id.clone()),
            index: frame.next_index,
            iteration: frame.iteration_index,
            item_index: None,
        });
    }
    path.push(PathSegment {
        kind: "item".to_string(),
        id: Some(item_id.to_string()),
        index: 0,
        iteration: None,
        item_index: None,
    });
    path
}

/// Place a frame under its loop chain, reusing the trailing loop entry and
/// iteration when the chain continues them and opening new ones otherwise,
/// so chronological order survives inside every level.
fn insert_frame(entries: &mut Vec<Entry>, chain: &[LoopPosition], frame: FrameRow) {
    let Some(head) = chain.first() else {
        entries.push(Entry::Frame(Box::new(frame)));
        return;
    };
    let continues = matches!(entries.last(), Some(Entry::Loop(section)) if section.id == head.id);
    if !continues {
        entries.push(Entry::Loop(LoopSection {
            id: head.id.clone(),
            iterations: Vec::new(),
            totals: Totals::default(),
        }));
    }
    let Some(Entry::Loop(section)) = entries.last_mut() else {
        unreachable!("a loop entry was just ensured at the tail");
    };
    let same_round = section
        .iterations
        .last()
        .is_some_and(|iteration| iteration.round == head.round);
    if !same_round {
        section.iterations.push(Iteration {
            round: head.round,
            entries: Vec::new(),
            totals: Totals::default(),
            running: false,
        });
    }
    let iteration = section
        .iterations
        .last_mut()
        .expect("an iteration was just ensured");
    insert_frame(&mut iteration.entries, &chain[1..], frame);
}

fn fold_totals(entries: &mut [Entry]) -> Totals {
    let mut totals = Totals::default();
    for entry in entries {
        match entry {
            Entry::Frame(frame) => {
                totals.frames += 1;
                totals.seconds += frame.seconds;
                totals.tool_calls += frame.tool_calls;
                totals.rejections += frame.rejections;
                totals.retries += frame.retries;
            }
            Entry::Loop(section) => {
                let mut loop_totals = Totals::default();
                for iteration in &mut section.iterations {
                    let inner = fold_totals(&mut iteration.entries);
                    iteration.totals = inner;
                    iteration.running = iteration_running(&iteration.entries);
                    loop_totals.add(inner);
                    loop_totals.iterations += 1;
                }
                section.totals = loop_totals;
                totals.add(loop_totals);
            }
        }
    }
    totals
}

fn iteration_running(entries: &[Entry]) -> bool {
    entries.iter().any(|entry| match entry {
        Entry::Frame(frame) => frame.outcome == FrameOutcome::Running,
        Entry::Loop(section) => section.iterations.iter().any(|it| it.running),
    })
}

fn first_frame_start(entries: &[Entry]) -> Option<u64> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Frame(frame) => Some(frame.started_at_epoch_ms),
            Entry::Loop(section) => section
                .iterations
                .iter()
                .filter_map(|iteration| first_frame_start(&iteration.entries))
                .min(),
        })
        .min()
}

/// `YYYY-MM-DD HH:MM UTC` from epoch milliseconds — the report needs one
/// unambiguous wall-clock anchor and the CLI carries no timezone crate.
pub(crate) fn utc_clock(epoch_ms: u64) -> String {
    let seconds = epoch_ms / 1000;
    let days = seconds / 86_400;
    let rem = seconds % 86_400;
    let (hours, minutes) = (rem / 3600, (rem % 3600) / 60);
    // Civil-from-days (Howard Hinnant), valid for every date this tool sees.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02} {hours:02}:{minutes:02} UTC")
}

fn elapsed(seconds: u64) -> String {
    human_elapsed_text(Duration::from_secs(seconds))
}

fn offset_label(report: &SessionReport, at_epoch_ms: u64) -> String {
    let origin = report.started_at_epoch_ms.unwrap_or(at_epoch_ms);
    format!("+{}", elapsed(at_epoch_ms.saturating_sub(origin) / 1000))
}

fn totals_text(totals: &Totals, with_iterations: bool) -> String {
    let mut parts = Vec::new();
    if with_iterations {
        parts.push(format!("{} iteration(s)", totals.iterations));
    }
    parts.push(format!("{} frame(s)", totals.frames));
    parts.push(elapsed(totals.seconds));
    parts.push(format!("{} tool call(s)", totals.tool_calls));
    if totals.rejections > 0 {
        parts.push(format!("{} rejected", totals.rejections));
    }
    if totals.retries > 0 {
        parts.push(format!("{} retried", totals.retries));
    }
    parts.join(" · ")
}

fn frame_value(frame: &FrameRow) -> String {
    let mut parts = vec![frame.title.clone()];
    if let Some(agent) = &frame.agent {
        parts.push(format!("@{agent}"));
    }
    parts.push(elapsed(frame.seconds));
    if frame.frame_kind == FrameKind::Agent {
        parts.push(format!("{} tool call(s)", frame.tool_calls));
    }
    if frame.retries > 0 {
        parts.push(format!("{} retried", frame.retries));
    }
    if frame.rejections > 0 {
        parts.push(format!("{} rejected", frame.rejections));
    }
    parts.push(frame.outcome.label());
    if let Some(facts) = &frame.command {
        if let Some(code) = facts.exit_code {
            parts.push(format!("exit {code}"));
        }
        if let Some(signal) = facts.signal {
            parts.push(format!("signal {signal}"));
        }
        if let Some(timeout) = &facts.timeout_kind {
            parts.push(format!("timeout {timeout}"));
        }
    }
    for slot in &frame.slots {
        parts.push(format!("{} {}", slot.slot, slot.summary));
    }
    parts.join(" · ")
}

/// Flatten the entry tree into panel sections: consecutive top-level frames
/// form a `run` section; every iteration with frames of its own forms one
/// section titled with its loop chain and totals.
fn collect_sections(
    report: &SessionReport,
    entries: &[Entry],
    breadcrumb: &[String],
    sections: &mut Vec<PanelSection>,
) {
    let mut run_rows: Vec<PanelRow> = Vec::new();
    let flush = |rows: &mut Vec<PanelRow>, sections: &mut Vec<PanelSection>| {
        if rows.is_empty() {
            return;
        }
        let title = if breadcrumb.is_empty() {
            "run".to_string()
        } else {
            breadcrumb.join(" › ")
        };
        sections.push(PanelSection::new(title, std::mem::take(rows)));
    };
    for entry in entries {
        match entry {
            Entry::Frame(frame) => run_rows.push(PanelRow::toned(
                offset_label(report, frame.started_at_epoch_ms),
                frame_value(frame),
                frame.outcome.tone(),
            )),
            Entry::Loop(section) => {
                flush(&mut run_rows, sections);
                for iteration in &section.iterations {
                    let mut crumb = breadcrumb.to_vec();
                    let mut title = format!(
                        "loop {} · iteration {} · {}",
                        section.id,
                        iteration.round,
                        totals_text(&iteration.totals, false)
                    );
                    if iteration.running {
                        title.push_str(" · running");
                    }
                    crumb.push(title);
                    collect_sections(report, &iteration.entries, &crumb, sections);
                }
            }
        }
    }
    flush(&mut run_rows, sections);
}

fn closing_status(report: &SessionReport) -> PanelStatus {
    if report.live {
        return PanelStatus::Passed("running".to_string());
    }
    match report.drive_outcome.as_deref() {
        Some("completed") => PanelStatus::Passed("completed".to_string()),
        Some(outcome @ ("killed" | "interrupted" | "rejected" | "failed" | "blocked")) => {
            PanelStatus::Blocked(outcome.to_string())
        }
        Some(outcome) if outcome.contains("exhausted") => PanelStatus::Blocked(outcome.to_string()),
        Some(outcome) => PanelStatus::Passed(outcome.to_string()),
        None => match report.status.as_str() {
            "completed" => PanelStatus::Passed("completed".to_string()),
            "failed" | "rejected" | "blocked" => PanelStatus::Blocked(report.status.clone()),
            other => PanelStatus::Passed(other.to_string()),
        },
    }
}

pub(crate) fn render_panel(report: &SessionReport) -> Panel {
    let mut panel = Panel::new("ctx", "sessions report", closing_status(report))
        .row(PanelRow::toned(
            "session",
            &report.session_id,
            RowTone::Default,
        ))
        .row(PanelRow::toned("trait", &report.trait_id, RowTone::Default))
        .row(PanelRow::toned("status", &report.status, RowTone::Default));
    if let Some(outcome) = &report.drive_outcome {
        panel = panel.row(PanelRow::toned("outcome", outcome, RowTone::Default));
    }
    if let Some(started) = report.started_at_epoch_ms {
        panel = panel.row(PanelRow::toned(
            "started",
            utc_clock(started),
            RowTone::Default,
        ));
    }
    panel = panel.row(PanelRow::toned(
        "active drive",
        elapsed(report.ledger_elapsed_seconds),
        RowTone::Default,
    ));
    let mut sections = Vec::new();
    collect_sections(report, &report.entries, &[], &mut sections);
    for section in sections {
        panel = panel.section(section);
    }
    let mut totals = vec![PanelRow::toned(
        "totals",
        totals_text(&report.totals, true),
        RowTone::Default,
    )];
    if report.skipped_activity_lines > 0 {
        totals.push(PanelRow::toned(
            "skipped activity lines",
            report.skipped_activity_lines.to_string(),
            RowTone::Warn,
        ));
    }
    panel.section(PanelSection::new("totals", totals))
}

/// Load a ledger and its sidecar from disk and build the report. `live` is
/// the caller's driver-lock probe result.
pub(crate) fn load_report(
    ledger_path: &Utf8Path,
    live: bool,
    now_epoch_ms: u64,
) -> crate::Result<SessionReport> {
    let session = ctx_traits_io::run_session::read_run_session(ledger_path)?;
    let (records, skipped) = if ctx_traits_io::activity_sidecar::activity_exists(ledger_path) {
        ctx_traits_io::activity_sidecar::read_activity(ledger_path)
    } else {
        (Vec::new(), 0)
    };
    Ok(build_report(
        &session,
        &records,
        skipped,
        live,
        now_epoch_ms,
    ))
}

fn now_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// `ctx traits sessions report <session> [--json]`.
pub(crate) fn handle_sessions_report(
    session: &str,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let (path, _) = crate::app::story::resolve_run(session, None)?;
    let live = matches!(
        ctx_traits_io::run_control::probe(&path),
        Ok(ctx_traits_io::run_control::DriverProbe::Held(_))
    );
    let report = load_report(&path, live, now_epoch_ms())?;
    if json {
        print_json_report(&report, "session report")?;
        return Ok(CommandOutput::new(()));
    }
    let panel = render_panel(&report);
    emit_human(false, &panel, HumanOutputMode::Compact, || Ok(()))?;
    Ok(CommandOutput::new(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_core::procedure::activity::ActivityEvent;
    use serde_json::json;

    fn segment(kind: &str, id: &str, index: usize, iteration: Option<usize>) -> serde_json::Value {
        let mut value = json!({ "kind": kind, "id": id, "index": index });
        if let Some(iteration) = iteration {
            value["iteration"] = json!(iteration);
        }
        value
    }

    fn loop_path(round0: usize, item: &str, index: usize) -> serde_json::Value {
        json!([
            segment("procedure", "refine", 3, None),
            segment("loop", "refine-body", index, Some(round0)),
            segment("item", item, index, Some(round0)),
        ])
    }

    fn fixture_session(status: &str, outcome: Option<&str>, current_item: &str) -> Session {
        let mut value = json!({
            "schema-version": "0.1.0",
            "session-id": "session-report-fixture",
            "run-id": "run-report-fixture",
            "trait-id": "report-fixture",
            "current-run-index": 3,
            "current-sequence-item-id": current_item,
            "status": status,
            "control-stack": [{
                "kind": "loop",
                "parent-run-index": 3,
                "control-item-id": "refine",
                "sequence-id": "refine-body",
                "next-index": 1,
                "iteration-index": 1,
                "unbounded": true,
            }],
            "provenance": {
                "started-by": {"surface": "test", "caller": "session-report-proof"},
                "state-source": "test",
                "started-at-epoch": 1_000,
            },
            "slot-revisions": [
                {
                    "slot-ref": "slot:base",
                    "value-digest": "sha256:base",
                    "acceptance-order": 1,
                    "submitted-payload": {"value": "abc123\n"},
                    "position-path": [segment("item", "capture-base", 0, None)],
                },
                {
                    "slot-ref": "slot:work-summary",
                    "value-digest": "sha256:w1",
                    "acceptance-order": 2,
                    "submitted-payload": {"value": "Implemented   the first\nslice of the stage and validated it with tests."},
                    "position-path": loop_path(0, "implement", 0),
                },
                {
                    "slot-ref": "slot:verdict",
                    "value-digest": "sha256:v1",
                    "acceptance-order": 3,
                    "submitted-payload": {"value": {
                        "status": "revise",
                        "blockers": [
                            {"id": "b1", "status": "open", "steps": [
                                {"step": "one", "status": "open"},
                                {"step": "two", "status": "done"},
                            ]}
                        ],
                        "stages": [{"id": "s1", "status": "open"}],
                    }},
                    "position-path": loop_path(0, "review", 1),
                },
                {
                    "slot-ref": "slot:work-summary",
                    "value-digest": "sha256:w2",
                    "acceptance-order": 4,
                    "submitted-payload": {"value": "Second round"},
                    "position-path": loop_path(1, "implement", 0),
                },
            ],
            "ledger": {
                "run-id": "run-report-fixture",
                "trait-id": "report-fixture",
                "current-run-index": 3,
                "final-state": "running",
                "elapsed-seconds": 900,
                "sequence-statuses": [
                    {"sequence-index": 0, "run-index": 0, "item-id": "capture-base", "title": "Capture the base", "status": "accepted", "reason": "accepted"},
                    {"sequence-index": 0, "run-index": 3, "item-id": "implement", "title": "Implement the task", "status": "accepted", "reason": "accepted"},
                    {"sequence-index": 1, "run-index": 3, "item-id": "review", "title": "Review the implementation", "status": "accepted", "reason": "accepted"},
                ],
            },
            "state-digest": "sha256:report-fixture",
        });
        if let Some(outcome) = outcome {
            value["last-drive-outcome"] = json!({"outcome": outcome, "recorded-at-epoch": 2_000});
        }
        serde_json::from_value(value).expect("fixture session")
    }

    fn activity(at: u64, frame: &str, kind: ActivityKind) -> ActivityRecord {
        ActivityRecord::Activity {
            at_epoch_ms: at,
            event: ActivityEvent {
                sequence: 0,
                frame_id: frame.to_string(),
                kind,
                text: None,
                tool: None,
                tokens: None,
                rate_limit: None,
            },
        }
    }

    fn verdict(
        at: u64,
        item: &str,
        verdict: &str,
        path: serde_json::Value,
        agent: Option<&str>,
    ) -> ActivityRecord {
        ActivityRecord::Verdict {
            at_epoch_ms: at,
            verdict: verdict.to_string(),
            reason: "reason".to_string(),
            item_id: Some(item.to_string()),
            source_index: Some(0),
            position_path: serde_json::from_value(path).expect("path"),
            surface: None,
            caller: None,
            agent: agent.map(str::to_string),
            harness: None,
        }
    }

    fn command_records(start: u64, item: &str, path: serde_json::Value) -> Vec<ActivityRecord> {
        let position_path: Vec<PathSegment> = serde_json::from_value(path).expect("path");
        vec![
            ActivityRecord::CommandAttemptStarted {
                at_epoch_ms: start,
                item_id: Some(item.to_string()),
                source_index: Some(0),
                run_index: Some(0),
                position_path: position_path.clone(),
                attempt: 1,
            },
            ActivityRecord::CommandAttemptEnded {
                at_epoch_ms: start + 400,
                item_id: Some(item.to_string()),
                source_index: Some(0),
                run_index: Some(0),
                position_path: position_path.clone(),
                attempt: 1,
                started_at_epoch_ms: start,
                exit_code: Some(0),
                signal: None,
                killed: false,
                timeout_kind: None,
                stdout_tail: "abc123".to_string(),
                stdout_tail_truncated: false,
                stderr_tail: String::new(),
                stderr_tail_truncated: false,
            },
            ActivityRecord::Verdict {
                at_epoch_ms: start + 500,
                verdict: "accepted-next-frame".to_string(),
                reason: "accepted".to_string(),
                item_id: Some(item.to_string()),
                source_index: Some(0),
                position_path,
                surface: Some("local-runtime-command".to_string()),
                caller: None,
                agent: None,
                harness: None,
            },
        ]
    }

    /// Two rounds of a loop after one top-level command frame; round one's
    /// implement frame was rejected once before being accepted.
    fn fixture_records(finish_second_round: bool) -> Vec<ActivityRecord> {
        let mut records = command_records(
            1_000_000,
            "capture-base",
            json!([segment("item", "capture-base", 0, None)]),
        );
        // Round 1: implement (2 tools, one rejection, then accepted), review.
        records.push(activity(1_010_000, "implement", ActivityKind::Dispatching));
        records.push(activity(1_020_000, "implement", ActivityKind::RunningTool));
        records.push(activity(1_030_000, "implement", ActivityKind::RunningTool));
        records.push(verdict(
            1_100_000,
            "implement",
            "rejected-correction-required",
            loop_path(0, "implement", 0),
            Some("worker"),
        ));
        records.push(activity(1_110_000, "implement", ActivityKind::Retrying));
        records.push(verdict(
            1_490_000,
            "implement",
            "accepted-next-frame",
            loop_path(0, "implement", 0),
            Some("worker"),
        ));
        records.push(ActivityRecord::StepSummary {
            at_epoch_ms: 1_491_000,
            key: "3:procedure:refine:3::/loop:refine-body:0:0:/item:implement:0:0::worker"
                .to_string(),
            role: "worker".to_string(),
            text: "Implemented the first slice".to_string(),
        });
        records.push(activity(1_500_000, "review", ActivityKind::Dispatching));
        records.push(activity(1_510_000, "review", ActivityKind::RunningTool));
        records.push(verdict(
            1_800_000,
            "review",
            "accepted-next-frame",
            loop_path(0, "review", 1),
            Some("smart"),
        ));
        // Round 2: implement dispatched; finished or still open.
        records.push(activity(1_900_000, "implement", ActivityKind::Dispatching));
        records.push(activity(1_910_000, "implement", ActivityKind::RunningTool));
        if finish_second_round {
            records.push(verdict(
                2_200_000,
                "implement",
                "accepted-next-frame",
                loop_path(1, "implement", 0),
                Some("worker"),
            ));
        }
        records
    }

    fn frames_of(entries: &[Entry]) -> Vec<&FrameRow> {
        entries
            .iter()
            .flat_map(|entry| match entry {
                Entry::Frame(frame) => vec![frame.as_ref()],
                Entry::Loop(section) => section
                    .iterations
                    .iter()
                    .flat_map(|iteration| frames_of(&iteration.entries))
                    .collect(),
            })
            .collect()
    }

    #[test]
    fn finished_run_groups_frames_by_loop_and_iteration_with_slot_summaries() {
        let session = fixture_session("awaiting-agent-output", Some("killed"), "review");
        let report = build_report(&session, &fixture_records(true), 0, false, 9_000_000);

        assert_eq!(report.status, "awaiting-agent-output");
        assert_eq!(report.drive_outcome.as_deref(), Some("killed"));
        assert!(!report.live);
        assert_eq!(report.started_at_epoch_ms, Some(1_000_000));
        assert_eq!(report.ledger_elapsed_seconds, 900);

        // Top-level command frame, then one loop with two iterations.
        assert_eq!(report.entries.len(), 2, "{:#?}", report.entries);
        let Entry::Frame(base) = &report.entries[0] else {
            panic!("first entry is the top-level command frame");
        };
        assert_eq!(base.title, "Capture the base");
        assert_eq!(base.frame_kind, FrameKind::Command);
        assert_eq!(base.outcome, FrameOutcome::Accepted);
        assert_eq!(
            base.command.as_ref().map(|facts| facts.exit_code),
            Some(Some(0))
        );
        assert_eq!(
            base.slots,
            vec![SlotWrite {
                slot: "base".to_string(),
                summary: "\"abc123\"".to_string()
            }]
        );

        let Entry::Loop(section) = &report.entries[1] else {
            panic!("second entry is the loop");
        };
        assert_eq!(section.id, "refine");
        assert_eq!(section.iterations.len(), 2);
        assert_eq!(section.totals.iterations, 2);
        assert_eq!(section.totals.rejections, 1);
        assert_eq!(section.totals.retries, 1);

        let round1 = &section.iterations[0];
        assert_eq!(round1.round, 1);
        assert!(!round1.running);
        let frames = frames_of(&round1.entries);
        assert_eq!(frames.len(), 2);
        let implement = frames[0];
        assert_eq!(implement.title, "Implement the task");
        assert_eq!(implement.agent.as_deref(), Some("worker"));
        assert_eq!(implement.seconds, 480);
        assert_eq!(implement.tool_calls, 2);
        assert_eq!(implement.rejections, 1);
        assert_eq!(implement.retries, 1);
        assert_eq!(implement.outcome, FrameOutcome::Accepted);
        assert_eq!(
            implement.summary.as_deref(),
            Some("Implemented the first slice")
        );
        assert_eq!(
            implement.loops,
            vec![LoopPosition {
                id: "refine".to_string(),
                round: 1
            }]
        );
        assert_eq!(implement.slots.len(), 1);
        assert_eq!(implement.slots[0].slot, "work-summary");
        assert_eq!(
            implement.slots[0].summary,
            "\"Implemented the first slice of the stage and validated it w…\""
        );
        let review = frames[1];
        assert_eq!(review.title, "Review the implementation");
        assert_eq!(review.seconds, 300);
        assert_eq!(review.tool_calls, 1);
        assert_eq!(
            review.slots[0].summary,
            "status=revise blockers=1 (open 1) steps: open 1, done 1 stages=1 (open 1)"
        );

        let round2 = &section.iterations[1];
        assert_eq!(round2.round, 2);
        let frames = frames_of(&round2.entries);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].outcome, FrameOutcome::Accepted);
        assert_eq!(frames[0].slots[0].summary, "\"Second round\"");

        assert_eq!(
            report.totals,
            Totals {
                frames: 4,
                iterations: 2,
                seconds: 1080,
                tool_calls: 4,
                rejections: 1,
                retries: 1
            }
        );
    }

    #[test]
    fn live_run_marks_the_open_frame_running_and_a_dead_run_explains_it() {
        let session = fixture_session("awaiting-agent-output", None, "implement");
        let live = build_report(&session, &fixture_records(false), 2, true, 2_500_000);
        assert!(live.live);
        assert_eq!(live.skipped_activity_lines, 2);
        let Entry::Loop(section) = &live.entries[1] else {
            panic!("loop entry");
        };
        let round2 = &section.iterations[1];
        assert!(round2.running);
        let frames = frames_of(&round2.entries);
        assert_eq!(frames[0].outcome, FrameOutcome::Running);
        assert_eq!(frames[0].ended_at_epoch_ms, None);
        assert_eq!(frames[0].seconds, 600, "seconds so far run to the clock");
        assert!(matches!(closing_status(&live), PanelStatus::Passed(text) if text == "running"));

        let killed_session = fixture_session("awaiting-agent-output", Some("killed"), "implement");
        let dead = build_report(
            &killed_session,
            &fixture_records(false),
            0,
            false,
            2_500_000,
        );
        let Entry::Loop(section) = &dead.entries[1] else {
            panic!("loop entry");
        };
        let frames = frames_of(&section.iterations[1].entries);
        assert_eq!(frames[0].outcome, FrameOutcome::Killed);
        assert_eq!(frames[0].ended_at_epoch_ms, Some(1_910_000));
        assert!(matches!(closing_status(&dead), PanelStatus::Blocked(text) if text == "killed"));
    }

    #[test]
    fn panel_renders_sections_per_iteration_and_json_round_trips() {
        let session = fixture_session("awaiting-agent-output", Some("completed"), "review");
        let report = build_report(&session, &fixture_records(true), 0, false, 9_000_000);
        let plain = render_panel(&report).plain_lines().join("\n");
        assert!(plain.contains("run:"), "{plain}");
        assert!(plain.contains("loop refine · iteration 1 · 2 frame(s) · 13m 0s · 3 tool call(s) · 1 rejected · 1 retried:"), "{plain}");
        assert!(
            plain.contains("loop refine · iteration 2 · 1 frame(s) · 5m 0s · 1 tool call(s):"),
            "{plain}"
        );
        assert!(plain.contains("Implement the task · @worker · 8m 0s · 2 tool call(s) · 1 retried · 1 rejected · accepted · work-summary"), "{plain}");
        assert!(
            plain.contains("Capture the base · 0s · accepted · exit 0 · base \"abc123\""),
            "{plain}"
        );
        assert!(plain.contains("totals: 2 iteration(s) · 4 frame(s) · 18m 0s · 4 tool call(s) · 1 rejected · 1 retried"), "{plain}");
        assert!(plain.contains("started: 1970-01-01 00:16 UTC"), "{plain}");

        let json = serde_json::to_value(&report).expect("serializable");
        assert_eq!(json["schema-version"], REPORT_SCHEMA_VERSION);
        assert_eq!(json["entries"][1]["kind"], "loop");
        assert_eq!(
            json["entries"][1]["iterations"][0]["entries"][0]["outcome"],
            "accepted"
        );
        assert_eq!(json["totals"]["tool-calls"], 4);
    }

    #[test]
    fn report_loads_from_a_scratch_ledger_and_sidecar() {
        let root = camino::Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("UTF-8 temp dir")
            .join(format!("ctx-session-report-{}", std::process::id()));
        std::fs::create_dir_all(root.as_std_path()).expect("scratch root");
        let ledger = root.join("session-report-fixture.json");
        let session = fixture_session("awaiting-agent-output", Some("completed"), "review");
        ctx_traits_io::run_session::write_run_session(&ledger, &session).expect("write ledger");
        let mut lines: Vec<String> = fixture_records(true)
            .iter()
            .map(|record| serde_json::to_string(record).expect("record json"))
            .collect();
        lines.push("{\"record\":\"truncated".to_string());
        std::fs::write(
            ctx_traits_io::activity_sidecar::activity_path(&ledger).as_std_path(),
            lines.join("\n"),
        )
        .expect("write sidecar");

        let report = load_report(&ledger, false, 9_000_000).expect("report loads");
        assert_eq!(report.totals.frames, 4);
        assert_eq!(report.skipped_activity_lines, 1);
        assert_eq!(report.session_id, "session-report-fixture");

        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn value_summaries_are_compact_and_generic() {
        assert_eq!(summarize_value(&json!("  a  b\n c ")), "\"a b c\"");
        assert_eq!(
            summarize_value(
                &json!({"claim": "complete", "next": "cut over the CLI ingress and then fix flow", "count": 3, "ok": true})
            ),
            "claim=complete count=3 next=\"cut over the CLI ingres…\" ok=true"
        );
        assert_eq!(summarize_value(&json!([1, 2, 3])), "[3]");
        assert_eq!(
            summarize_value(&json!({"brief": {"status": "open"}, "meta": {"a": 1}})),
            "brief.status=open meta={1}"
        );
        assert_eq!(utc_clock(1_788_548_985_000), "2026-09-04 19:09 UTC");
    }
}
