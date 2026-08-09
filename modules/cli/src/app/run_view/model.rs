//! Pure view-model types: the plan/session projection (`RunView`,
//! `RunStep`, `RunHeader`, ...) and pane layout constants shared across
//! `run_view`'s tick/render/projection code.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::app::tui;
use crate::app::tui_panes::PaneId;

use super::{EventRow, PaneTitleRow};

pub(super) const PROGRESS_PANE: PaneId = "progress";
pub(super) const JOURNEY_PANE: PaneId = "journey";
pub(super) const HISTORY_PANE: PaneId = "history";
pub(super) const CURRENT_PANE: PaneId = "current";
/// A bordered pane needs two border rows plus one content row.
pub(super) const HISTORY_MIN_OUTER_ROWS: u16 = 3;
pub(super) const CURRENT_MIN_OUTER_ROWS: u16 = 8;
/// P552: below this width every populated pane stacks vertically instead of
/// the 2x2 (or narrower 2-pane) grid — unchanged from the pre-P552 threshold;
/// re-evaluated against the extra journey pane/borders and confirmed still
/// wide enough for a 60/40 two-column split to stay usable.
pub(super) const NARROW_WIDTH_THRESHOLD: u16 = 109;
pub(super) const GUIDE_FOOTER_HINT: &str = "[?] guide · [up/down] scroll · [pg] page · [home/end] jump · [tab] pane · [d] dash · [q] exit · [ctrl-c] kill";

/// P552: the (up to) four panes a run's presentation contract can show,
/// identified once so [`pane_tree`], [`render_pane_body`], and every caller
/// agree on the same ids — a live run's four leaves, dashboard preview's
/// progress/journey pair, and dashboard attach's four leaves each supply
/// their own set of ids here (dashboard's ids differ from the live ones so
/// its existing focus ring, which also holds the sessions list, is
/// unaffected by this module's own reconciliation).
pub(crate) struct PaneIds {
    pub(crate) progress: PaneId,
    pub(crate) journey: PaneId,
    pub(crate) history: PaneId,
    pub(crate) current: PaneId,
}

pub(crate) const LIVE_PANE_IDS: PaneIds = PaneIds {
    progress: PROGRESS_PANE,
    journey: JOURNEY_PANE,
    history: HISTORY_PANE,
    current: CURRENT_PANE,
};

/// P552: which lines/events populate each of the (up to) four panes this
/// module can render — entirely source-driven, with no separate live/
/// preview/attached mode flag anywhere in this module. A live run and a
/// dashboard attach supply all four; a dashboard preview supplies only
/// `progress`/`journey`; a legacy session with no activity sidecar omits
/// `history`/`current` rather than fabricating them.
pub(crate) struct PaneData<'a> {
    pub(crate) progress: Option<&'a [tui::Line]>,
    pub(crate) journey: Option<&'a [JourneyRow]>,
    pub(crate) history: Option<&'a [EventRow]>,
    pub(crate) current: Option<&'a [EventRow]>,
    /// Completed runs move observed merge work into this full-height pane.
    /// It deliberately reuses `current`'s pane id and scroll state.
    pub(crate) post_run: Option<&'a [tui::Line]>,
    /// P552: the title row this pane set's own body sits under — [`PaneTitleRow::None`]
    /// for a dashboard preview, [`PaneTitleRow::Reserved`] for a live run or
    /// a dashboard attach, so [`render_pane_body`] gives every surface
    /// identical title behavior rather than each caller reserving/rendering
    /// its own row.
    pub(crate) title: PaneTitleRow<'a>,
}

pub(super) const CURRENT_STREAM_CAP: usize = 400;
/// The ask strip is intentionally one display row for the answer. Keep its
/// presentation-only state bounded as well, so a verbose model cannot crowd
/// the live 2x2 view or retain an unbounded response in memory.
pub(super) const MAX_GUIDE_ANSWER_CHARS: usize = 600;

/// Presentation-only context for a just-completed step (P455), returned by
/// [`RunPanel::refresh`] when it observes an accepted active-key transition.
/// Built entirely from panel-held display state (never slot values), so the
/// narrator's finish-and-summarize request never reads the ledger.
pub(crate) struct CompletedStepContext {
    /// The completed step's own `structural_step_key` (see [`step_key`]), so
    /// a P455 summary that lands asynchronously — typically after the next
    /// step has already started — still joins the story row of the step it
    /// actually narrated, never whatever step happens to be current when it
    /// returns.
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) role: String,
    pub(crate) elapsed: Duration,
    pub(crate) work_tokens: Option<u64>,
}

#[derive(Debug, Clone)]
pub(super) struct RunView {
    pub(super) header: RunHeader,
    pub(super) steps: Vec<RunStep>,
    pub(super) history: Vec<HistoryStep>,
    pub(super) narration: Option<RunNarration>,
    pub(super) outputs: Vec<RunOutput>,
    /// P549: merge stage rows, rendered directly after `steps` in the same
    /// pane the run's own journey draws in — empty for every run that never
    /// hands a [`RunPanel`] into a merge span. Carried on `RunPanelState`
    /// itself (not derived by [`run_view`]) since a merge event has nothing
    /// to do with the plan/session `run_view` otherwise projects from.
    pub(super) merge_rows: Vec<MergeRowView>,
}

/// One accepted execution as recorded by the ledger. Unlike `RunStep`, this
/// is deliberately not a plan projection: a loop body can occur many times.
#[derive(Debug, Clone)]
pub(super) struct HistoryStep {
    pub(super) label: String,
    pub(super) kind: Option<ctx_traits_core::procedure::run::PlannedSequenceKind>,
    pub(super) outcome: Option<HistoryOutcome>,
    pub(super) elapsed: Option<Duration>,
    pub(super) output_tokens: Option<u64>,
    pub(super) summary: Option<String>,
    pub(super) summary_at: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HistoryOutcome {
    Check {
        ok: bool,
        exit_code: Option<i64>,
    },
    Command {
        succeeded: bool,
        exit_code: Option<i32>,
    },
}

/// One folded row of merge stage progress, keyed by the P504
/// `ActivityEvent::frame_id` its updates arrive under (`merge:<stage-slug>`,
/// [`merge_story::stage_text`]'s own wording) so a stage's several events
/// (start, gate commands, merger attempts, terminal outcome) collapse into
/// one row that updates in place rather than growing the pane unbounded.
#[derive(Debug, Clone)]
pub(super) struct MergeRowView {
    pub(super) frame_id: String,
    pub(super) label: String,
    pub(super) state: MergeRowState,
    pub(super) detail: Option<String>,
    pub(super) started: Instant,
    pub(super) finished: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MergeRowState {
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone)]
pub(super) struct RunHeader {
    pub(super) session_id: String,
    pub(super) run_id: String,
    pub(super) input: String,
    pub(super) harnesses: String,
    pub(super) done: usize,
    pub(super) total: usize,
    pub(super) phase: String,
    pub(super) completed: bool,
    pub(super) stopped: Option<String>,
    pub(super) stop_detail: Option<String>,
    pub(super) state_digest: String,
    pub(super) harness_count: usize,
    pub(super) elapsed: Option<Duration>,
    pub(super) output_tokens: Option<u64>,
    /// P445: drive-wide narrator output tokens, shown distinctly from
    /// `output_tokens` (the work agent's total).
    pub(super) narrator_tokens: Option<u64>,
    pub(super) guide_tokens: Option<u64>,
    pub(super) structured_count: usize,
    pub(super) structured_label: Option<String>,
    pub(super) structured_verdict: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct RunStep {
    pub(super) key: String,
    pub(super) label: String,
    pub(super) role: String,
    pub(super) harness: Option<String>,
    pub(super) tags: Vec<String>,
    pub(super) status: String,
    pub(super) state: StepState,
    pub(super) active: bool,
    pub(super) counts_progress: bool,
    #[allow(dead_code)] // Retained plan facts; compact journey rows no longer render ports.
    pub(super) inputs: Vec<PortSlug>,
    pub(super) outputs: Vec<PortSlug>,
    pub(super) elapsed: Option<Duration>,
    pub(super) output_tokens: Option<u64>,
    /// `Some(loop_container_key)` when this step is a loop/for-each
    /// container; its elapsed/tokens are populated from the presentation
    /// loop aggregates instead of the generic per-step maps.
    pub(super) loop_key: Option<String>,
    /// `true` when this step is a container currently on the active path
    /// (`Activity::Ancestor` — a loop mid-iteration, say), from
    /// [`item_activity`]. Drives the journey pane's ladder follow target
    /// (0082): the ladder is exactly the rows with this flag set, outermost
    /// first, with the active row itself appended.
    pub(super) on_active_path: bool,
    pub(super) position_path: Vec<ctx_traits_core::procedure::runtime::PathSegment>,
    pub(super) run_index: usize,
    pub(super) structured_count: usize,
    /// P470: this step's P455 finished-step summary, when one landed —
    /// `None` for a narrator-free run, a failed/timed-out/disabled
    /// narration, or a step that hasn't produced one yet. Drives the story
    /// column's one-line-per-completed-step compression; the facts fallback
    /// (`label · elapsed · tokens`) is used in its absence.
    pub(super) summary: Option<String>,
    /// Elapsed-since-pane-start stamp for `summary`, `None` alongside it.
    pub(super) summary_at: Option<Duration>,
}

#[derive(Clone)]
pub(crate) struct JourneyRow(pub(super) JourneyRowKind);

#[allow(dead_code)] // Used by dashboard fixtures; production rows originate here.
pub(crate) fn journey_line(line: tui::Line) -> JourneyRow {
    JourneyRow(JourneyRowKind::Line(line))
}

#[derive(Clone)]
pub(super) enum JourneyRowKind {
    Step(Box<RunStep>),
    Line(tui::Line),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StepState {
    Done,
    Running,
    Pending,
    Failed,
}

/// How a planned item relates to the frame the session is currently serving.
/// Loop/for-each children share their container's run slot, so run-index
/// equality alone smears "active" across the container and every child; only
/// the exactly-current item may claim the agent column and the live line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Activity {
    /// The item the current frame executes.
    Current,
    /// A container somewhere on the active path (a loop mid-iteration).
    Ancestor,
    Idle,
}

#[derive(Debug, Clone)]
pub(super) struct PortSlug {
    pub(super) slug: String,
    #[allow(dead_code)] // Retained port satisfaction fact; no longer rendered in journey rows.
    pub(super) satisfied: bool,
}

#[derive(Debug, Clone)]
pub(super) struct RunNarration {
    pub(super) label: String,
    pub(super) text: String,
    pub(super) muted: bool,
    pub(super) finished: bool,
}

#[derive(Debug, Clone)]
pub(super) struct RunOutput {
    pub(super) slug: String,
    pub(super) status: String,
    pub(super) accepted: bool,
}

pub(super) struct PresentationState<'a> {
    pub(super) active_started: &'a Option<(String, Instant)>,
    pub(super) finished_durations: &'a BTreeMap<String, Duration>,
    pub(super) output_tokens: &'a BTreeMap<String, u64>,
    pub(super) loop_elapsed: &'a BTreeMap<String, Duration>,
    pub(super) loop_output_tokens: &'a BTreeMap<String, u64>,
    pub(super) step_summaries: &'a BTreeMap<String, String>,
    pub(super) step_summary_at: &'a BTreeMap<String, Duration>,
    pub(super) narrator_tokens: u64,
    pub(super) guide_tokens: u64,
    pub(super) run_started: Instant,
    /// Whether this view belongs to the in-process drive of the run (as
    /// opposed to a dashboard observer or a ledger preview). A live drive
    /// that has a command frame at the cursor is EXECUTING it — the derived
    /// `BlockedCommandPermissionRequired` status names the frame kind, not a
    /// wait — while an observer of a parked session cannot tell the two
    /// apart and keeps the conservative status text.
    pub(super) live_drive: bool,
}
