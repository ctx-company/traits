//! Live run-journey presentation for driven procedure sessions.
//!
//! This module is CLI presentation only. It maps an already-built dry plan plus
//! live session state into styled terminal lines; it never mutates the run ledger
//! or changes driver/report semantics.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent};
use ctx_traits_core::procedure::activity::{ActivityEvent, ActivityKind};
use serde_json::Value;

use crate::app::tui;
use crate::app::tui_kit;
use crate::app::tui_panes::{
    self, FocusRing, PaneId, PaneLayoutResult, PaneScrolls, PaneTree, TabStep,
};
use crate::app::tui_ratatui::{self, RatatuiPane};
use ratatui::layout::{Constraint, Direction, Layout, Rect};

type GuideDispatch = Arc<dyn Fn(String, String) -> crate::Result<String> + Send + Sync>;

#[derive(Default)]
struct AskPane {
    input: tui_kit::TextInput,
    exchanges: Vec<GuideExchange>,
    open: bool,
    scroll: tui_kit::ViewportScroll,
    follow: bool,
    body_rows: usize,
    /// Authoritative request state. Presentation may collapse while this stays
    /// true, preventing a second paid call until the worker settles.
    in_flight: bool,
    generation: u64,
}

struct GuideChat {
    ask: AskPane,
    dispatch: GuideDispatch,
    tokens: crate::app::harness_stream::OneShotTokenTracker,
    results: Option<mpsc::Receiver<(u64, Result<String, String>)>>,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    // This is refreshed by the live surface and intentionally remains the last
    // bounded snapshot when terminal ownership moves to the dashboard.
    context: String,
}

struct GuideExchange {
    question: String,
    generation: u64,
    answer: Option<String>,
}

/// Process-local conversation state which may move from a live run to its
/// dashboard. Dispatch configuration remains with the live run; a separately
/// launched dashboard never receives this handle.
#[derive(Clone)]
pub(crate) struct GuideChatHandle(Arc<Mutex<GuideChat>>);

impl GuideChatHandle {
    pub(crate) fn new(
        dispatch: GuideDispatch,
        tokens: crate::app::harness_stream::OneShotTokenTracker,
    ) -> Self {
        Self(Arc::new(Mutex::new(GuideChat {
            ask: AskPane::default(),
            dispatch,
            tokens,
            results: None,
            wake: None,
            context: String::new(),
        })))
    }

    #[cfg(test)]
    pub(crate) fn test_handle() -> Self {
        Self::new(
            Arc::new(|_, _| Ok("test answer".to_string())),
            Default::default(),
        )
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GuideChat> {
        self.0.lock().expect("guide chat lock poisoned")
    }

    pub(crate) fn poll_results(&self) -> bool {
        let mut chat = self.lock();
        let result = chat
            .results
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok());
        if let Some((generation, result)) = result {
            let changed = apply_ask_result(&mut chat.ask, generation, result);
            chat.results = None;
            return changed;
        }
        false
    }

    fn set_context(&self, context: String) {
        self.lock().context = context;
    }

    fn set_wake(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        self.lock().wake = Some(wake);
    }

    fn guide_tokens(&self) -> u64 {
        self.lock().tokens.snapshot().tokens.unwrap_or(0)
    }

    pub(crate) fn handle_key(&self, key: &KeyEvent, body_rows: usize) -> bool {
        let mut chat = self.lock();
        if let Some(consumed) = apply_ask_presentation_key(&mut chat.ask, key) {
            return consumed;
        }
        if matches!(
            key.code,
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown
        ) && let Some(delta) = tui_kit::scroll_key(key)
        {
            chat.ask.scroll.apply(delta, body_rows);
            chat.ask.follow = chat.ask.scroll.is_at_bottom(body_rows);
            return true;
        }
        match key.code {
            KeyCode::Enter => {
                if chat.ask.in_flight {
                    return true;
                }
                let question = chat.ask.input.text().trim().to_string();
                if question.is_empty() {
                    return true;
                }
                chat.tokens.begin_call();
                chat.ask.in_flight = true;
                chat.ask.generation = chat.ask.generation.wrapping_add(1);
                let generation = chat.ask.generation;
                chat.ask.exchanges.push(GuideExchange {
                    question: question.clone(),
                    generation,
                    answer: None,
                });
                chat.ask.input.reset();
                chat.ask.follow = true;
                let dispatch = Arc::clone(&chat.dispatch);
                let context = chat.context.clone();
                let wake = chat.wake.clone();
                let (sender, receiver) = mpsc::channel();
                chat.results = Some(receiver);
                std::thread::spawn(move || {
                    let result = dispatch(question, context).map_err(|error| error.to_string());
                    let _ = sender.send((generation, result));
                    if let Some(wake) = wake {
                        wake();
                    }
                });
                true
            }
            _ => matches!(
                chat.ask.input.handle_key(false, key),
                tui_kit::ModalOutcome::Pending
            ),
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.lock().ask.open
    }

    pub(crate) fn render(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let mut chat = self.lock();
        let ask = &mut chat.ask;
        if ask.open {
            let lines = ask_lines(ask);
            let input = ask.input.clone();
            let follow = ask.follow;
            ask.body_rows = tui_kit::conversation_body_rows(area);
            tui_kit::render_conversation_modal(
                frame,
                area,
                "Guide",
                &lines,
                &input,
                &mut ask.scroll,
                follow,
            );
        }
    }
}

#[derive(Clone)]
pub(crate) struct RunPanel {
    state: Arc<Mutex<RunPanelState>>,
    cadence: Arc<PanelCadence>,
    handoff: Arc<DashboardHandoff>,
}

/// The request, spawned dashboard, and teardown state are one lifecycle. This
/// prevents `close` from observing an empty handle while a launcher is between
/// taking a request and publishing its thread.
struct DashboardHandoff {
    state: Mutex<DashboardHandoffState>,
}

#[derive(Default)]
struct DashboardHandoffState {
    pending_session: Option<(String, Option<GuideChatHandle>)>,
    dashboard: Option<JoinHandle<()>>,
    closing: bool,
}

impl DashboardHandoff {
    fn request(&self, session_id: String, guide_chat: Option<GuideChatHandle>) {
        if let Ok(mut state) = self.state.lock()
            && !state.closing
            && state.pending_session.is_none()
            && state.dashboard.is_none()
        {
            state.pending_session = Some((session_id, guide_chat));
        }
    }

    fn drive(&self) {
        self.drive_with(|session_id, guide_chat| {
            std::thread::spawn(move || {
                if let Err(error) = crate::app::dashboard::run_for_session(session_id, guide_chat) {
                    eprintln!("dashboard: {error}");
                }
            })
        });
    }

    fn drive_with(&self, launch: impl FnOnce(String, Option<GuideChatHandle>) -> JoinHandle<()>) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some((session_id, guide_chat)) = (!state.closing && state.dashboard.is_none())
            .then(|| state.pending_session.take())
            .flatten()
        else {
            return;
        };
        state.dashboard = Some(launch(session_id, guide_chat));
    }

    fn close(&self) {
        self.close_after(|| {});
    }

    fn close_after(&self, before_lock: impl FnOnce()) {
        before_lock();
        let dashboard = self.state.lock().ok().and_then(|mut state| {
            state.closing = true;
            state.pending_session = None;
            state.dashboard.take()
        });
        if let Some(thread) = dashboard {
            let _ = thread.join();
        }
    }
}

struct HandoffDriver(Arc<DashboardHandoff>);

impl Drop for HandoffDriver {
    fn drop(&mut self) {
        self.0.drive();
    }
}

/// Cheap, lock-free admission control for 20ms harness observers and direct
/// input wakes. Timer-only repaint work remains limited to one second.
struct PanelCadence {
    input_generation: Arc<AtomicU64>,
    handled_generation: Arc<AtomicU64>,
    next_timer_paint_ms: AtomicU64,
    next_resize_retry_ms: AtomicU64,
    started: Instant,
}

impl PanelCadence {
    fn new(input_generation: Arc<AtomicU64>, handled_generation: Arc<AtomicU64>) -> Self {
        Self {
            input_generation,
            handled_generation,
            next_timer_paint_ms: AtomicU64::new(0),
            next_resize_retry_ms: AtomicU64::new(u64::MAX),
            started: Instant::now(),
        }
    }

    fn should_run(&self) -> bool {
        self.input_generation.load(Ordering::Acquire)
            != self.handled_generation.load(Ordering::Acquire)
            || self.now_ms() >= self.next_timer_paint_ms.load(Ordering::Acquire)
            || self.now_ms() >= self.next_resize_retry_ms.load(Ordering::Acquire)
    }

    fn observe(&self, outcome: TickOutcome) {
        self.handled_generation
            .fetch_max(outcome.consumed_generation, Ordering::Release);
        let now = self.now_ms();
        if outcome.disable_timer {
            self.next_timer_paint_ms.store(u64::MAX, Ordering::Release);
        }
        self.next_resize_retry_ms.store(
            if outcome.resize_retry {
                now.saturating_add(50)
            } else {
                u64::MAX
            },
            Ordering::Release,
        );
    }

    fn painted(&self) {
        self.next_timer_paint_ms
            .store(self.now_ms().saturating_add(1_000), Ordering::Release);
    }

    fn inactive(&self) {
        self.next_timer_paint_ms.store(u64::MAX, Ordering::Release);
        self.next_resize_retry_ms.store(u64::MAX, Ordering::Release);
    }

    fn now_ms(&self) -> u64 {
        self.started
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
}

#[derive(Default)]
struct TickOutcome {
    consumed_generation: u64,
    resize_retry: bool,
    disable_timer: bool,
}

struct RunPanelState {
    cadence: Arc<PanelCadence>,
    input_generation: Arc<AtomicU64>,
    handled_generation: Arc<AtomicU64>,
    repaint: RatatuiPane,
    trait_name: String,
    trait_ref: ctx_traits_core::Trait,
    plan: ctx_traits_core::procedure::run::Plan,
    session: ctx_traits_core::procedure::session::Session,
    active_key: Option<String>,
    active_started: Option<(String, Instant)>,
    finished_durations: BTreeMap<String, Duration>,
    output_tokens: BTreeMap<String, u64>,
    /// Presentation-only aggregates for loop/for-each containers, keyed by
    /// `loop_container_key` (the container's own structural position path,
    /// iteration-independent so totals survive rollover). Kept separate from
    /// `finished_durations`/`output_tokens` above so `RunHeader`'s sum over
    /// the per-step token map never double-counts a loop's own children.
    loop_elapsed: BTreeMap<String, Duration>,
    loop_output_tokens: BTreeMap<String, u64>,
    /// P445: drive-wide narrator output tokens, tracked separately from the
    /// work agent's per-step `output_tokens` above (narration is not tied to
    /// one step, and is presentation-only — it never gates or joins the
    /// drive).
    narrator_tokens: u64,
    run_started: Instant,
    last_timer_paint: Instant,
    /// Window focus as of the last painted frame. A focus change alters how
    /// the frame is drawn (the whole buffer is dimmed while unfocused)
    /// without changing a byte of its content, so it has to be tracked here
    /// to be recognised as a reason to repaint — see `tick_locked`.
    last_focus: bool,
    live: tui::LiveLine,
    /// P470: completed steps' P455 finished-step summaries, keyed on the same
    /// `structural_step_key` a story row and its tree row both agree on (see
    /// [`step_key`]). Derived into each `RunStep::summary` by [`run_view`] —
    /// never rendered directly.
    step_summaries: BTreeMap<String, String>,
    /// Elapsed-since-pane-start stamp for each entry in `step_summaries`,
    /// recorded at the moment the summary landed (which may be well after
    /// the step itself finished, since the narrator call is async).
    step_summary_at: BTreeMap<String, Duration>,
    /// P470: the CURRENT step's own message/thinking flow only — narrations
    /// in narrated mode, drained model-text deltas in passthrough mode.
    /// Cleared on every active-key transition; ring-capped so a chatty step
    /// cannot grow without bound.
    current_stream: VecDeque<StreamRow>,
    view: RunView,
    scrolls: PaneScrolls,
    progress_follow: bool,
    journey_follow: bool,
    history_follow: bool,
    current_follow: bool,
    focus: FocusRing,
    /// Keys drained by the existing input pump. They are replayed inside the
    /// draw pass after the current tree and inner pane rectangles are known.
    pending_keys: Vec<KeyEvent>,
    /// `q`'s confirm-quit dialog (P551). While open, every drained key routes
    /// here instead of into `pending_keys` — the pane's own focus trap.
    modal: Option<tui_kit::Modal>,
    /// P244: the tree lines drawn by the LAST completed render, cached so
    /// [`RunPanel::close`] can commit exactly the last-drawn frame to
    /// scrollback via [`RatatuiPane::commit_inline_scrollback`] — a no-op
    /// for the alt-screen panes this state also backs (dashboard, demo,
    /// editor never call `close`'s inline path). Deliberately NOT
    /// re-derived from `render_ledger_run_view` (`:1022`), which produces a
    /// different frame (no live timing/narration) — this is the actual last
    /// frame the pane drew.
    last_tree_lines: Vec<tui::Line>,
    /// P549: folded merge stage rows, updated by [`RunPanel::merge_event`]
    /// and copied verbatim into `view.merge_rows` on every rebuild (never
    /// re-derived from `trait_ref`/`plan`/`session`, which know nothing
    /// about a merge span).
    merge_rows: Vec<MergeRowView>,
    /// P552 one-time narrator session title, set once by
    /// [`RunPanel::set_title`] after a successful title dispatch — `None`
    /// before success and permanently for a missing-narrator/failed/killed
    /// attempt.
    title_state: Option<ctx_traits_core::procedure::session::SessionTitleState>,
    ask: Option<GuideChatHandle>,
    guide_ledger_path: Option<camino::Utf8PathBuf>,
    /// Detached guide workers wake through this non-owning handle after they
    /// queue a result, so a completed answer does not wait for another event.
    wake_state: Weak<Mutex<RunPanelState>>,
    /// The same handoff the owning `RunPanel` holds. A detached worker's wake
    /// has to pump it too: `tick_weak` drives the handoff after ticking, and
    /// a worker that could only reach the panel's weak state would tick
    /// without ever driving a dashboard the tick may have queued.
    handoff: Arc<DashboardHandoff>,
    /// P081: true only for a dashboard-attach observer panel — one that
    /// polls a live run's own ledger rather than being fed by a drive loop.
    /// Folded into [`has_running_work`] (an observer's `active_started` is
    /// `None` between ledger polls, but its clock must still tick) and into
    /// [`poll_and_apply_keys`]'s ask-refusal branch.
    observer: bool,
    /// The observed run's own ledger path — `Some` only for an observer
    /// panel, read by [`has_running_work`]'s liveness probe and by
    /// [`RunPanel::refresh_from_ledger`]'s terminal check.
    ledger_path: Option<camino::Utf8PathBuf>,
    /// The ledger's own persisted guide-token count, used as `rebuild_view`'s
    /// header total only while no live `ask` handle is installed — an
    /// observer with an inherited handle reads its live counter instead.
    ledger_guide_tokens: u64,
    /// Set once [`RunPanel::refresh_from_ledger`] observes a terminal session
    /// and has already surfaced the finished note, so a later poll before
    /// [`RunPanel::close`] takes effect does not push the note twice.
    observer_finished: bool,
    /// P081: the observer's ask-refusal notice, retained OUTSIDE
    /// `current_stream` because [`apply_ledger_seed`] rebuilds that field
    /// wholesale from the sidecar on every poll — a refusal pushed only into
    /// `current_stream` would vanish after at most one `RELOAD_INTERVAL`,
    /// silently violating the "never silence" rule. Re-appended by
    /// [`apply_ledger_seed`] after it rebuilds `current_stream`; cleared only
    /// by a fresh ask attempt or an inherited handle making ask usable.
    observer_notice: Option<StreamRow>,
}

/// One row of the CURRENT step's verbatim message/thinking stream.
#[derive(Clone)]
struct StreamRow {
    /// Elapsed since the run pane started, rendered as `HH:MM:SS`.
    at: Duration,
    kind: StreamRowKind,
    text: String,
}

#[derive(Clone, Copy)]
enum StreamRowKind {
    /// A narrator summary line (narrated mode).
    Narration,
    /// A drained raw model-text delta (narrator-free/passthrough mode).
    ModelText,
}

const PROGRESS_PANE: PaneId = "progress";
const JOURNEY_PANE: PaneId = "journey";
const HISTORY_PANE: PaneId = "history";
const CURRENT_PANE: PaneId = "current";
/// A bordered pane needs two border rows plus one content row.
const HISTORY_MIN_OUTER_ROWS: u16 = 3;
const CURRENT_MIN_OUTER_ROWS: u16 = 8;
/// P552: below this width every populated pane stacks vertically instead of
/// the 2x2 (or narrower 2-pane) grid — unchanged from the pre-P552 threshold;
/// re-evaluated against the extra journey pane/borders and confirmed still
/// wide enough for a 60/40 two-column split to stay usable.
const NARROW_WIDTH_THRESHOLD: u16 = 109;
const ASK_FOOTER_HINT: &str = "[?] ask · [up/down] scroll · [pg] page · [home/end] jump · [tab] pane · [d] dash · [q] exit · [ctrl-c] kill";

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

const CURRENT_STREAM_CAP: usize = 400;
/// The ask strip is intentionally one display row for the answer. Keep its
/// presentation-only state bounded as well, so a verbose model cannot crowd
/// the live 2x2 view or retain an unbounded response in memory.
const MAX_GUIDE_ANSWER_CHARS: usize = 600;

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
struct RunView {
    header: RunHeader,
    steps: Vec<RunStep>,
    history: Vec<HistoryStep>,
    narration: Option<RunNarration>,
    outputs: Vec<RunOutput>,
    /// P549: merge stage rows, rendered directly after `steps` in the same
    /// pane the run's own journey draws in — empty for every run that never
    /// hands a [`RunPanel`] into a merge span. Carried on `RunPanelState`
    /// itself (not derived by [`run_view`]) since a merge event has nothing
    /// to do with the plan/session `run_view` otherwise projects from.
    merge_rows: Vec<MergeRowView>,
}

/// One accepted execution as recorded by the ledger. Unlike `RunStep`, this
/// is deliberately not a plan projection: a loop body can occur many times.
#[derive(Debug, Clone)]
struct HistoryStep {
    label: String,
    kind: Option<ctx_traits_core::procedure::run::PlannedSequenceKind>,
    outcome: Option<HistoryOutcome>,
    elapsed: Option<Duration>,
    output_tokens: Option<u64>,
    summary: Option<String>,
    summary_at: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HistoryOutcome {
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
struct MergeRowView {
    frame_id: String,
    label: String,
    state: MergeRowState,
    detail: Option<String>,
    started: Instant,
    finished: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeRowState {
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone)]
struct RunHeader {
    session_id: String,
    run_id: String,
    input: String,
    harnesses: String,
    done: usize,
    total: usize,
    phase: String,
    completed: bool,
    stopped: Option<String>,
    stop_detail: Option<String>,
    state_digest: String,
    harness_count: usize,
    elapsed: Option<Duration>,
    output_tokens: Option<u64>,
    /// P445: drive-wide narrator output tokens, shown distinctly from
    /// `output_tokens` (the work agent's total).
    narrator_tokens: Option<u64>,
    guide_tokens: Option<u64>,
    structured_count: usize,
    structured_label: Option<String>,
    structured_verdict: Option<String>,
}

#[derive(Debug, Clone)]
struct RunStep {
    key: String,
    label: String,
    role: String,
    harness: Option<String>,
    tags: Vec<String>,
    status: String,
    state: StepState,
    active: bool,
    counts_progress: bool,
    #[allow(dead_code)] // Retained plan facts; compact journey rows no longer render ports.
    inputs: Vec<PortSlug>,
    outputs: Vec<PortSlug>,
    elapsed: Option<Duration>,
    output_tokens: Option<u64>,
    /// `Some(loop_container_key)` when this step is a loop/for-each
    /// container; its elapsed/tokens are populated from the presentation
    /// loop aggregates instead of the generic per-step maps.
    loop_key: Option<String>,
    position_path: Vec<ctx_traits_core::procedure::runtime::PathSegment>,
    run_index: usize,
    structured_count: usize,
    /// P470: this step's P455 finished-step summary, when one landed —
    /// `None` for a narrator-free run, a failed/timed-out/disabled
    /// narration, or a step that hasn't produced one yet. Drives the story
    /// column's one-line-per-completed-step compression; the facts fallback
    /// (`label · elapsed · tokens`) is used in its absence.
    summary: Option<String>,
    /// Elapsed-since-pane-start stamp for `summary`, `None` alongside it.
    summary_at: Option<Duration>,
}

#[derive(Clone)]
pub(crate) struct JourneyRow(JourneyRowKind);

#[allow(dead_code)] // Used by dashboard fixtures; production rows originate here.
pub(crate) fn journey_line(line: tui::Line) -> JourneyRow {
    JourneyRow(JourneyRowKind::Line(line))
}

#[derive(Clone)]
enum JourneyRowKind {
    Step(Box<RunStep>),
    Line(tui::Line),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepState {
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
enum Activity {
    /// The item the current frame executes.
    Current,
    /// A container somewhere on the active path (a loop mid-iteration).
    Ancestor,
    Idle,
}

#[derive(Debug, Clone)]
struct PortSlug {
    slug: String,
    #[allow(dead_code)] // Retained port satisfaction fact; no longer rendered in journey rows.
    satisfied: bool,
}

#[derive(Debug, Clone)]
struct RunNarration {
    label: String,
    text: String,
    muted: bool,
    finished: bool,
}

#[derive(Debug, Clone)]
struct RunOutput {
    slug: String,
    status: String,
    accepted: bool,
}

struct PresentationState<'a> {
    active_started: &'a Option<(String, Instant)>,
    finished_durations: &'a BTreeMap<String, Duration>,
    output_tokens: &'a BTreeMap<String, u64>,
    loop_elapsed: &'a BTreeMap<String, Duration>,
    loop_output_tokens: &'a BTreeMap<String, u64>,
    step_summaries: &'a BTreeMap<String, String>,
    step_summary_at: &'a BTreeMap<String, Duration>,
    narrator_tokens: u64,
    guide_tokens: u64,
    run_started: Instant,
}

impl RunPanel {
    pub(crate) fn new(
        trait_name: String,
        trait_ref: ctx_traits_core::Trait,
        plan: ctx_traits_core::procedure::run::Plan,
        session: ctx_traits_core::procedure::session::Session,
    ) -> std::io::Result<Self> {
        Ok(Self::new_with_pane(
            trait_name,
            trait_ref,
            plan,
            session,
            RatatuiPane::new_inline()?,
        ))
    }

    pub(crate) fn new_with_pane(
        trait_name: String,
        trait_ref: ctx_traits_core::Trait,
        plan: ctx_traits_core::procedure::run::Plan,
        session: ctx_traits_core::procedure::session::Session,
        repaint: RatatuiPane,
    ) -> Self {
        let input_generation = repaint.input_generation();
        let handled_generation = Arc::new(AtomicU64::new(0));
        let cadence = Arc::new(PanelCadence::new(
            Arc::clone(&input_generation),
            Arc::clone(&handled_generation),
        ));
        let active_key = active_key(&session);
        let now = Instant::now();
        let active_started = active_key.as_ref().map(|key| (key.clone(), now));
        // A resumed drive whose title already resolved in an earlier
        // invocation shows it immediately — never re-dispatched, per
        // `SessionTitleState`'s "resolved title is read-only" contract.
        let title_state = session.provenance.session_title.clone();
        let view = run_view(
            &trait_ref,
            &plan,
            &session,
            None,
            PresentationState {
                active_started: &active_started,
                finished_durations: &BTreeMap::new(),
                output_tokens: &BTreeMap::new(),
                loop_elapsed: &BTreeMap::new(),
                loop_output_tokens: &BTreeMap::new(),
                step_summaries: &BTreeMap::new(),
                step_summary_at: &BTreeMap::new(),
                narrator_tokens: 0,
                guide_tokens: 0,
                run_started: now,
            },
        );
        let handoff = Arc::new(DashboardHandoff {
            state: Mutex::new(DashboardHandoffState::default()),
        });
        let state = Arc::new(Mutex::new(RunPanelState {
            cadence: Arc::clone(&cadence),
            input_generation,
            handled_generation,
            repaint,
            trait_name,
            trait_ref,
            plan,
            session,
            active_key,
            active_started,
            finished_durations: BTreeMap::new(),
            output_tokens: BTreeMap::new(),
            loop_elapsed: BTreeMap::new(),
            loop_output_tokens: BTreeMap::new(),
            narrator_tokens: 0,
            run_started: now,
            last_timer_paint: now,
            // Panes start focused, matching `PumpControl`'s own default for a
            // terminal that never reports focus at all.
            last_focus: true,
            live: tui::LiveLine::default(),
            step_summaries: BTreeMap::new(),
            step_summary_at: BTreeMap::new(),
            current_stream: VecDeque::new(),
            view,
            scrolls: PaneScrolls::new(),
            progress_follow: true,
            journey_follow: true,
            history_follow: true,
            current_follow: true,
            // The step list is what a watcher reads first, so it holds focus by
            // default; tab cycles to the activity panes.
            focus: FocusRing::new(vec![PROGRESS_PANE]),
            pending_keys: Vec::new(),
            modal: None,
            last_tree_lines: Vec::new(),
            merge_rows: Vec::new(),
            title_state,
            ask: None,
            guide_ledger_path: None,
            wake_state: Weak::new(),
            handoff: Arc::clone(&handoff),
            observer: false,
            ledger_path: None,
            ledger_guide_tokens: 0,
            observer_finished: false,
            observer_notice: None,
        }));
        let panel = Self {
            state: Arc::clone(&state),
            cadence: Arc::clone(&cadence),
            handoff,
        };
        let weak_state = Arc::downgrade(&state);
        let wake_cadence = Arc::clone(&cadence);
        let wake_handoff = Arc::clone(&panel.handoff);
        if let Ok(mut state) = state.lock() {
            state.wake_state = weak_state.clone();
            state.repaint.install_input_wake(Arc::new(move || {
                tick_weak(&weak_state, &wake_cadence, &wake_handoff);
            }));
        }
        panel.render();
        panel.cadence.painted();
        panel
    }

    /// P081: the dashboard-attach mirror of [`Self::new`] — the SAME shared
    /// renderer a live `--progress tui` run builds, driven purely as an
    /// observer of `session`'s own ledger (never a drive loop, never a second
    /// `GuideDispatch`). Seeds every presentation field
    /// [`ledger_presentation_seed`] can supply from the ledger + activity
    /// sidecar alone (back-dated `run_started`, token maps, step summaries,
    /// the CURRENT pane's latest-frame rows) — the same derivation
    /// [`render_ledger_run_view`] uses for the dashboard's list-visible
    /// preview, so an attach's very first frame matches what a reload of the
    /// preview would already have shown. The handoff is constructed
    /// pre-closed (`closing: true`) so a `d` press inside this observer
    /// cannot spawn a second dashboard thread — see [`Self::presentation_closed`].
    pub(crate) fn new_observer(
        trait_name: String,
        trait_ref: ctx_traits_core::Trait,
        plan: ctx_traits_core::procedure::run::Plan,
        session: ctx_traits_core::procedure::session::Session,
        ledger_path: camino::Utf8PathBuf,
        pane: RatatuiPane,
    ) -> Self {
        let panel = Self::new_with_pane(trait_name, trait_ref, plan, session, pane);
        if let Ok(mut handoff_state) = panel.handoff.state.lock() {
            handoff_state.closing = true;
        }
        if let Ok(mut state) = panel.state.lock() {
            state.observer = true;
            state.ledger_path = Some(ledger_path.clone());
            apply_ledger_seed(&mut state, &ledger_path);
            rebuild_view(&mut state, None);
            render_locked(&mut state);
        }
        panel.cadence.painted();
        panel
    }

    /// P081: the observer's counterpart of the drive loop's incremental
    /// `add_output_tokens`/`push_summary`/`refresh` feeding — re-derives the
    /// whole ledger/sidecar presentation from persisted truth on every call
    /// (never accumulated in-process) via the same [`ledger_presentation_seed`]
    /// [`Self::new_observer`] seeds from, then goes through the ordinary
    /// [`rebuild_view`]/[`render_locked`] pipeline. A newly terminal session
    /// renders its final frame, surfaces a [`Self::note`]-equivalent stream
    /// row exactly once, and closes this presentation — the caller (the
    /// dashboard's attach loop) observes that through [`Self::presentation_closed`]
    /// and [`Self::observer_finished`].
    pub(crate) fn refresh_from_ledger(
        &self,
        session: &ctx_traits_core::procedure::session::Session,
        ledger_path: &camino::Utf8Path,
    ) {
        let _handoff = self.handoff_driver();
        let terminal = super::dashboard::session_is_terminal(session, ledger_path);
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.session = session.clone();
        state.title_state = session.provenance.session_title.clone();
        apply_ledger_seed(&mut state, ledger_path);
        if terminal && !state.observer_finished {
            state.observer_finished = true;
            push_stream_row(
                &mut state,
                StreamRowKind::Narration,
                "run finished — returning to dashboard".to_string(),
            );
        }
        rebuild_view(&mut state, None);
        render_locked(&mut state);
        drop(state);
        if terminal {
            self.close();
        }
    }

    /// P081: `true` once this observer's own pane can no longer draw — either
    /// the run finished (see [`Self::refresh_from_ledger`]) or the user
    /// pressed `d`/confirmed `q` (both routed through the ordinary
    /// [`poll_and_apply_keys`] key handling, unchanged from the live view).
    /// The dashboard's attach loop polls this to know when to tear this
    /// pane down and rebuild its own.
    pub(crate) fn presentation_closed(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.repaint.detached())
            .unwrap_or(true)
    }

    /// P081: `true` once [`Self::refresh_from_ledger`] observed the run reach
    /// a terminal state while this observer was attached — the dashboard's
    /// attach loop reads this (after `presentation_closed`) to decide whether
    /// its own return message reports a finished run or an ordinary detach.
    pub(crate) fn observer_finished(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.observer_finished)
            .unwrap_or(false)
    }

    /// Deterministically restore the terminal, regardless of how many other
    /// clones of this panel are still alive. Teardown must NEVER rely on the
    /// last `Arc` dropping: per-frame narrator workers are detached threads
    /// that hold panel clones inside their closures and may sit blocked in a
    /// model call past the end of the drive — at process exit those threads
    /// are killed without running destructors, which is exactly how raw mode
    /// and the alternate screen used to outlive the process. Idempotent; any
    /// late render from a lingering clone no-ops on the detached pane.
    pub(crate) fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            let tree_lines = state.last_tree_lines.clone();
            let _ = state.repaint.commit_inline_scrollback(&tree_lines);
            state.repaint.quit();
            state.cadence.inactive();
        }
        self.handoff.close();
    }

    fn handoff_driver(&self) -> HandoffDriver {
        HandoffDriver(Arc::clone(&self.handoff))
    }

    /// Install the optional, explicitly configured guide. The pane keeps only
    /// its current request state; the dispatcher receives a bounded display
    /// snapshot and has no access to the session mutation path.
    pub(crate) fn set_guide(
        &self,
        dispatch: GuideDispatch,
        tokens: crate::app::harness_stream::OneShotTokenTracker,
        ledger_path: camino::Utf8PathBuf,
    ) {
        self.install_guide_handle(GuideChatHandle::new(dispatch, tokens), ledger_path);
    }

    /// P081: shared by [`Self::set_guide`] (a fresh dispatcher for a driven
    /// run) and an attach observer inheriting the SAME in-process
    /// [`GuideChatHandle`] a `d`-handoff already holds — the only guide seat
    /// an observer is ever permitted to use, per the implementation draft's
    /// "ask: one deliberate rule" (never a second writer against the run).
    pub(crate) fn install_guide_handle(
        &self,
        chat: GuideChatHandle,
        ledger_path: camino::Utf8PathBuf,
    ) {
        if let Ok(mut state) = self.state.lock() {
            let weak_state = state.wake_state.clone();
            let cadence = Arc::clone(&state.cadence);
            let handoff = Arc::clone(&state.handoff);
            let input_generation = Arc::clone(&state.input_generation);
            chat.set_wake(Arc::new(move || {
                input_generation.fetch_add(1, Ordering::Release);
                tick_weak(&weak_state, &cadence, &handoff);
            }));
            state.ask = Some(chat);
            state.guide_ledger_path = Some(ledger_path);
            render_locked(&mut state);
        }
    }

    /// The sole active-key transition detector (P455): when the active step
    /// changes, returns presentation context for the step that just finished
    /// — its display label/role, elapsed time, and observed work tokens — so
    /// the caller can request exactly one narrator step-summary call. `None`
    /// on every repaint that is not itself a transition, and on a transition
    /// whose prior step never entered `state.view` (e.g. this panel's very
    /// first refresh).
    pub(crate) fn refresh(
        &self,
        session: &ctx_traits_core::procedure::session::Session,
    ) -> Option<CompletedStepContext> {
        let _handoff = self.handoff_driver();
        let Ok(mut state) = self.state.lock() else {
            return None;
        };
        let active_key = active_key(session);
        let active_changed = state.active_key != active_key;
        let mut completed = None;
        if active_changed {
            if let Some((key, started)) = state.active_started.take() {
                let elapsed = started.elapsed();
                // Credit every loop this key's item was running inside, using
                // the pre-transition session so a rollover still attributes
                // the interval to the loop it belongs to.
                for loop_key in active_loop_container_keys(&state.session) {
                    *state.loop_elapsed.entry(loop_key).or_default() += elapsed;
                }
                // Captured from the last-rendered view (still the pre-
                // transition step) before `rebuild_view` below replaces it.
                let completed_step = state
                    .view
                    .steps
                    .iter()
                    .find(|step| step.active)
                    .map(|step| (step.key.clone(), step.label.clone(), step.role.clone()));
                let work_tokens = state.output_tokens.get(&key).copied();
                completed = completed_step.map(|(key, label, role)| CompletedStepContext {
                    key,
                    label,
                    role,
                    elapsed,
                    work_tokens,
                });
                state.finished_durations.insert(key, elapsed);
                // P470: the CURRENT step's verbatim stream is exactly that —
                // the step now ending is done, its story row is the P455
                // summary or the facts fallback, never its raw thinking ticks.
                state.current_stream.clear();
                state.scrolls.reset(CURRENT_PANE);
                state.current_follow = true;
            }
            // The accepted final step clears the active key. Keep its finished
            // narration through the terminal repaint instead of replacing it
            // with the initial live line.
            if session.status != ctx_traits_core::procedure::session::Status::Completed {
                state.live = tui::LiveLine::default();
            }
            state.active_key = active_key;
            state.active_started = state
                .active_key
                .as_ref()
                .map(|key| (key.clone(), Instant::now()));
        }
        state.session = session.clone();
        // Frame refreshes read the authoritative ledger lifecycle. This also
        // replaces an abandoned final in-flight claim with its terminal state.
        state.title_state = session.provenance.session_title.clone();
        let display = state.live.current_display();
        let mut narration = narration_for(&state.session, display.clone());
        if display.finished
            && let (Some(current), Some(previous)) =
                (narration.as_mut(), state.view.narration.as_ref())
        {
            current.label.clone_from(&previous.label);
        }
        rebuild_view(&mut state, narration);
        if active_changed && let Some(text) = entered_step_text(&state.view, "Initialization") {
            push_stream_row(&mut state, StreamRowKind::Narration, text);
        }
        render_locked(&mut state);
        mark_timer_painted(&mut state);
        completed
    }

    pub(crate) fn push_bytes(&self, chunk: &[u8]) {
        let _handoff = self.handoff_driver();
        if chunk.is_empty() {
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let before = state.live.current_display();
        let deltas = state.live.push_bytes(chunk);
        for delta in &deltas {
            push_stream_row(&mut state, StreamRowKind::ModelText, delta.clone());
        }
        let after = state.live.current_display();
        // A chunk that drains no delta and doesn't change the live display
        // (a partial NDJSON line, a tool-use event) is a genuine no-op —
        // restores the hot-path early return the previous single-delta
        // `latest_stream_text` path had, on the chattiest passthrough surface.
        if deltas.is_empty() && before == after {
            return;
        }
        if before != after {
            update_live_display(&mut state, after);
        }
        render_locked(&mut state);
    }

    /// P496: update the between-narrations token pill shown while the live
    /// line still holds the initialization fallback. Presentation only.
    pub(crate) fn set_thinking_tokens(&self, tokens: u64) {
        let _handoff = self.handoff_driver();
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let before = state.live.current_display();
        state.live.set_thinking_tokens(tokens);
        let after = state.live.current_display();
        if before == after {
            return;
        }
        update_live_display(&mut state, after);
        render_locked(&mut state);
    }

    pub(crate) fn push_summary(&self, summary: String) {
        let _handoff = self.handoff_driver();
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        // P470: every accepted narration is appended verbatim to the CURRENT
        // step's own stream, while the live line keeps its P330
        // replace-in-place behavior.
        push_stream_row(&mut state, StreamRowKind::Narration, summary.clone());
        let before = state.live.current_display();
        state.live.push_summary(summary);
        let after = state.live.current_display();
        if before != after {
            update_live_display(&mut state, after);
        }
        render_locked(&mut state);
    }

    /// P551: append a one-line progress note (e.g. worktree setup activity)
    /// to the CURRENT step's stream without touching the live line — used
    /// while the pane is up before any step has actually started, so setup
    /// activity is visible instead of the pane sitting frozen.
    pub(crate) fn note(&self, text: String) {
        let _handoff = self.handoff_driver();
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        push_stream_row(&mut state, StreamRowKind::Narration, text);
        render_locked(&mut state);
    }

    /// P455: record a successful finished-step narration as this step's own
    /// one-line story summary, keyed on the step's own structural key so an
    /// async landing (typically after the next step has already started)
    /// still joins the right step's row. A silent no-op is the caller's job
    /// for a failed/timed-out/disabled narration; only a successful summary
    /// ever reaches this method.
    pub(crate) fn push_step_summary(&self, context: &CompletedStepContext, summary: String) {
        let _handoff = self.handoff_driver();
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let at = state.run_started.elapsed();
        state.step_summary_at.insert(context.key.clone(), at);
        state.step_summaries.insert(context.key.clone(), summary);
        let narration = state.view.narration.clone();
        rebuild_view(&mut state, narration);
        render_locked(&mut state);
    }

    pub(crate) fn finish_live(&self, summary: &str) {
        let _handoff = self.handoff_driver();
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let before = state.live.current_display();
        state.live.finish(summary);
        let after = state.live.current_display();
        if before == after {
            return;
        }
        update_live_display(&mut state, after);
        render_locked(&mut state);
    }

    pub(crate) fn tick(&self) {
        let _handoff = self.handoff_driver();
        if !self.cadence.should_run() {
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let outcome = tick_locked(&mut state);
        drop(state);
        self.cadence.observe(outcome);
    }

    pub(crate) fn add_output_tokens(&self, tokens: u64) {
        let _handoff = self.handoff_driver();
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(key) = state.active_key.clone() else {
            return;
        };
        if tokens == 0 {
            return;
        }
        let total = state.output_tokens.entry(key).or_default();
        *total = total.saturating_add(tokens);
        for loop_key in active_loop_container_keys(&state.session) {
            let loop_total = state.loop_output_tokens.entry(loop_key).or_default();
            *loop_total = loop_total.saturating_add(tokens);
        }
        if state.last_timer_paint.elapsed() < Duration::from_secs(1) {
            return;
        }
        mark_timer_painted(&mut state);
        let narration = state.view.narration.clone();
        rebuild_view(&mut state, narration);
        render_locked(&mut state);
    }

    /// P445: fold one finished narrator call's observed output-token delta
    /// into the drive-wide narrator total shown in the header, distinct from
    /// `add_output_tokens`'s per-step work-agent total above.
    pub(crate) fn add_narrator_tokens(&self, tokens: u64) {
        let _handoff = self.handoff_driver();
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if tokens == 0 {
            return;
        }
        state.narrator_tokens = state.narrator_tokens.saturating_add(tokens);
        if state.last_timer_paint.elapsed() < Duration::from_secs(1) {
            return;
        }
        mark_timer_painted(&mut state);
        let narration = state.view.narration.clone();
        rebuild_view(&mut state, narration);
        render_locked(&mut state);
    }

    /// P549: fold one merge-progress [`ActivityEvent`] into `merge_rows`,
    /// keyed by `event.frame_id` (`merge:<stage-slug>`, `merge_story`'s own
    /// wording — see [`MergeRowView`]). Presentation-only: never reads or
    /// writes the ledger. `ActivityKind::Stalled` at the exact frame_id
    /// `"merge:lock"` is the lock-wait point (nonterminal — see
    /// `merge_story::activity_event`'s doc comment); `Stalled` at any other
    /// frame_id is a terminal park/failure outcome for that stage.
    pub(crate) fn merge_event(&self, event: &ActivityEvent) {
        let _handoff = self.handoff_driver();
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        fold_merge_event(&mut state.merge_rows, event, Instant::now());
        let narration = state.view.narration.clone();
        rebuild_view(&mut state, narration);
        render_locked(&mut state);
    }

    /// Update the persisted title lifecycle without changing the row geometry.
    pub(crate) fn set_title_state(
        &self,
        title_state: Option<ctx_traits_core::procedure::session::SessionTitleState>,
    ) {
        let _handoff = self.handoff_driver();
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.title_state = title_state;
        render_locked(&mut state);
    }

    fn render(&self) {
        let _handoff = self.handoff_driver();
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        render_locked(&mut state);
    }
    /// A tick observer callable from any thread (including a concurrent
    /// wave's worker threads or a blocking command call), so quit input stays
    /// responsive across every supported blocking dispatch path — not just
    /// the streamed single-harness call that drives [`Self::tick`] via its
    /// own tick loop.
    pub(crate) fn tick_observer(&self) -> ctx_traits_io::harness::TickObserver {
        let panel = self.clone();
        std::sync::Arc::new(move || panel.tick())
    }
}

fn tick_weak(
    state: &Weak<Mutex<RunPanelState>>,
    cadence: &PanelCadence,
    handoff: &Arc<DashboardHandoff>,
) {
    if !cadence.should_run() {
        return;
    }
    let Some(panel_state) = state.upgrade() else {
        return;
    };
    let Ok(mut locked_state) = panel_state.lock() else {
        return;
    };
    let outcome = tick_locked(&mut locked_state);
    drop(locked_state);
    cadence.observe(outcome);
    handoff.drive();
}

fn tick_locked(state: &mut RunPanelState) -> TickOutcome {
    let consumed_generation = state.input_generation.load(Ordering::Acquire);
    let resized = state.repaint.apply_resize();
    // A focus change wakes this tick (`set_focus` bumps the input generation)
    // but used to count as no change at all, because only keys and resizes
    // did. The tick therefore ran and returned WITHOUT painting, and the dim
    // appeared only when something else forced a frame: the 1s timer paint,
    // the 500ms presentation ticker, or the next keystroke. On an idle run it
    // never appeared, since no running work means no timer paint is
    // scheduled. Recorded before the early return below so a change is
    // consumed exactly once whether or not this tick goes on to paint.
    let focus_now = state.repaint.focused();
    let focus_changed = focus_now != state.last_focus;
    state.last_focus = focus_now;
    let changed = poll_and_apply_keys(state) || resized || focus_changed;
    if state.repaint.detached() {
        return TickOutcome {
            consumed_generation,
            disable_timer: true,
            ..TickOutcome::default()
        };
    }
    let resize_retry = state.repaint.resize_pending();
    // P549: a merge span's gate/lock/merger waits pump no harness observer
    // (unlike a driven frame's `tick_observer`), so a Running merge row is
    // the ONLY other source — besides an active step — of an elapsed clock
    // that must keep ticking without a discrete event to trigger it.
    let has_running_work = has_running_work(state);
    if !tick_must_paint(changed, has_running_work, state.last_timer_paint.elapsed()) {
        return TickOutcome {
            consumed_generation,
            resize_retry,
            disable_timer: !has_running_work,
        };
    }
    mark_timer_painted(state);
    let narration = state.view.narration.clone();
    rebuild_view(state, narration);
    render_locked(state);
    TickOutcome {
        consumed_generation,
        resize_retry,
        ..TickOutcome::default()
    }
}

/// Whether a tick has to paint. Stated once, and as a free function so it can
/// be tested without a terminal.
///
/// `changed` is anything that alters what the frame should look like: a key,
/// a resize, and — the case this was extracted for — a window focus change,
/// which alters the whole frame's styling without altering a byte of its
/// content. Everything else rides the once-a-second clock refresh, and only
/// while work is actually running; an idle pane schedules no timer paint at
/// all, which is why a change it does not recognise can go unpainted
/// indefinitely rather than merely late.
fn tick_must_paint(changed: bool, has_running_work: bool, since_timer_paint: Duration) -> bool {
    changed || (has_running_work && since_timer_paint >= Duration::from_secs(1))
}

fn has_running_work(state: &RunPanelState) -> bool {
    state.active_started.is_some()
        // Presentation may be collapsed or reopened while the detached
        // request remains authoritative; keep polling until it settles.
        || state.ask.as_ref().is_some_and(|ask| ask.lock().ask.in_flight)
        || state
            .merge_rows
            .iter()
            .any(|row| row.state == MergeRowState::Running)
        // P081: an observer's ledger snapshot carries no in-process active
        // step timer at all (`active_started` is `None` between ledger
        // polls) — its own repaint must stay admitted for as long as the
        // observed run itself is non-terminal, or the header clock freezes
        // on a between-steps snapshot instead of merely between polls.
        || (state.observer
            && state.ledger_path.as_deref().is_none_or(|ledger_path| {
                !super::dashboard::session_is_terminal(&state.session, ledger_path)
            }))
}

fn mark_timer_painted(state: &mut RunPanelState) {
    state.last_timer_paint = Instant::now();
    state.cadence.painted();
}

fn rebuild_view(state: &mut RunPanelState, narration: Option<RunNarration>) {
    state.view = run_view(
        &state.trait_ref,
        &state.plan,
        &state.session,
        narration,
        PresentationState {
            active_started: &state.active_started,
            finished_durations: &state.finished_durations,
            output_tokens: &state.output_tokens,
            loop_elapsed: &state.loop_elapsed,
            loop_output_tokens: &state.loop_output_tokens,
            step_summaries: &state.step_summaries,
            step_summary_at: &state.step_summary_at,
            narrator_tokens: state.narrator_tokens,
            guide_tokens: state
                .ask
                .as_ref()
                .map_or(state.ledger_guide_tokens, GuideChatHandle::guide_tokens),
            run_started: state.run_started,
        },
    );
    state.view.merge_rows = state.merge_rows.clone();
}

/// Sole key-application site for the live pane: drains whatever the pump
/// forwarded (ctrl-c already handled inside `poll_detach` as an instant
/// kill), routes every key through the `q` confirm-quit modal when one is
/// open, opens that modal on a bare `q` otherwise, and applies scroll/focus
/// for everything else. Reports whether anything actually changed so callers
/// (`tick`) know to bypass the repaint throttle. Called at the top of every
/// render path via [`render_locked`], plus directly by `tick` so a quit
/// keypress stays responsive even when a throttled tick skips the render
/// below.
#[derive(Debug, PartialEq, Eq)]
enum LiveViewKeyAction {
    OpenDashboard,
    ConfirmQuit,
}

/// Bare `d` hands presentation to the dashboard immediately. `q` intentionally
/// retains its existing confirmation-modal behavior.
fn live_view_key_action(key: &KeyEvent) -> Option<LiveViewKeyAction> {
    match key.code {
        KeyCode::Char('d') if key.modifiers.is_empty() => Some(LiveViewKeyAction::OpenDashboard),
        KeyCode::Char('q') => Some(LiveViewKeyAction::ConfirmQuit),
        _ => None,
    }
}

fn poll_and_apply_keys(state: &mut RunPanelState) -> bool {
    let mut changed = false;
    if let Some(ask) = state.ask.as_ref() {
        changed |= ask.poll_results();
    }
    let keys = state.repaint.poll_detach();
    for key in keys {
        if let Some(modal) = state.modal.as_mut() {
            match modal.handle_key(&key) {
                tui_kit::ModalOutcome::Confirmed => {
                    state.modal = None;
                    state.repaint.quit();
                    // P081: an observer's `q` returns to the dashboard
                    // automatically — this message is only accurate for the
                    // live view's own quit, which leaves no dashboard to
                    // return to.
                    if !state.observer {
                        eprintln!(
                            "live view closed; run continues — reattach with ctx traits dashboard"
                        );
                    }
                }
                tui_kit::ModalOutcome::Cancelled => {
                    state.modal = None;
                }
                tui_kit::ModalOutcome::Pending | tui_kit::ModalOutcome::Submitted(_) => {}
            }
            changed = true;
            continue;
        }
        if state.ask.is_some() && apply_ask_key(state, key) {
            changed = true;
            continue;
        }
        // P081: an observer never dispatches a fresh guide seat (that would
        // reintroduce a second writer against the driving process's own
        // ledger). Without an inherited handle, the ask-open key gets a
        // visible refusal — never silence, per the implementation draft's
        // "ask: one deliberate rule".
        if state.observer && state.ask.is_none() && key.code == KeyCode::Char('?') {
            let notice = StreamRow {
                at: state.run_started.elapsed(),
                kind: StreamRowKind::Narration,
                text: "ask refused: this run is driven by another process — ask there".to_string(),
            };
            // Retained on the state (not just pushed into `current_stream`)
            // since `apply_ledger_seed` rebuilds that field wholesale on
            // every poll — see `observer_notice`'s doc comment.
            state.observer_notice = Some(notice.clone());
            state.current_stream.push_back(notice);
            while state.current_stream.len() > CURRENT_STREAM_CAP {
                state.current_stream.pop_front();
            }
            changed = true;
            continue;
        }
        // Routed through `live_view_key_action` rather than matched inline so
        // the key contract has one definition the tests can hold: `d` is
        // presentation-only handoff, `q` keeps its confirmation. This runs
        // AFTER the ask pane consumes keys — while the guide is open, `d` and
        // `q` are text, not commands.
        match live_view_key_action(&key) {
            Some(LiveViewKeyAction::OpenDashboard) => {
                state.repaint.quit();
                state.cadence.inactive();
                state.handoff.request(
                    state.session.session_id.as_str().to_string(),
                    state.ask.clone(),
                );
                changed = true;
                continue;
            }
            Some(LiveViewKeyAction::ConfirmQuit) => {
                state.modal = Some(tui_kit::Modal::confirm(
                    "Quit live view?",
                    [
                        "The run keeps going in the background.",
                        "Reattach anytime with `ctx traits dashboard`.",
                    ]
                    .join("\n"),
                ));
                changed = true;
                continue;
            }
            None => {}
        }
        if tui_panes::tab_cycle_key(&key).is_some() || tui_kit::scroll_key(&key).is_some() {
            changed = true;
        }
        state.pending_keys.push(key);
    }
    changed
}

fn apply_ask_key(state: &mut RunPanelState, key: KeyEvent) -> bool {
    let Some(ask) = state.ask.as_ref() else {
        return false;
    };
    let (current_step, statuses) = guide_snapshot(&state.view);
    ask.set_context(crate::app::guide::evidence(
        &state.session,
        &state.plan,
        state.guide_ledger_path.as_deref(),
        &current_step,
        &statuses,
    ));
    let body_rows = ask.lock().ask.body_rows;
    ask.handle_key(&key, body_rows)
}

/// Handle presentation-only keys before dispatch. Keeping this reducer small
/// makes every visible phase transition share the live router's exact rules;
/// in particular, Waiting consumes pane-navigation keys until Escape collapses
/// the ask pane.
fn apply_ask_presentation_key(ask: &mut AskPane, key: &KeyEvent) -> Option<bool> {
    if !ask.open {
        if key.code == KeyCode::Char('?') {
            ask.open = true;
            return Some(true);
        }
        return Some(false);
    }
    if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
        ask.open = false;
        return Some(true);
    }
    None
}

fn apply_ask_result(ask: &mut AskPane, generation: u64, result: Result<String, String>) -> bool {
    if !ask.in_flight || generation != ask.generation {
        return false;
    }
    let Some(exchange) = ask
        .exchanges
        .iter_mut()
        .find(|exchange| exchange.generation == generation && exchange.answer.is_none())
    else {
        return false;
    };
    ask.in_flight = false;
    exchange.answer =
        Some(displayable_guide_answer(&result.unwrap_or_else(|error| {
            format!("Guide unavailable: {error}")
        })));
    true
}

fn displayable_guide_answer(answer: &str) -> String {
    let cleaned = tui::clean_live_text(answer);
    let normalized = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut bounded: String = normalized.chars().take(MAX_GUIDE_ANSWER_CHARS).collect();
    if bounded.chars().count() < normalized.chars().count() {
        bounded.push_str("...");
    }
    bounded
}

fn guide_snapshot(view: &RunView) -> (String, String) {
    let step = view
        .steps
        .iter()
        .find(|step| step.active)
        .map(|step| step.label.as_str())
        .unwrap_or("none")
        .to_string();
    let statuses = view
        .steps
        .iter()
        .map(|step| format!("{}: {:?}", step.label, step.state))
        .collect::<Vec<_>>()
        .join("; ");
    (step, statuses)
}

#[derive(Clone, Copy)]
enum FollowTarget {
    Tail,
    ActiveRow(Option<usize>),
}

impl FollowTarget {
    /// The viewport start which puts this target at its follow alignment,
    /// clamped using the same bounds as `ViewportScroll`.
    fn viewport_start(self, len: usize, rows: usize) -> usize {
        let desired_start = match self {
            Self::Tail => len.saturating_sub(rows),
            Self::ActiveRow(active_row) => active_row
                .unwrap_or_else(|| len.saturating_sub(1))
                .saturating_sub(rows.saturating_sub(1)),
        };
        desired_start.min(len.saturating_sub(rows.min(len)))
    }

    fn is_following(self, scroll: &tui_kit::ViewportScroll, len: usize, rows: usize) -> bool {
        match self {
            Self::Tail => scroll.is_at_bottom(rows),
            // `window(0)` is always empty, so its start cannot describe the
            // persisted offset. At zero height, bottom remains the only
            // observable follow alignment.
            Self::ActiveRow(_) if rows == 0 => scroll.is_at_bottom(rows),
            Self::ActiveRow(_) => scroll.window(rows).start == self.viewport_start(len, rows),
        }
    }
}

/// Applies one scroll delta and derives `follow` from the resulting viewport,
/// never from the key direction. A journey only resumes active-row following
/// at its exact active-row alignment; append-only panes resume at their tail.
fn apply_scroll_and_derive_follow(
    scroll: &mut tui_kit::ViewportScroll,
    follow: &mut bool,
    delta: tui_kit::ScrollDelta,
    budget: usize,
    len: usize,
    target: FollowTarget,
) {
    scroll.apply(delta, budget);
    *follow = target.is_following(scroll, len, budget);
}

/// Only row-at-a-time scroll keys are safe to fold. Page and jump keys share
/// `ScrollDelta` variants with row keys, so classify the original key first.
fn repeat_row_scroll_key(key: &KeyEvent) -> Option<tui_kit::ScrollDelta> {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => Some(tui_kit::ScrollDelta::Up(1)),
        KeyCode::Down | KeyCode::Char('j') => Some(tui_kit::ScrollDelta::Down(1)),
        _ => None,
    }
}

fn capped_repeat_delta(
    delta: tui_kit::ScrollDelta,
    repeats: Option<usize>,
    budget: usize,
) -> tui_kit::ScrollDelta {
    let Some(repeats) = repeats else {
        return delta;
    };
    match delta {
        tui_kit::ScrollDelta::Up(_) => tui_kit::ScrollDelta::Up(repeats.min(budget)),
        tui_kit::ScrollDelta::Down(_) => tui_kit::ScrollDelta::Down(repeats.min(budget)),
        tui_kit::ScrollDelta::Start | tui_kit::ScrollDelta::End => delta,
    }
}

fn push_stream_row(state: &mut RunPanelState, kind: StreamRowKind, text: String) {
    let at = state.run_started.elapsed();
    state.current_stream.push_back(StreamRow { at, kind, text });
    while state.current_stream.len() > CURRENT_STREAM_CAP {
        state.current_stream.pop_front();
    }
}

/// The boundary marker names the step from the rebuilt ledger view, while the
/// activity phase describes the freshly entered stream state.
fn entered_step_text(view: &RunView, phase: &str) -> Option<String> {
    let step = view.steps.iter().find(|step| step.active)?;
    let phase = phase.trim();
    Some(if phase.is_empty() {
        format!("[{}]", step.label)
    } else {
        format!("[{}] ({phase})", step.label)
    })
}

fn render_locked(state: &mut RunPanelState) {
    // Capture before draining: a wake that lands concurrently afterwards must
    // remain pending for a later tick rather than being acknowledged unseen.
    let input_generation = state.input_generation.load(Ordering::Acquire);
    state.repaint.apply_resize();
    poll_and_apply_keys(state);
    let progress_lines = progress_lines(&state.view);
    let (journey_lines, active_row) = journey_lines_with_active_row(&state.view);
    let post_run_lines = post_run_lines(&state.view);
    let history_rows = story_history_lines(&state.view);
    let mut current_rows = story_stream_lines(state);
    // P552: the trailing in-flight line is not a special overlay outside the
    // event model — it folds into the same `current_rows` set the recorded
    // stream uses, so `render_pane_body` formats every CURRENT row (recorded
    // or in-flight) through the one `event_row_line` contract.
    if let Some(overlay) = stream_overlay_line(state) {
        current_rows.push(overlay);
    }
    let title_line = title_row_line(
        state.title_state.as_ref(),
        &state.trait_name,
        state.session.provenance.started_at_epoch,
    );
    let RunPanelState {
        repaint,
        scrolls,
        progress_follow,
        journey_follow,
        history_follow,
        current_follow,
        focus,
        pending_keys,
        modal,
        ask,
        ..
    } = state;
    let modal = modal.as_ref();
    let _ = repaint.draw(|frame| {
        render_live_panes(
            frame,
            LiveFrame {
                title_line: &title_line,
                progress_lines: &progress_lines,
                journey_lines: &journey_lines,
                active_row,
                history_rows: &history_rows,
                current_rows: &current_rows,
                post_run_lines: post_run_lines.as_deref(),
                scrolls,
                progress_follow,
                journey_follow,
                history_follow,
                current_follow,
                focus,
                pending_keys,
                modal,
                ask: ask.as_ref(),
            },
        );
    });
    state.last_tree_lines = journey_row_lines(&journey_lines, 80);
    state
        .handled_generation
        .fetch_max(input_generation, Ordering::Release);
}

/// One visible title row for every live or attached lifecycle state.
pub(crate) fn title_row_line(
    title_state: Option<&ctx_traits_core::procedure::session::SessionTitleState>,
    trait_name: &str,
    started_at_epoch: Option<u64>,
) -> tui::Line {
    let mut line = tui::Line::blank();
    match title_state {
        Some(ctx_traits_core::procedure::session::SessionTitleState::Resolved {
            title, ..
        }) => {
            line.push(title.clone(), tui::Tone::Bold);
            line.push(" \u{b7} ", tui::Tone::Muted);
        }
        Some(ctx_traits_core::procedure::session::SessionTitleState::Terminal { .. }) => {}
        None
        | Some(ctx_traits_core::procedure::session::SessionTitleState::InFlight { .. })
        | Some(ctx_traits_core::procedure::session::SessionTitleState::Retryable { .. }) => {
            line.push("(Generating session title…)".to_string(), tui::Tone::Muted);
            line.push(" \u{b7} ", tui::Tone::Muted);
        }
    }
    line.push(trait_name.to_string(), tui::Tone::Default);
    if let Some(epoch) = started_at_epoch {
        line.push(" \u{b7} ", tui::Tone::Muted);
        line.push(
            format!("Started at {}", epoch_clock_utc(epoch)),
            tui::Tone::Muted,
        );
    }
    line
}

/// Pure `HH:MM:SS` decomposition of a UNIX epoch, UTC (no calendar handling
/// needed — only the seconds-of-day remainder). No `chrono`/`time` dependency
/// exists in this workspace for a single clock string.
fn epoch_clock_utc(epoch: u64) -> String {
    let seconds_of_day = epoch % 86_400;
    let hours = seconds_of_day / 3_600;
    let minutes = (seconds_of_day % 3_600) / 60;
    let seconds = seconds_of_day % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

/// P552: builds a [`PaneTree`] with a leaf for exactly the panes `data`
/// supplies content for — no live/preview/attached mode flag anywhere in
/// this function. Full data (all four `Some`) produces the bounded-progress
/// 2x2 grid; preview data (`history`/`current` both `None`) naturally
/// collapses to the progress/journey left stack alone; a narrow terminal
/// stacks every populated pane instead, without omitting any of them.
/// `ids.progress`/`ids.journey` must not both be `None` in `data` — every
/// caller has at least a progress or a journey source.
pub(crate) fn pane_tree(ids: &PaneIds, area: Rect, data: &PaneData<'_>) -> PaneTree {
    let progress = || PaneTree::Leaf {
        id: ids.progress,
        title: "information".to_string(),
    };
    let journey = || PaneTree::Leaf {
        id: ids.journey,
        title: "journey".to_string(),
    };
    let history = || PaneTree::Leaf {
        id: ids.history,
        title: "history".to_string(),
    };
    let current = || PaneTree::Leaf {
        id: ids.current,
        title: "current activity".to_string(),
    };
    let post_run = || PaneTree::Leaf {
        id: ids.current,
        title: "post-run".to_string(),
    };
    let has_progress = data.progress.is_some();
    let has_journey = data.journey.is_some();
    let has_history = data.history.is_some();
    let has_current = data.current.is_some();
    let has_post_run = data.post_run.is_some();
    debug_assert!(
        has_progress || has_journey,
        "pane_tree requires at least a progress or journey source"
    );

    if area.width < NARROW_WIDTH_THRESHOLD {
        let mut children = Vec::new();
        if has_progress {
            children.push((Constraint::Min(3), progress()));
        }
        if has_journey {
            children.push((Constraint::Min(3), journey()));
        }
        if has_post_run {
            children.push((Constraint::Min(CURRENT_MIN_OUTER_ROWS), post_run()));
        } else if has_history {
            children.push((Constraint::Min(3), history()));
        }
        if has_current && !has_post_run {
            children.push((Constraint::Min(CURRENT_MIN_OUTER_ROWS), current()));
        }
        return PaneTree::Split {
            dir: Direction::Vertical,
            children,
        };
    }

    let left = match (has_progress, has_journey) {
        (true, true) => {
            // P552: progress is bounded to its own handful of standing-fact
            // rows so journey — the pane with unbounded content — receives
            // the rest of the left column's height.
            let progress_rows = data.progress.map_or(0, <[_]>::len);
            let progress_outer = u16::try_from(progress_rows)
                .unwrap_or(u16::MAX)
                .saturating_add(2);
            PaneTree::Split {
                dir: Direction::Vertical,
                children: vec![
                    (Constraint::Length(progress_outer), progress()),
                    (Constraint::Min(3), journey()),
                ],
            }
        }
        (true, false) => progress(),
        (false, true) => journey(),
        (false, false) => PaneTree::Split {
            dir: Direction::Vertical,
            children: Vec::new(),
        },
    };

    let right = if has_post_run {
        Some(post_run())
    } else if has_history
        && has_current
        && area.height >= HISTORY_MIN_OUTER_ROWS + CURRENT_MIN_OUTER_ROWS
    {
        let history_rows = data.history.map_or(0, <[_]>::len);
        let history_height = u16::try_from(history_rows)
            .unwrap_or(u16::MAX)
            .saturating_add(2)
            .max(HISTORY_MIN_OUTER_ROWS)
            .min(area.height / 2)
            .min(area.height.saturating_sub(CURRENT_MIN_OUTER_ROWS));
        Some(PaneTree::Split {
            dir: Direction::Vertical,
            children: vec![
                (Constraint::Length(history_height), history()),
                (Constraint::Min(CURRENT_MIN_OUTER_ROWS), current()),
            ],
        })
    } else if has_current {
        Some(current())
    } else if has_history {
        Some(history())
    } else {
        None
    };

    match right {
        Some(right) => PaneTree::Split {
            dir: Direction::Horizontal,
            children: vec![
                (Constraint::Percentage(60), left),
                (Constraint::Percentage(40), right),
            ],
        },
        None => left,
    }
}

struct LiveFrame<'a> {
    title_line: &'a tui::Line,
    /// The PROGRESS pane's bounded standing facts — [`progress_lines`].
    progress_lines: &'a [tui::Line],
    /// The JOURNEY pane's full content — [`journey_lines_with_active_row`].
    journey_lines: &'a [JourneyRow],
    active_row: Option<usize>,
    /// Untruncated history/current-activity events — [`event_row_line`]
    /// truncates each to a single physical row only once this pane's inner
    /// width is known, inside [`render_pane_body`] itself.
    history_rows: &'a [EventRow],
    /// The CURRENT pane's full row set — the recorded stream plus, when
    /// live, the trailing in-flight overlay already folded in by the
    /// caller (see [`RunPanel`]'s render path) so this module's shared
    /// [`render_pane_body`] treats every CURRENT row through the one
    /// `EventRow`/[`event_row_line`] contract, with no separate overlay
    /// case.
    current_rows: &'a [EventRow],
    post_run_lines: Option<&'a [tui::Line]>,
    scrolls: &'a mut PaneScrolls,
    progress_follow: &'a mut bool,
    journey_follow: &'a mut bool,
    history_follow: &'a mut bool,
    current_follow: &'a mut bool,
    focus: &'a mut FocusRing,
    pending_keys: &'a mut Vec<KeyEvent>,
    modal: Option<&'a tui_kit::Modal>,
    ask: Option<&'a GuideChatHandle>,
}

fn ask_lines(ask: &AskPane) -> Vec<String> {
    ask.exchanges
        .iter()
        .flat_map(|exchange| {
            let answer = exchange.answer.as_deref().unwrap_or("thinking...");
            [
                format!("You: {}", exchange.question),
                format!("Guide: {answer}"),
            ]
        })
        .collect()
}

fn drawable_pane_ids(tree: &PaneTree, layout: &PaneLayoutResult) -> Vec<PaneId> {
    tree.leaf_ids()
        .into_iter()
        .filter(|id| {
            layout.rect(id).is_some_and(|rect| {
                let inner = tui_panes::pane_inner(rect);
                inner.width > 0 && inner.height > 0
            })
        })
        .collect()
}

/// The live run surface's own footer chrome, wrapping the shared
/// [`render_pane_body`] for its title row + four-pane body — the live run
/// has no standing-facts `progress` source separate from `tree_lines` today
/// (its header still folds progress-facts and journey rows into one
/// `tree_lines` vector; splitting that fully into its own `PaneData::progress`
/// source is tracked separately), so this wrapper hands the tree's own
/// content to the `journey` pane and leaves `progress` populated from the
/// same standing facts computed by [`progress_lines`]. Title reservation and
/// rendering live in [`render_pane_body`] itself (via `PaneData::title`), so
/// this wrapper only carves out the footer row.
fn render_live_panes(frame: &mut ratatui::Frame<'_>, state: LiveFrame<'_>) {
    let LiveFrame {
        title_line,
        progress_lines,
        journey_lines: journey_rows,
        active_row,
        history_rows,
        current_rows,
        post_run_lines,
        scrolls,
        progress_follow,
        journey_follow,
        history_follow,
        current_follow,
        focus,
        pending_keys,
        modal,
        ask,
    } = state;
    let full_area = frame.area();
    let regions = live_frame_regions(full_area);
    frame.render_widget(
        tui_kit::keymap_footer(
            if ask.is_some() {
                ASK_FOOTER_HINT
            } else {
                "[d] dashboard · [q] exit · [ctrl-c] kill · [up/down] scroll · [pgup/pgdn] page · [home/end] jump · [tab] cycle pane"
            },
            None,
        ),
        regions[1],
    );
    let data = PaneData {
        progress: Some(progress_lines),
        journey: Some(journey_rows),
        // Accepted statuses are durable ledger evidence. Do not reserve a pane
        // merely because the live projection currently has an empty row list.
        history: (!history_rows.is_empty()).then_some(history_rows),
        current: Some(current_rows),
        post_run: post_run_lines,
        title: PaneTitleRow::Visible(title_line),
    };
    render_pane_body(
        frame,
        regions[0],
        &LIVE_PANE_IDS,
        &data,
        active_row,
        Some(CURRENT_PANE),
        PaneRenderState {
            scrolls,
            follow: PaneFollow {
                progress: progress_follow,
                journey: journey_follow,
                history: history_follow,
                current: current_follow,
            },
            focus,
            pending_keys,
            key_target: None,
        },
    );
    if let Some(modal) = modal {
        tui_kit::render_modal(frame, full_area, modal);
    } else if let Some(ask) = ask.filter(|ask| ask.is_open()) {
        ask.render(frame, full_area);
    }
}

fn live_frame_regions(full_area: Rect) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(full_area)
}

/// P552: which of the (up to) four panes' user-driven scroll state should
/// stay pinned to its own tail/anchor whenever new content lands — a live
/// run and a dashboard attach each own their own four bools; a caller with
/// fewer sources than four (a dashboard preview) still owns all four, since
/// [`render_pane_body`] only ever reads the bool for a pane `data` actually
/// populates.
pub(crate) struct PaneFollow<'a> {
    pub(crate) progress: &'a mut bool,
    pub(crate) journey: &'a mut bool,
    pub(crate) history: &'a mut bool,
    pub(crate) current: &'a mut bool,
}

/// The mutable state [`render_pane_body`] reads and updates on every call —
/// grouped into one struct (rather than four separate parameters) so the
/// shared renderer's own signature stays under clippy's argument-count
/// lint without suppressing it.
pub(crate) struct PaneRenderState<'a> {
    pub(crate) scrolls: &'a mut PaneScrolls,
    pub(crate) follow: PaneFollow<'a>,
    pub(crate) focus: &'a mut FocusRing,
    pub(crate) pending_keys: &'a mut Vec<KeyEvent>,
    /// P552 review `live-run-pane-contract-absent`: the pane a drained
    /// scroll key addresses instead of `focus.current()`, when `Some` — see
    /// [`render_pane_body`]'s own doc for why the ordinary (list-visible)
    /// SESSIONS preview needs this. `None` for a live run or a dashboard
    /// attach, whose `focus` is reconciled to the very tree being drawn.
    pub(crate) key_target: Option<PaneId>,
}

/// P552's title row, owned by [`render_pane_body`] itself so a live run and
/// a dashboard attach receive identical title behavior — a dashboard
/// preview supplies [`PaneTitleRow::None`] (no row consumed at all, per the
/// implementation draft's out-of-scope: dashboard previews never carry a
/// title), while a live run and an attach both supply [`PaneTitleRow::Reserved`],
/// which consumes its one row whether or not a title has resolved yet
/// (`None` renders blank — there is no placeholder variant, per
/// [`title_row_line`]).
pub(crate) enum PaneTitleRow<'a> {
    None,
    Visible(&'a tui::Line),
}

fn title_row_area(area: Rect) -> Rect {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area)[0]
}

/// The pane body area after [`PaneTitleRow`]'s own row (if any) is carved
/// off `area` — the single source of truth both [`render_pane_body`]'s own
/// paint pass and a caller's cached pane-layout resolution (e.g. dashboard
/// attach's `state.last_pane_layout`) must agree on, so generic scroll/focus
/// handling never reads rects that are off by the title row.
pub(crate) fn pane_body_area(area: Rect, title: &PaneTitleRow<'_>) -> Rect {
    match title {
        PaneTitleRow::None => area,
        PaneTitleRow::Visible(_) => Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area)[1],
    }
}

/// P552's one shared pane renderer: resolves `data`'s populated panes into a
/// [`PaneTree`] via [`pane_tree`], reconciles focus, formats every
/// history/current-activity row through [`event_row_line`], and renders
/// border-only content directly into [`tui_panes::pane_inner`] — the sole
/// renderer for a live run's body, dashboard preview's progress/journey
/// pair, and dashboard attach's full four-pane body. `reconcile_default`
/// is `Some` only when `focus` is scoped to exactly this pane set (a live
/// run, or a dashboard attach with its session list hidden); a dashboard
/// preview, whose `focus` also spans the sessions list, passes `None` so
/// this call never steals focus away from a pane outside this tree.
///
/// P552 review `live-run-pane-contract-absent`: `state.key_target`, when
/// `Some`, is the pane a drained scroll key addresses instead of
/// `focus.current()` — the ordinary (list-visible) SESSIONS preview queues
/// PageUp/PageDown into `pending_keys` while `focus` itself stays on the
/// sessions list (so the list's own selection and visibility never move),
/// and needs those keys to reach the journey pane anyway. A live run or a
/// dashboard attach, whose `focus` is reconciled to this very tree, pass
/// `None` and keep routing by `focus.current()` as before.
pub(crate) fn render_pane_body(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    ids: &PaneIds,
    data: &PaneData<'_>,
    journey_active_row: Option<usize>,
    reconcile_default: Option<PaneId>,
    state: PaneRenderState<'_>,
) {
    if let PaneTitleRow::Visible(title_line) = &data.title {
        let title_area = title_row_area(area);
        frame.render_widget(
            ratatui::widgets::Paragraph::new(vec![tui_ratatui::render_line(title_line)]),
            title_area,
        );
    }
    let area = pane_body_area(area, &data.title);
    let PaneRenderState {
        scrolls,
        follow,
        focus,
        pending_keys,
        key_target,
    } = state;
    let PaneFollow {
        progress: progress_follow,
        journey: journey_follow,
        history: history_follow,
        current: current_follow,
    } = follow;
    let tree = pane_tree(ids, area, data);
    let layout = tree.resolve(area);
    if let Some(default) = reconcile_default {
        focus.reconcile(drawable_pane_ids(&tree, &layout), default);
    }
    let progress_outer = data.progress.and(layout.rect(ids.progress));
    let journey_outer = data.journey.and(layout.rect(ids.journey));
    let history_outer = data.history.and(layout.rect(ids.history));
    // Post-run deliberately reuses current's pane id and scroll state. Once it
    // exists, current must be absent from every renderer path, not only the tree.
    let current_outer = data
        .post_run
        .is_none()
        .then(|| data.current.and(layout.rect(ids.current)))
        .flatten();
    let post_run_outer = data.post_run.and(layout.rect(ids.current));
    let progress_inner = progress_outer.map(tui_panes::pane_inner);
    let journey_inner = journey_outer.map(tui_panes::pane_inner);
    let history_inner = history_outer.map(tui_panes::pane_inner);
    let current_inner = current_outer.map(tui_panes::pane_inner);
    let post_run_inner = post_run_outer.map(tui_panes::pane_inner);

    let progress = data.progress.map(|lines| {
        lines
            .iter()
            .map(tui_ratatui::render_line)
            .collect::<Vec<_>>()
    });
    let journey = data.journey.map(|rows| {
        journey_row_lines(rows, journey_inner.map_or(0, |rect| rect.width))
            .iter()
            .map(tui_ratatui::render_line)
            .collect::<Vec<_>>()
    });
    let history = data.history.map(|rows| {
        event_row_lines(rows, history_inner.map_or(0, |rect| rect.width))
            .iter()
            .map(tui_ratatui::render_line)
            .collect::<Vec<_>>()
    });
    let current = data
        .post_run
        .is_none()
        .then_some(data.current)
        .flatten()
        .map(|rows| {
            event_row_lines(rows, current_inner.map_or(0, |rect| rect.width))
                .iter()
                .map(tui_ratatui::render_line)
                .collect::<Vec<_>>()
        });
    let post_run = data.post_run.map(|lines| {
        lines
            .iter()
            .map(tui_ratatui::render_line)
            .collect::<Vec<_>>()
    });

    let mut pending_keys = pending_keys.drain(..).peekable();
    while let Some(key) = pending_keys.next() {
        if let Some(step) = tui_panes::tab_cycle_key(&key) {
            match step {
                TabStep::Next => focus.next(),
                TabStep::Prev => focus.prev(),
            }
            continue;
        }
        let repeat_delta = repeat_row_scroll_key(&key);
        let repeats = repeat_delta.map(|delta| {
            let mut repeats = 1;
            while pending_keys
                .peek()
                .is_some_and(|next| repeat_row_scroll_key(next) == Some(delta))
            {
                pending_keys.next();
                repeats += 1;
            }
            repeats
        });
        let Some(delta) = repeat_delta.or_else(|| tui_kit::scroll_key(&key)) else {
            continue;
        };
        let Some(id) = key_target.or_else(|| focus.current()) else {
            continue;
        };
        if id == ids.progress
            && let (Some(progress), Some(inner)) = (&progress, progress_inner)
        {
            let scroll = scrolls.get_mut(id);
            scroll.set_len(progress.len());
            apply_scroll_and_derive_follow(
                scroll,
                progress_follow,
                capped_repeat_delta(delta, repeats, inner.height as usize),
                inner.height as usize,
                progress.len(),
                FollowTarget::Tail,
            );
        } else if id == ids.journey
            && let (Some(journey), Some(inner)) = (&journey, journey_inner)
        {
            let scroll = scrolls.get_mut(id);
            scroll.set_len(journey.len());
            apply_scroll_and_derive_follow(
                scroll,
                journey_follow,
                capped_repeat_delta(delta, repeats, inner.height as usize),
                inner.height as usize,
                journey.len(),
                FollowTarget::ActiveRow(journey_active_row),
            );
        } else if id == ids.history
            && let (Some(history), Some(inner)) = (&history, history_inner)
        {
            let scroll = scrolls.get_mut(id);
            scroll.set_len(history.len());
            apply_scroll_and_derive_follow(
                scroll,
                history_follow,
                capped_repeat_delta(delta, repeats, inner.height as usize),
                inner.height as usize,
                history.len(),
                FollowTarget::Tail,
            );
        } else if id == ids.current
            && let (Some(post_run), Some(inner)) = (&post_run, post_run_inner)
        {
            let scroll = scrolls.get_mut(id);
            scroll.set_len(post_run.len());
            apply_scroll_and_derive_follow(
                scroll,
                current_follow,
                capped_repeat_delta(delta, repeats, inner.height as usize),
                inner.height as usize,
                post_run.len(),
                FollowTarget::Tail,
            );
        } else if id == ids.current
            && let (Some(current), Some(inner)) = (&current, current_inner)
        {
            let scroll = scrolls.get_mut(id);
            scroll.set_len(current.len());
            apply_scroll_and_derive_follow(
                scroll,
                current_follow,
                capped_repeat_delta(delta, repeats, inner.height as usize),
                inner.height as usize,
                current.len(),
                FollowTarget::Tail,
            );
        }
    }

    if let Some(outer) = progress_outer {
        tui_panes::render_pane(
            frame,
            outer,
            tree.title(ids.progress).expect("progress title"),
            focus.is_focused(ids.progress),
        );
    }
    if let Some(outer) = journey_outer {
        tui_panes::render_pane(
            frame,
            outer,
            tree.title(ids.journey).expect("journey title"),
            focus.is_focused(ids.journey),
        );
    }
    if let Some(outer) = history_outer {
        tui_panes::render_pane(
            frame,
            outer,
            tree.title(ids.history).expect("history title"),
            focus.is_focused(ids.history),
        );
    }
    if let Some(outer) = post_run_outer.or(current_outer) {
        tui_panes::render_pane(
            frame,
            outer,
            tree.title(ids.current).expect("current/post-run title"),
            focus.is_focused(ids.current),
        );
    }

    if let (Some(progress), Some(inner)) = (&progress, progress_inner) {
        follow_target(
            scrolls.get_mut(ids.progress),
            *progress_follow,
            FollowTarget::Tail,
            progress.len(),
            inner.height as usize,
        );
        tui_panes::render_wrapped_lines_pane(frame, inner, progress, scrolls.get_mut(ids.progress));
    }
    if let (Some(journey), Some(inner)) = (&journey, journey_inner) {
        follow_target(
            scrolls.get_mut(ids.journey),
            *journey_follow,
            FollowTarget::ActiveRow(journey_active_row),
            journey.len(),
            inner.height as usize,
        );
        tui_panes::render_wrapped_lines_pane(frame, inner, journey, scrolls.get_mut(ids.journey));
    }
    if let (Some(history), Some(inner)) = (&history, history_inner) {
        follow_target(
            scrolls.get_mut(ids.history),
            *history_follow,
            FollowTarget::Tail,
            history.len(),
            inner.height as usize,
        );
        tui_panes::render_wrapped_lines_pane(frame, inner, history, scrolls.get_mut(ids.history));
    }
    if let (Some(current), Some(inner)) = (&current, current_inner) {
        follow_target(
            scrolls.get_mut(ids.current),
            *current_follow,
            FollowTarget::Tail,
            current.len(),
            inner.height as usize,
        );
        tui_panes::render_wrapped_lines_pane(frame, inner, current, scrolls.get_mut(ids.current));
    }
    if let (Some(post_run), Some(inner)) = (&post_run, post_run_inner) {
        follow_target(
            scrolls.get_mut(ids.current),
            *current_follow,
            FollowTarget::Tail,
            post_run.len(),
            inner.height as usize,
        );
        tui_panes::render_wrapped_lines_pane(frame, inner, post_run, scrolls.get_mut(ids.current));
    }
}

fn follow_target(
    scroll: &mut tui_kit::ViewportScroll,
    follow: bool,
    target: FollowTarget,
    len: usize,
    rows: usize,
) {
    scroll.set_len(len);
    scroll.clamp(rows);
    if follow {
        align_scroll_start(scroll, target.viewport_start(len, rows), rows);
    }
}

/// Moves the persisted viewport only as far as its current rendered window is
/// from `desired_start`; `ViewportScroll` intentionally keeps its raw offset
/// private, so alignment remains coupled to the window the pane actually draws.
fn align_scroll_start(scroll: &mut tui_kit::ViewportScroll, desired_start: usize, rows: usize) {
    let current_start = scroll.window(rows).start;
    let delta = if desired_start >= current_start {
        tui_kit::ScrollDelta::Down(desired_start - current_start)
    } else {
        tui_kit::ScrollDelta::Up(current_start - desired_start)
    };
    scroll.apply(delta, rows);
}

/// One logical history/current-activity event, built independent of any
/// terminal width — [`event_row_line`] is the only place that turns one of
/// these into a width-truncated physical row, so this type carries the raw
/// (untruncated, uncleaned) tail text plus the tone to render it in.
/// `pub(crate)` (with a constructor rather than public fields) so a
/// dashboard-attach projection outside this module can build rows from a
/// persisted activity sidecar through the exact same type this module's own
/// live rows use — never a second event shape.
#[derive(Clone)]
pub(crate) struct EventRow {
    at: Option<Duration>,
    tail: String,
    tone: tui::Tone,
}

impl EventRow {
    pub(crate) fn new(at: Option<Duration>, tail: String, tone: tui::Tone) -> Self {
        Self { at, tail, tone }
    }

    #[cfg(test)]
    pub(crate) fn tail(&self) -> &str {
        &self.tail
    }
}

/// The story column's compressed history: one event per accepted ledger
/// execution in ledger order — `<label>: <summary>` when a P455 summary landed, otherwise
/// the truthful facts fallback `<label> · elapsed · tokens` — never a
/// placeholder. `at` is when the row itself was stamped (the summary's own
/// landing time, or the step's own elapsed for the fallback). Render through
/// [`event_row_line`] for the fixed `HH:MM:SS ` prefix.
fn story_history_lines(view: &RunView) -> Vec<EventRow> {
    view.history.iter().map(story_row_line).collect()
}

fn story_row_line(step: &HistoryStep) -> EventRow {
    let at = step.summary_at.or(step.elapsed);
    let (tail, tone) = match (&step.summary, step.kind.as_ref(), step.outcome) {
        (
            _,
            Some(ctx_traits_core::procedure::run::PlannedSequenceKind::Check),
            Some(HistoryOutcome::Check { ok, exit_code }),
        ) => {
            let mut tail = format!("{} · {}", step.label, if ok { "passed" } else { "failed" });
            if !ok && let Some(code) = exit_code {
                tail.push_str(&format!(" (exit {code})"));
            }
            (
                tail,
                if ok {
                    tui::Tone::Muted
                } else {
                    tui::Tone::Fail
                },
            )
        }
        (
            _,
            Some(ctx_traits_core::procedure::run::PlannedSequenceKind::Command),
            Some(HistoryOutcome::Command {
                succeeded,
                exit_code,
            }),
        ) => {
            let mut tail = format!(
                "{} · {}",
                step.label,
                if succeeded { "succeeded" } else { "failed" }
            );
            if !succeeded && let Some(code) = exit_code {
                tail.push_str(&format!(" (exit {code})"));
            }
            (
                tail,
                if succeeded {
                    tui::Tone::Muted
                } else {
                    tui::Tone::Fail
                },
            )
        }
        (_, Some(ctx_traits_core::procedure::run::PlannedSequenceKind::Command), None) => {
            (step.label.clone(), tui::Tone::Muted)
        }
        (Some(summary), _, _) => (format!("{}: {}", step.label, summary), tui::Tone::Default),
        (None, _, _) => {
            let mut tail = step.label.clone();
            if let Some(elapsed) = step.elapsed {
                tail.push_str(" \u{b7} ");
                tail.push_str(&tui::elapsed_text(elapsed));
            }
            if let Some(tokens) = step.output_tokens {
                tail.push_str(" \u{b7} ");
                tail.push_str(&tui::token_text(tokens));
            }
            (tail, tui::Tone::Muted)
        }
    };
    EventRow { at, tail, tone }
}

/// P552 shared one-row event formatter for history and current-activity
/// panes: a fixed `HH:MM:SS ` prefix (never truncated — a terminal
/// narrower than the prefix itself is left to clip it, per the P552
/// contract) followed by a cleaned tail, truncated by display width only in
/// whatever budget remains after the prefix. Each call produces exactly one
/// physical row; callers must not pass the result through
/// `tui_panes::wrapped_lines`.
const EVENT_PREFIX_SEP: &str = " ";

fn event_row_line(row: &EventRow, width: u16) -> tui::Line {
    let prefix = row
        .at
        .map(|at| format!("{}{EVENT_PREFIX_SEP}", tui::elapsed_text(at)))
        .unwrap_or_default();
    let prefix_width = tui::display_width(&prefix);
    let mut line = tui::Line::blank();
    line.push(prefix, tui::Tone::Muted);
    let tail = tui::clean_live_text(&row.tail);
    let budget = (width as usize).saturating_sub(prefix_width);
    line.push(tui::truncate_display_width_end(&tail, budget), row.tone);
    line
}

/// Renders a full set of [`EventRow`]s to physical rows at `width` — one row
/// per event, per [`event_row_line`].
fn event_row_lines(rows: &[EventRow], width: u16) -> Vec<tui::Line> {
    rows.iter().map(|row| event_row_line(row, width)).collect()
}

/// The CURRENT step's verbatim recorded stream — narrations in narrated
/// mode, drained model-text deltas in passthrough mode — each rendered
/// through [`event_row_line`], the same P552 one-row formatter
/// [`story_history_lines`] uses. The trailing in-flight tail line (still
/// updating, not yet a discrete timestamped event) is handled separately by
/// [`stream_overlay_line`], since it needs its own [`narration_line`]
/// rendering and dedup check.
fn story_stream_lines(state: &RunPanelState) -> Vec<EventRow> {
    state.current_stream.iter().map(stream_row_line).collect()
}

/// The CURRENT pane's trailing in-flight event, appended after the recorded
/// stream rows — `None` when there is no live narration or when its text
/// duplicates the last recorded row (so a just-landed narration is never
/// shown twice). Formatted through [`event_row_line`] like every other
/// history/current event, per the P552 one-formatter contract — the
/// in-flight line is not a special overlay outside that model.
fn stream_overlay_line(state: &RunPanelState) -> Option<EventRow> {
    let narration = display_narration(&state.view)?;
    let last_text = state.current_stream.back().map(|row| row.text.as_str());
    let at = Instant::now().duration_since(state.run_started);
    overlay_event_row(narration, last_text, at)
}

/// Pure fold of a live [`RunNarration`] into the CURRENT pane's trailing
/// [`EventRow`], split out of [`stream_overlay_line`] so it is testable
/// without a full `RunPanelState`. `None` when the narration text duplicates
/// the last recorded stream row.
fn overlay_event_row(
    narration: &RunNarration,
    last_recorded_text: Option<&str>,
    at: Duration,
) -> Option<EventRow> {
    if last_recorded_text == Some(narration.text.as_str()) {
        return None;
    }
    let tail = format!("{}: {}", narration.label, narration.text);
    let tone = if narration.finished {
        tui::Tone::Pass
    } else if narration.muted {
        tui::Tone::Muted
    } else {
        tui::Tone::Default
    };
    Some(EventRow {
        at: Some(at),
        tail,
        tone,
    })
}

fn stream_row_line(row: &StreamRow) -> EventRow {
    let tone = match row.kind {
        StreamRowKind::Narration => tui::Tone::Default,
        StreamRowKind::ModelText => tui::Tone::Muted,
    };
    EventRow {
        at: Some(row.at),
        tail: row.text.clone(),
        tone,
    }
}

fn update_live_display(state: &mut RunPanelState, display: tui::LiveDisplay) {
    state.view.narration = narration_for(&state.session, display);
}

fn narration_for(
    session: &ctx_traits_core::procedure::session::Session,
    display: tui::LiveDisplay,
) -> Option<RunNarration> {
    Some(RunNarration {
        label: active_label(session),
        text: display.text,
        muted: display.muted,
        finished: display.finished,
    })
}

fn run_view(
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
fn history_step_from_status(
    status: &ctx_traits_core::procedure::runtime::SequenceStatus,
    plan: Option<&ctx_traits_core::procedure::run::Plan>,
    session: Option<&ctx_traits_core::procedure::session::Session>,
    presentation: &PresentationState<'_>,
) -> HistoryStep {
    let key = history_presentation_key(status, presentation);
    let loop_key = history_loop_container_key(status);
    let planned = plan.and_then(|plan| planned_item_for_status(plan, status));
    HistoryStep {
        label: super::story::format_step_title(&status.title, &status.position_path),
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

fn planned_item_for_status<'a>(
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
fn canonical_status_path(
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

fn history_outcome(
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

fn command_succeeded(
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

/// P552: the information pane's own bounded standing facts — session, run,
/// input, harness,
/// and one combined progress/status/current-step/elapsed/work-token/
/// narrator-token line (or, once completed, the equivalent completed-summary
/// line) — never the step journey itself, which lives in
/// [`journey_lines_with_active_row`]. Kept intentionally small so the
/// [`pane_tree`] geometry can bound this pane's height and hand the rest of
/// the left column to journey.
fn progress_lines(view: &RunView) -> Vec<tui::Line> {
    let mut lines = Vec::new();
    render_header(&mut lines, &view.header);
    lines
}

/// P470: the journey (left) column's full, always-scrolled content — no line
/// cap, no compact-viewport degrade. Every step's mark/ports/facts renders in
/// full; the live narration itself moved to the story column's CURRENT
/// stream (see [`story_stream_lines`]) and is no longer inlined per-step.
/// Convenience wrapper over [`journey_lines_with_active_row`] for callers
/// that don't need the active row (e.g. [`render_ledger_run_view`], whose
/// ledger-only view never has an active step to anchor on).
fn journey_lines(view: &RunView) -> Vec<JourneyRow> {
    journey_lines_with_active_row(view).0
}

/// Same output as [`journey_lines`], plus the rendered row index of the
/// ACTIVE step's [`render_step_summary`] line — the one true source of that
/// mapping, since this is the only place that knows how many rows each step
/// group emits. Follow-mode anchoring must derive from this row index, never
/// from `view.steps`' own item index (a different coordinate space: each
/// step group is 3 rows, `render_step_summary` + two `render_port_line`
/// calls, under a multi-row header the caller must not hand-count either).
fn journey_lines_with_active_row(view: &RunView) -> (Vec<JourneyRow>, Option<usize>) {
    let mut lines = Vec::new();
    let target_step = active_step_index(view);
    let mut active_row = None;
    for (index, step) in view.steps.iter().enumerate() {
        if Some(index) == target_step {
            active_row = Some(lines.len());
        }
        lines.push(JourneyRow(JourneyRowKind::Step(Box::new(step.clone()))));
    }
    if let Some(narration) = completed_narration(view) {
        lines.push(JourneyRow(JourneyRowKind::Line(narration_line(narration))));
    }
    if view.header.completed {
        lines.push(JourneyRow(JourneyRowKind::Line(tui::Line::blank())));
        let mut line = tui::Line::blank();
        line.push("digest-stamped ", tui::Tone::Muted);
        line.push(view.header.state_digest.clone(), tui::Tone::Default);
        lines.push(JourneyRow(JourneyRowKind::Line(line)));
        lines.push(JourneyRow(JourneyRowKind::Line(tui::Line::blank())));
        let mut outputs = Vec::new();
        render_outputs_box(&mut outputs, &view.outputs);
        lines.extend(
            outputs
                .into_iter()
                .map(|line| JourneyRow(JourneyRowKind::Line(line))),
        );
    }
    (lines, active_row)
}

/// The completed-run morph requires both a completed journey and actual,
/// observed merge activity. `merge_rows` is retained for the panel lifetime,
/// so terminal transitions cannot rotate the pane tree back to history/current.
fn post_run_lines(view: &RunView) -> Option<Vec<tui::Line>> {
    (view.header.completed && !view.merge_rows.is_empty()).then(|| {
        let mut lines = Vec::new();
        for row in &view.merge_rows {
            render_merge_row(&mut lines, row);
        }
        lines
    })
}

/// Pure fold of one [`ActivityEvent`] into `rows`, split out of
/// [`RunPanel::merge_event`] so the P549 stage/status→row-state mapping is
/// directly unit-testable without a live terminal pane.
fn fold_merge_event(rows: &mut Vec<MergeRowView>, event: &ActivityEvent, now: Instant) {
    let terminal = match event.kind {
        ActivityKind::ValidatingOutput => Some(MergeRowState::Done),
        ActivityKind::Stalled if event.frame_id != "merge:lock" => Some(MergeRowState::Failed),
        _ => None,
    };
    let label = event
        .frame_id
        .strip_prefix("merge:")
        .unwrap_or(event.frame_id.as_str())
        .to_string();
    let detail = event.text.clone().or_else(|| event.tool.clone());
    let is_new_row = rows.iter().all(|row| row.frame_id != event.frame_id);
    if is_new_row {
        // P549: most stages record a ledger frame only on failure — a
        // stage that succeeds silently would otherwise leave its row
        // "Running" forever once a later stage's first event arrives. The
        // sole progression signal a live merge actually has is "a new
        // stage's row just appeared", so that is what closes every prior
        // still-open row to Done — a park/failure closes its own row via
        // `terminal` above and is left untouched here.
        for row in rows
            .iter_mut()
            .filter(|row| row.state == MergeRowState::Running)
        {
            row.state = MergeRowState::Done;
            row.finished.get_or_insert(now);
        }
    }
    match rows.iter_mut().find(|row| row.frame_id == event.frame_id) {
        Some(row) => {
            if detail.is_some() {
                row.detail = detail;
            }
            if let Some(terminal) = terminal {
                row.state = terminal;
                row.finished.get_or_insert(now);
            }
        }
        None => {
            rows.push(MergeRowView {
                frame_id: event.frame_id.clone(),
                label,
                state: terminal.unwrap_or(MergeRowState::Running),
                detail,
                started: now,
                finished: terminal.map(|_| now),
            });
        }
    }
}

fn render_merge_row(lines: &mut Vec<tui::Line>, row: &MergeRowView) {
    let (mark, tone) = match row.state {
        MergeRowState::Done => ("✓", tui::Tone::Pass),
        MergeRowState::Running => ("~", tui::Tone::Warn),
        MergeRowState::Failed => ("×", tui::Tone::Fail),
    };
    let mut line = tui::Line::blank();
    line.push(mark, tone);
    line.push(" ", tui::Tone::Muted);
    line.push(row.label.clone(), tone);
    if let Some(detail) = &row.detail {
        line.push("   ", tui::Tone::Muted);
        line.push(detail.clone(), tui::Tone::Default);
    }
    let elapsed = row.finished.unwrap_or_else(Instant::now) - row.started;
    line.push(" (", tui::Tone::Muted);
    line.push(tui::elapsed_text(elapsed), tui::Tone::Muted);
    line.push(")", tui::Tone::Muted);
    lines.push(line);
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
/// `ledger_path`'s P521 activity sidecar (via [`super::story::load_activity`])
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
struct LedgerPresentationSeed {
    run_started: Instant,
    output_tokens: BTreeMap<String, u64>,
    narrator_tokens: u64,
    guide_tokens: u64,
    step_summaries: BTreeMap<String, String>,
    step_summary_at: BTreeMap<String, Duration>,
    activity: Option<ctx_traits_core::procedure::story::ActivityInput>,
    started_at_epoch: Option<u64>,
}

fn ledger_presentation_seed(
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
    let activity = super::story::load_activity(ledger_path);
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
fn apply_ledger_seed(state: &mut RunPanelState, ledger_path: &camino::Utf8Path) {
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
            line.push(
                super::merge_story::stage_text(frame.stage).to_string(),
                tone,
            );
            line.push("   ", tui::Tone::Muted);
            line.push(
                super::merge_story::explain_frame(frame).sentence,
                tui::Tone::Default,
            );
            line
        })
        .collect()
}

/// The sidecar-only slice of [`LedgerPaneProjection`]: `current`/`history`
/// and the `activity_available`/`activity_degraded` authority, sourced
/// purely from [`super::story::load_activity`] — no [`ctx_traits_core::Trait`]
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
    let activity = super::story::load_activity(ledger_path);
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
                    super::story::format_step_title(&status.title, &status.position_path),
                    summary.text
                ),
                None => super::story::format_step_title(&status.title, &status.position_path),
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
fn sidecar_step_summary_maps(
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
fn latest_frame_event_rows(
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

fn completed_narration(view: &RunView) -> Option<&RunNarration> {
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
fn display_narration(view: &RunView) -> Option<&RunNarration> {
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

fn active_step_index(view: &RunView) -> Option<usize> {
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

/// P552: the information pane's standing facts, including compact session and
/// run identity. The completion digest stamp remains in
/// [`journey_lines_with_active_row`] as a terminal-outputs fact.
fn render_header(lines: &mut Vec<tui::Line>, header: &RunHeader) {
    identifier_line(lines, "session", &header.session_id, "session-");
    identifier_line(lines, "run", &header.run_id, "run-");

    let mut line = tui::Line::blank();
    line.push("input ", tui::Tone::Muted);
    line.push(header.input.clone(), tui::Tone::Default);
    lines.push(line);

    let mut line = tui::Line::blank();
    line.push("harness ", tui::Tone::Muted);
    line.push(header.harnesses.clone(), tui::Tone::Default);
    lines.push(line);

    if header.completed {
        let mut line = tui::Line::blank();
        line.push("✓ complete", tui::Tone::Pass);
        line.push(
            format!(
                " · {} steps · {} harnesses",
                header.total, header.harness_count
            ),
            tui::Tone::Default,
        );
        if let Some(elapsed) = header.elapsed {
            line.push(
                format!(" · {}", tui::elapsed_text(elapsed)),
                tui::Tone::Muted,
            );
        }
        if let Some(tokens) = header.output_tokens {
            line.push(format!(" · {}", tui::token_text(tokens)), tui::Tone::Muted);
        }
        if let Some(tokens) = header.narrator_tokens {
            line.push(
                format!(" · narrator {}", tui::token_text(tokens)),
                tui::Tone::Muted,
            );
        }
        if let Some(tokens) = header.guide_tokens {
            line.push(
                format!(" · guide {}", tui::token_text(tokens)),
                tui::Tone::Muted,
            );
        }
        if let Some(stopped) = &header.stopped {
            line.push(format!(" · {stopped}"), tui::Tone::Muted);
        }
        if header.structured_count > 0 {
            line.push(
                format!(
                    " · completed{} {}: {} open",
                    header
                        .structured_verdict
                        .as_deref()
                        .map_or(String::new(), |verdict| format!(" - {verdict},")),
                    header
                        .structured_label
                        .as_deref()
                        .unwrap_or("structured output"),
                    header.structured_count
                ),
                tui::Tone::Default,
            );
        }
        lines.push(line);
    } else {
        let mut line = tui::Line::blank();
        line.push("progress ", tui::Tone::Muted);
        line.push(
            format!("{}/{}", header.done, header.total),
            tui::Tone::Default,
        );
        line.push(" · ", tui::Tone::Muted);
        line.push(header.phase.clone(), tui::Tone::Default);
        if let Some(elapsed) = header.elapsed {
            line.push(
                format!(" · {}", tui::elapsed_text(elapsed)),
                tui::Tone::Muted,
            );
        }
        if let Some(tokens) = header.output_tokens {
            line.push(format!(" · {}", tui::token_text(tokens)), tui::Tone::Muted);
        }
        if let Some(tokens) = header.narrator_tokens {
            line.push(
                format!(" · narrator {}", tui::token_text(tokens)),
                tui::Tone::Muted,
            );
        }
        if let Some(tokens) = header.guide_tokens {
            line.push(
                format!(" · guide {}", tui::token_text(tokens)),
                tui::Tone::Muted,
            );
        }
        lines.push(line);
        if let Some(detail) = header.stop_detail.as_deref() {
            let mut line = tui::Line::blank();
            line.push("■ stopped ", tui::Tone::Fail);
            line.push("— ", tui::Tone::Muted);
            line.push(detail.to_string(), tui::Tone::Default);
            lines.push(line);
        }
    }
}

fn identifier_line(lines: &mut Vec<tui::Line>, label: &str, id: &str, prefix: &str) {
    let mut line = tui::Line::blank();
    line.push(format!("{label} "), tui::Tone::Muted);
    line.push(compact_identifier(id, prefix), tui::Tone::Default);
    lines.push(line);
}

fn compact_identifier(id: &str, prefix: &str) -> String {
    id.strip_prefix(prefix)
        .unwrap_or(id)
        .chars()
        .take(12)
        .collect()
}

fn narration_line(narration: &RunNarration) -> tui::Line {
    let mut line = tui::Line::blank();
    line.push("    ", tui::Tone::Muted);
    line.push(
        format!("{}:", narration.label),
        if narration.finished {
            tui::Tone::Pass
        } else {
            tui::Tone::Warn
        },
    );
    line.push(" ", tui::Tone::Muted);
    line.push(
        narration.text.clone(),
        if narration.finished {
            tui::Tone::Pass
        } else if narration.muted {
            tui::Tone::Muted
        } else {
            tui::Tone::Default
        },
    );
    line
}

fn journey_row_lines(rows: &[JourneyRow], width: u16) -> Vec<tui::Line> {
    rows.iter()
        .map(|row| match &row.0 {
            JourneyRowKind::Step(step) => journey_step_line(step, width),
            JourneyRowKind::Line(line) => line.clone(),
        })
        .collect()
}

fn journey_step_line(step: &RunStep, width: u16) -> tui::Line {
    let (mark, tone) = match step.state {
        StepState::Done => ("✓", tui::Tone::Pass),
        StepState::Running => ("~", tui::Tone::Warn),
        StepState::Pending => ("○", tui::Tone::Muted),
        StepState::Failed => ("×", tui::Tone::Fail),
    };
    let agent = step.harness.as_deref().map_or_else(
        || step.role.clone(),
        |harness| format!("{}@{harness}", step.role),
    );
    let variant = step.tags.join(" ∙ ");
    let state = step.status.clone();
    let mut suffixes = Vec::new();
    if step.structured_count > 0 {
        suffixes.push(format!("({} open)", step.structured_count));
    }
    let metrics = step.elapsed.map(|elapsed| {
        let mut text = tui::elapsed_text(elapsed);
        if let Some(tokens) = step.output_tokens {
            text.push_str(" · ");
            text.push_str(&tui::token_text(tokens));
        }
        text
    });
    let elapsed_only = step.elapsed.map(tui::elapsed_text);
    let full = [
        Some(format!("[agent] {agent}")),
        (!variant.is_empty()).then(|| format!("[variant] {variant}")),
        Some(state.clone()),
        metrics.clone(),
    ]
    .into_iter()
    .flatten()
    .chain(suffixes.iter().cloned())
    .collect::<Vec<_>>();
    let compact = [
        Some(agent.clone()),
        (!variant.is_empty()).then_some(variant.clone()),
        Some(state.clone()),
        elapsed_only,
    ]
    .into_iter()
    .flatten()
    .chain(suffixes.iter().cloned())
    .collect::<Vec<_>>();
    let without_variant_or_metrics = std::iter::once(agent.clone())
        .chain(std::iter::once(state.clone()))
        .chain(suffixes.iter().cloned())
        .collect::<Vec<_>>();
    let without_agent = std::iter::once(state.clone())
        .chain(suffixes.iter().cloned())
        .collect::<Vec<_>>();
    let candidates = [
        full,
        compact,
        without_variant_or_metrics,
        without_agent.clone(),
    ];
    let (label, fields) = candidates
        .into_iter()
        .find(|fields| journey_text_width(mark, &step.label, fields) <= width as usize)
        .map(|fields| (step.label.clone(), fields))
        .unwrap_or_else(|| {
            let tail = without_agent;
            let tail_width = tail
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(" · ");
            let budget = (width as usize)
                .saturating_sub(tui::display_width(mark) + 1 + 3 + tui::display_width(&tail_width));
            (tui::truncate_display_width_end(&step.label, budget), tail)
        });
    let mut line = tui::Line::blank();
    line.push(mark, tone);
    line.push(" ", tui::Tone::Muted);
    line.push(label, tone);
    for field in fields {
        line.push(" · ", tui::Tone::Muted);
        let field_tone = if field == step.status {
            status_tone(step)
        } else if field.starts_with('(') && field.ends_with(" open)") {
            tui::Tone::Default
        } else {
            tui::Tone::Muted
        };
        line.push(field, field_tone);
    }
    line
}

fn journey_text_width(mark: &str, label: &str, fields: &[String]) -> usize {
    tui::display_width(mark)
        + 1
        + tui::display_width(label)
        + fields
            .iter()
            .map(|field| 3 + tui::display_width(field))
            .sum::<usize>()
}

fn render_outputs_box(lines: &mut Vec<tui::Line>, outputs: &[RunOutput]) {
    let row_width = outputs
        .iter()
        .map(|output| output.slug.len() + 1 + output.status.len())
        .max()
        .unwrap_or(4)
        .max("outputs".len());
    let inner_width = row_width + 2;
    let label = " outputs ";
    let rule = "─".repeat(inner_width.saturating_sub(label.len()));
    let mut top = tui::Line::blank();
    top.push(format!("┌{label}{rule}┐"), tui::Tone::Muted);
    lines.push(top);

    if outputs.is_empty() {
        let mut line = tui::Line::blank();
        line.push("│ ", tui::Tone::Muted);
        line.push(format!("{:row_width$}", "none"), tui::Tone::Muted);
        line.push(" │", tui::Tone::Muted);
        lines.push(line);
    } else {
        for output in outputs {
            let text = format!("{} {}", output.slug, output.status);
            let mut line = tui::Line::blank();
            line.push("│ ", tui::Tone::Muted);
            line.push(
                format!("{text:row_width$}"),
                if output.accepted {
                    tui::Tone::Pass
                } else {
                    tui::Tone::Muted
                },
            );
            line.push(" │", tui::Tone::Muted);
            lines.push(line);
        }
    }

    let mut bottom = tui::Line::blank();
    bottom.push(format!("└{}┘", "─".repeat(inner_width)), tui::Tone::Muted);
    lines.push(bottom);
}

#[derive(Clone)]
struct PlannedItemLocation {
    position_path: Vec<ctx_traits_core::procedure::runtime::PathSegment>,
}

impl PlannedItemLocation {
    fn root(item: &ctx_traits_core::procedure::run::PlannedSequenceItem) -> Self {
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
fn flatten_step(
    item: &ctx_traits_core::procedure::run::PlannedSequenceItem,
    location: &PlannedItemLocation,
    session: &ctx_traits_core::procedure::session::Session,
    harness_by_role: &BTreeMap<String, Vec<(Option<u32>, String)>>,
    accepted: &BTreeSet<String>,
    force_done: bool,
) -> Vec<RunStep> {
    let step = step_from_item(
        item,
        location,
        session,
        harness_by_role,
        accepted,
        force_done,
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

fn child_location(
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

fn parallel_child_location(
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

fn structural_path_matches(
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
fn structural_path_key(
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

fn structural_step_key(
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

fn stamp_control_stack_iterations(
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
fn loop_container_key(
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
fn active_loop_container_keys(
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
    let mut status_text = step_status_text(session, runtime_status, activity, item, location);
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
        position_path: stamped_path,
        run_index: item.run_index,
        structured_count: 0,
        summary: None,
        summary_at: None,
    }
}

fn counts_progress(item: &ctx_traits_core::procedure::run::PlannedSequenceItem) -> bool {
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
/// it opened (1-based for display).
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
) -> String {
    if session.completion.is_none() {
        match activity {
            Activity::Current => {
                // A structured stop names the real state; "blocked" alone
                // reads like a wait, not an ended run.
                if let Some(stop) = session.stop_reason.as_ref() {
                    return stop.reason.clone();
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

fn status_tone(step: &RunStep) -> tui::Tone {
    match step.state {
        StepState::Done => tui::Tone::Pass,
        StepState::Failed => tui::Tone::Fail,
        StepState::Running => tui::Tone::Warn,
        StepState::Pending => tui::Tone::Muted,
    }
}

fn step_key(step: &RunStep) -> String {
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

fn ref_slug(ref_text: &str) -> String {
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
fn harness_by_role(
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

fn accepted_refs(session: &ctx_traits_core::procedure::session::Session) -> BTreeSet<String> {
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

/// P552: the trait name and accepted-input text a session-title prompt is
/// built from, independent of a live `RunPanel` — the drive-side title
/// dispatch (which must also run for panel-less surfaces such as a
/// dashboard-spawned `--progress none` drive) derives its prompt context
/// through this function rather than requiring a panel to exist first.
pub(crate) fn title_prompt_context_for(
    trait_name: &str,
    session: &ctx_traits_core::procedure::session::Session,
) -> (String, String) {
    (trait_name.to_string(), input_text(session))
}

fn input_text(session: &ctx_traits_core::procedure::session::Session) -> String {
    let inputs: Vec<String> = session
        .accepted_port_values
        .iter()
        .filter(|value| {
            value.acceptance == ctx_traits_core::procedure::runtime::AcceptanceStatus::Accepted
        })
        .map(|value| {
            let id = value
                .ref_text
                .strip_prefix("port:")
                .unwrap_or(&value.ref_text);
            format!("{id} {}", value_text(&value.value))
        })
        .collect();
    if inputs.is_empty() {
        "not provided".to_string()
    } else {
        inputs.join(" · ")
    }
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => clean_value_text(text),
        _ => serde_json::to_string(value).unwrap_or_else(|_| "<unrenderable>".to_string()),
    }
}

fn clean_value_text(text: &str) -> String {
    tui::clean_live_text(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Human summary of a structured control-flow stop: where the run stopped, how
/// many loop iterations ran, and which signals the stop emitted.
pub(crate) fn stop_reason_summary(
    session: &ctx_traits_core::procedure::session::Session,
) -> Option<String> {
    let stop = session.stop_reason.as_ref()?;
    let mut summary = stop.reason.clone();
    let iteration = stop
        .at
        .iter()
        .rev()
        .find_map(|segment| segment.iteration)
        .map(|iteration| iteration + 1);
    // The outermost segment names the stopped sequence item (the loop as the
    // trait declares it); deeper segments name its body internals.
    if let Some(segment) = stop.at.first() {
        let name = segment.id.as_deref().unwrap_or("unnamed");
        match iteration {
            Some(iteration) => summary.push_str(&format!(
                " — loop {name} stopped after iteration {iteration}"
            )),
            None => summary.push_str(&format!(" — at {name}")),
        }
    }
    let signals = session
        .emitted_signals
        .iter()
        .map(|signal| signal.signal_ref.as_str().to_string())
        .collect::<Vec<_>>();
    if !signals.is_empty() {
        summary.push_str(&format!(" · emitted {}", signals.join(", ")));
    }
    Some(summary)
}

pub(crate) fn phase_text(session: &ctx_traits_core::procedure::session::Session) -> String {
    match session.current_sequence_title.as_deref() {
        Some(title) if !title.trim().is_empty() => {
            format!("{} · {}", session_status(&session.status), title)
        }
        _ => session_status(&session.status).to_string(),
    }
}

fn active_label(session: &ctx_traits_core::procedure::session::Session) -> String {
    let role = session
        .current_agent
        .as_ref()
        .map(|agent| agent.role.as_str())
        .unwrap_or(ctx_traits_io::harness_config::DEFAULT_SEAT);
    let structural_seat = session
        .current_agent
        .as_ref()
        .and_then(|agent| agent.structural_seat);
    let harness = session
        .provenance
        .agent_assignments
        .as_ref()
        .and_then(|assignments| {
            ctx_traits_core::procedure::session::select_agent_assignment(
                assignments,
                role,
                structural_seat,
            )
        })
        .map(|assignment| assignment.harness.as_str())
        .unwrap_or("unassigned");
    format!("in-progress {role}@{harness}")
}

fn harness_summary(harness_by_role: &BTreeMap<String, Vec<(Option<u32>, String)>>) -> String {
    if harness_by_role.is_empty() {
        return "unassigned".to_string();
    }
    harness_by_role
        .iter()
        .map(|(role, rows)| format!("{role}→{}", harness_joined(rows)))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Joined display text for every one of `role`'s configured seats' harness,
/// in seat order — the header's per-role summary, which (unlike a single
/// planned row's harness) legitimately reports every distinct configured
/// harness at once.
fn harness_joined(rows: &[(Option<u32>, String)]) -> String {
    if rows.is_empty() {
        return "unassigned".to_string();
    }
    rows.iter()
        .map(|(_, harness)| harness.as_str())
        .collect::<Vec<_>>()
        .join("/")
}

fn active_key(session: &ctx_traits_core::procedure::session::Session) -> Option<String> {
    session.next_frame.as_ref().map(|frame| {
        structural_step_key(
            frame.run_index.unwrap_or(session.current_run_index),
            frame.item_id.as_deref().unwrap_or(""),
            &frame.position_path,
            frame
                .assigned_agent
                .as_ref()
                .map(|agent| agent.role.as_str())
                .unwrap_or(ctx_traits_io::harness_config::DEFAULT_SEAT),
        )
    })
}

pub(crate) fn session_status(status: &ctx_traits_core::procedure::session::Status) -> &'static str {
    match status {
        ctx_traits_core::procedure::session::Status::AwaitingInput => "awaiting-input",
        ctx_traits_core::procedure::session::Status::WaitingOnHuman => "waiting-on-human",
        ctx_traits_core::procedure::session::Status::AwaitingAgentOutput => "in-progress",
        ctx_traits_core::procedure::session::Status::BlockedCommandPermissionRequired => {
            "blocked-command-permission-required"
        }
        ctx_traits_core::procedure::session::Status::BlockedAgentUnassigned => {
            "blocked-agent-unassigned"
        }
        ctx_traits_core::procedure::session::Status::Rejected => "rejected",
        ctx_traits_core::procedure::session::Status::Blocked => "blocked",
        ctx_traits_core::procedure::session::Status::Completed => "completed",
        ctx_traits_core::procedure::session::Status::Failed => "failed",
    }
}

fn output_port_status(
    status: &ctx_traits_core::procedure::runtime::OutputPortStatus,
) -> &'static str {
    match status {
        ctx_traits_core::procedure::runtime::OutputPortStatus::Accepted => "accepted",
        ctx_traits_core::procedure::runtime::OutputPortStatus::Missing => "missing",
        ctx_traits_core::procedure::runtime::OutputPortStatus::OptionalMissing => {
            "optional-missing"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashboard_handoff_is_immediate_and_q_keeps_confirmation_behavior() {
        let bare_d = KeyEvent::new(KeyCode::Char('d'), crossterm::event::KeyModifiers::NONE);
        let bare_q = KeyEvent::new(KeyCode::Char('q'), crossterm::event::KeyModifiers::NONE);
        let modified_d = KeyEvent::new(KeyCode::Char('d'), crossterm::event::KeyModifiers::CONTROL);
        let modified_q = KeyEvent::new(KeyCode::Char('q'), crossterm::event::KeyModifiers::CONTROL);

        assert_eq!(
            live_view_key_action(&bare_d),
            Some(LiveViewKeyAction::OpenDashboard)
        );
        assert_eq!(
            live_view_key_action(&bare_q),
            Some(LiveViewKeyAction::ConfirmQuit)
        );
        assert_eq!(live_view_key_action(&modified_d), None);
        assert_eq!(
            live_view_key_action(&modified_q),
            Some(LiveViewKeyAction::ConfirmQuit)
        );
    }

    #[test]
    fn dashboard_handoff_lifecycle_drives_a_render_drained_request_once() {
        let handoff = DashboardHandoff {
            state: Mutex::new(DashboardHandoffState::default()),
        };
        let launches = Arc::new(AtomicU64::new(0));

        // This models a render draining `d`: it records the request while the
        // panel lock is held, then the post-unlock driver observes it.
        handoff.request("session".to_string(), None);
        for _ in 0..2 {
            let launches = Arc::clone(&launches);
            handoff.drive_with(move |_, _| {
                launches.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(|| {})
            });
        }
        handoff.close();

        assert_eq!(launches.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dashboard_handoff_lifecycle_close_joins_a_published_launch() {
        let handoff = Arc::new(DashboardHandoff {
            state: Mutex::new(DashboardHandoffState::default()),
        });
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        handoff.request("session".to_string(), None);
        handoff.drive_with(move |_, _| {
            std::thread::spawn(move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            })
        });
        started_rx.recv().unwrap();

        let closing = Arc::clone(&handoff);
        let close = std::thread::spawn(move || closing.close());
        release_tx.send(()).unwrap();
        close.join().unwrap();

        let state = handoff.state.lock().unwrap();
        assert!(state.closing);
        assert!(state.dashboard.is_none());
        assert!(state.pending_session.is_none());
    }

    #[test]
    fn dashboard_handoff_lifecycle_close_waits_for_handle_publication() {
        let handoff = Arc::new(DashboardHandoff {
            state: Mutex::new(DashboardHandoffState::default()),
        });
        let (launch_entered_tx, launch_entered_rx) = std::sync::mpsc::channel();
        let (allow_publication_tx, allow_publication_rx) = std::sync::mpsc::channel();
        let (close_entered_tx, close_entered_rx) = std::sync::mpsc::channel();
        let (dashboard_started_tx, dashboard_started_rx) = std::sync::mpsc::channel();
        let (release_dashboard_tx, release_dashboard_rx) = std::sync::mpsc::channel();
        handoff.request("session".to_string(), None);

        let driving = Arc::clone(&handoff);
        let drive = std::thread::spawn(move || {
            driving.drive_with(move |_, _| {
                launch_entered_tx.send(()).unwrap();
                allow_publication_rx.recv().unwrap();
                std::thread::spawn(move || {
                    dashboard_started_tx.send(()).unwrap();
                    release_dashboard_rx.recv().unwrap();
                })
            });
        });
        launch_entered_rx.recv().unwrap();

        let closing = Arc::clone(&handoff);
        let close = std::thread::spawn(move || {
            closing.close_after(|| close_entered_tx.send(()).unwrap());
        });
        close_entered_rx.recv().unwrap();

        // `drive_with` still owns the lifecycle mutex, so close cannot take an
        // empty handle slot before this release publishes the dashboard thread.
        allow_publication_tx.send(()).unwrap();
        dashboard_started_rx.recv().unwrap();
        release_dashboard_tx.send(()).unwrap();
        drive.join().unwrap();
        close.join().unwrap();

        let state = handoff.state.lock().unwrap();
        assert!(state.closing);
        assert!(state.dashboard.is_none());
        assert!(state.pending_session.is_none());
    }

    fn activity_event(frame_id: &str, kind: ActivityKind, text: Option<&str>) -> ActivityEvent {
        ActivityEvent {
            sequence: 0,
            frame_id: frame_id.to_string(),
            kind,
            text: text.map(str::to_string),
            tool: None,
            tokens: None,
        }
    }

    // P549: a fresh stage-boundary event (`Dispatching`) starts a Running
    // row; a `ValidatingOutput` FrameRecorded event closes that same row
    // (by `frame_id`) as Done, freezing its elapsed time.
    #[test]
    fn fold_merge_event_opens_running_then_closes_done_on_the_same_row() {
        let mut rows: Vec<MergeRowView> = Vec::new();
        let started = Instant::now();
        fold_merge_event(
            &mut rows,
            &activity_event(
                "merge:gates",
                ActivityKind::Dispatching,
                Some("starting gates"),
            ),
            started,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state, MergeRowState::Running);
        assert!(rows[0].finished.is_none());

        let finished = started + Duration::from_secs(5);
        fold_merge_event(
            &mut rows,
            &activity_event(
                "merge:gates",
                ActivityKind::ValidatingOutput,
                Some("gates passed"),
            ),
            finished,
        );
        assert_eq!(rows.len(), 1, "same frame_id must fold into one row");
        assert_eq!(rows[0].state, MergeRowState::Done);
        assert_eq!(rows[0].detail.as_deref(), Some("gates passed"));
        assert_eq!(rows[0].finished, Some(finished));
    }

    // P549: `Stalled` on any frame_id OTHER than "merge:lock" is a terminal
    // park/failure — the row must read Failed, never Running.
    #[test]
    fn fold_merge_event_stalled_marks_failed_except_at_the_lock_frame() {
        let mut rows: Vec<MergeRowView> = Vec::new();
        let now = Instant::now();
        fold_merge_event(
            &mut rows,
            &activity_event(
                "merge:gates",
                ActivityKind::Stalled,
                Some("post-run gate failed"),
            ),
            now,
        );
        assert_eq!(rows[0].state, MergeRowState::Failed);

        fold_merge_event(
            &mut rows,
            &activity_event(
                "merge:lock",
                ActivityKind::Stalled,
                Some("waiting for the merge lock"),
            ),
            now,
        );
        let lock_row = rows
            .iter()
            .find(|row| row.frame_id == "merge:lock")
            .unwrap();
        assert_eq!(lock_row.state, MergeRowState::Running);
    }

    // P549 blocker `merge-rows-do-not-track-stage-progression`: on a
    // happy-path merge, most stages record no ledger frame until the NEXT
    // stage's first event arrives — the only progression signal a live
    // merge has. A stage entered earlier must read Done once a later
    // stage's row appears, not stay Running forever; `merge:gates` (the
    // final `StageEntered` before this trace's terminal `Merged`-frame
    // stand-in) is left Running here on purpose — this test traces
    // `StageEntered` events only, exactly what a park-free happy path emits
    // for every stage but the last, whose own terminal frame closes it.
    #[test]
    fn fold_merge_event_closes_the_prior_stage_row_when_the_next_stage_starts() {
        let mut rows: Vec<MergeRowView> = Vec::new();
        let now = Instant::now();
        let stage_order = [
            "merge:lock",
            "merge:preflight",
            "merge:rebase",
            "merge:reconciliation",
            "merge:gates",
            "merge:post-run",
            "merge:cleanup",
        ];
        for (index, frame_id) in stage_order.iter().enumerate() {
            fold_merge_event(
                &mut rows,
                &activity_event(
                    frame_id,
                    ActivityKind::Dispatching,
                    Some(&format!("starting {frame_id}")),
                ),
                now + Duration::from_secs(index as u64),
            );
        }
        assert_eq!(rows.len(), stage_order.len());
        for frame_id in &stage_order[..stage_order.len() - 1] {
            let row = rows.iter().find(|row| &row.frame_id == frame_id).unwrap();
            assert_eq!(
                row.state,
                MergeRowState::Done,
                "{frame_id} must read Done once a later stage's row appeared"
            );
            assert!(row.finished.is_some());
        }
        let last = rows.last().unwrap();
        assert_eq!(last.frame_id, "merge:cleanup");
        assert_eq!(
            last.state,
            MergeRowState::Running,
            "the most recently entered stage stays Running until its own terminal frame"
        );
        assert!(last.finished.is_none());
    }

    fn step(key: &str, state: StepState, summary: Option<&str>) -> RunStep {
        RunStep {
            key: key.to_string(),
            label: key.to_string(),
            role: "worker".to_string(),
            harness: None,
            tags: Vec::new(),
            status: "done".to_string(),
            state,
            active: false,
            counts_progress: true,
            inputs: Vec::new(),
            outputs: Vec::new(),
            elapsed: Some(Duration::from_secs(5)),
            output_tokens: Some(1_200),
            loop_key: None,
            position_path: Vec::new(),
            run_index: 0,
            structured_count: 0,
            summary: summary.map(str::to_string),
            summary_at: summary.map(|_| Duration::from_secs(7)),
        }
    }

    fn history_step(label: &str, elapsed: Option<Duration>, summary: Option<&str>) -> HistoryStep {
        HistoryStep {
            label: label.to_string(),
            kind: None,
            outcome: None,
            elapsed,
            output_tokens: Some(1_200),
            summary: summary.map(str::to_string),
            summary_at: summary.map(|_| Duration::from_secs(7)),
        }
    }

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
            max_items: None,
            item_index: None,
            item_total: None,
            over_slot: None,
            item_slot: None,
            list_digest: None,
            concurrent: false,
            until: None,
            stop_if: None,
            on_exhausted: None,
            on_stop: None,
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
    fn story_row_line_keeps_typed_verdicts_and_omits_unknown_command_facts() {
        use ctx_traits_core::procedure::run::PlannedSequenceKind;

        let check = HistoryStep {
            label: "check".to_string(),
            kind: Some(PlannedSequenceKind::Check),
            outcome: Some(HistoryOutcome::Check {
                ok: false,
                exit_code: Some(7),
            }),
            elapsed: Some(Duration::from_secs(5)),
            output_tokens: Some(10),
            summary: Some("summary must not suppress verdict".to_string()),
            summary_at: Some(Duration::from_secs(9)),
        };
        let rendered = story_row_line(&check);
        assert_eq!(rendered.tone, tui::Tone::Fail);
        assert!(rendered.tail.contains("failed (exit 7)"));
        assert!(!rendered.tail.contains("summary"));

        let command = HistoryStep {
            label: "command".to_string(),
            kind: Some(PlannedSequenceKind::Command),
            outcome: None,
            elapsed: Some(Duration::from_secs(5)),
            output_tokens: Some(10),
            summary: None,
            summary_at: None,
        };
        let rendered = story_row_line(&command);
        assert_eq!(rendered.tail, "command");
        assert!(!rendered.tail.contains("00:00:05"));

        let check = |ok, exit_code, summary: Option<&str>| {
            story_row_line(&HistoryStep {
                label: "check".to_string(),
                kind: Some(PlannedSequenceKind::Check),
                outcome: Some(HistoryOutcome::Check { ok, exit_code }),
                elapsed: Some(Duration::from_secs(5)),
                output_tokens: Some(10),
                summary: summary.map(str::to_string),
                summary_at: None,
            })
        };
        let passed = check(true, Some(0), None);
        assert_eq!(passed.tone, tui::Tone::Muted);
        assert!(passed.tail.contains("passed"));
        assert!(!passed.tail.contains("exit"));
        let failed = check(false, Some(9), Some("must not replace verdict"));
        assert_eq!(failed.tone, tui::Tone::Fail);
        assert!(failed.tail.contains("failed (exit 9)"));
        assert!(!failed.tail.contains("replace verdict"));

        let unknown_check = story_row_line(&HistoryStep {
            label: "check".to_string(),
            kind: Some(PlannedSequenceKind::Check),
            outcome: None,
            elapsed: None,
            output_tokens: None,
            summary: None,
            summary_at: None,
        });
        assert_eq!(unknown_check.tone, tui::Tone::Muted);
        assert_eq!(unknown_check.tail, "check");

        let command = |succeeded, exit_code| {
            story_row_line(&HistoryStep {
                label: "command".to_string(),
                kind: Some(PlannedSequenceKind::Command),
                outcome: Some(HistoryOutcome::Command {
                    succeeded,
                    exit_code,
                }),
                elapsed: Some(Duration::from_secs(5)),
                output_tokens: Some(10),
                summary: Some("summary".to_string()),
                summary_at: None,
            })
        };
        let succeeded = command(true, Some(0));
        assert_eq!(succeeded.tone, tui::Tone::Muted);
        assert!(succeeded.tail.contains("succeeded"));
        let failed = command(false, Some(2));
        assert_eq!(failed.tone, tui::Tone::Fail);
        assert!(failed.tail.contains("failed (exit 2)"));
    }

    fn view_with(steps: Vec<RunStep>) -> RunView {
        RunView {
            header: RunHeader {
                session_id: "session-0123456789abcdef".to_string(),
                run_id: "run-fedcba9876543210".to_string(),
                input: "not provided".to_string(),
                harnesses: "unassigned".to_string(),
                done: 0,
                total: 0,
                phase: "in-progress".to_string(),
                completed: false,
                stopped: None,
                stop_detail: None,
                state_digest: String::new(),
                harness_count: 0,
                elapsed: None,
                output_tokens: None,
                narrator_tokens: None,
                guide_tokens: None,
                structured_count: 0,
                structured_label: None,
                structured_verdict: None,
            },
            steps,
            history: Vec::new(),
            narration: None,
            outputs: Vec::new(),
            merge_rows: Vec::new(),
        }
    }

    fn line_text(line: &tui::Line) -> String {
        line.segments().map(|(text, _)| text).collect()
    }

    #[test]
    fn progress_lines_show_compact_session_and_run_identifiers() {
        let lines = progress_lines(&view_with(Vec::new()));
        assert_eq!(line_text(&lines[0]), "session 0123456789ab");
        assert_eq!(line_text(&lines[1]), "run fedcba987654");
    }

    #[test]
    fn post_run_morph_requires_completion_and_observed_merge_work() {
        let merge_row = MergeRowView {
            frame_id: "merge:post-run".to_string(),
            label: "post-run".to_string(),
            state: MergeRowState::Done,
            detail: None,
            started: Instant::now(),
            finished: Some(Instant::now()),
        };
        let mut incomplete = view_with(Vec::new());
        incomplete.merge_rows.push(merge_row.clone());
        assert!(post_run_lines(&incomplete).is_none());

        let mut no_work = view_with(Vec::new());
        no_work.header.completed = true;
        assert!(post_run_lines(&no_work).is_none());

        incomplete.header.completed = true;
        assert!(post_run_lines(&incomplete).is_some());
    }

    #[test]
    fn persisted_post_run_rows_fold_stages_and_preserve_failures() {
        use ctx_traits_core::procedure::session::{MergeFrame, MergeStage, MergeStatus};

        let frame = |stage, status| MergeFrame {
            stage,
            status,
            reason: None,
            evidence: Vec::new(),
            park_reason: None,
            deep_decisions: Vec::new(),
        };
        let rows = post_run_lines_from_frames(
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
        assert!(line_text(&rows[3]).starts_with("✓ post-run"));
    }

    #[test]
    fn compact_identifier_handles_short_and_nonstandard_values() {
        assert_eq!(compact_identifier("session-short", "session-"), "short");
        assert_eq!(compact_identifier("custom-id", "session-"), "custom-id");
    }

    // P470's plan projection still only supplies journey rows; history is
    // now populated from accepted ledger executions.
    #[test]
    fn story_history_lines_only_covers_done_steps_in_plan_order() {
        let mut view = view_with(vec![
            step("a", StepState::Done, None),
            step("b", StepState::Running, None),
            step("c", StepState::Done, Some("finished cleanly")),
            step("d", StepState::Pending, None),
        ]);
        view.history = vec![
            history_step("a", Some(Duration::from_secs(5)), None),
            history_step("c", Some(Duration::from_secs(7)), Some("finished cleanly")),
        ];
        let lines = story_history_lines(&view);
        assert_eq!(lines.len(), 2);
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
                .starts_with("00:00:10 ")
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
            max_items: None,
            item_index: None,
            item_total: None,
            over_slot: None,
            item_slot: None,
            list_digest: None,
            concurrent: false,
            until: None,
            stop_if: None,
            on_exhausted: None,
            on_stop: None,
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
            assert!(text.starts_with(&format!("00:00:{elapsed_seconds:02} {title}")));
        }
    }

    // A step with a landed P455 summary joins its row; one without falls
    // back to the truthful facts line — never a placeholder.
    #[test]
    fn story_row_line_prefers_summary_over_facts_fallback() {
        let with_summary = story_row_line(&history_step(
            "a",
            Some(Duration::from_secs(5)),
            Some("did the thing"),
        ));
        assert_eq!(with_summary.at, Some(Duration::from_secs(7)));
        assert!(with_summary.tail.contains("did the thing"));

        let without_summary =
            story_row_line(&history_step("b", Some(Duration::from_secs(5)), None));
        assert_eq!(without_summary.at, Some(Duration::from_secs(5)));
        assert!(without_summary.tail.contains('5')); // elapsed
        assert!(without_summary.tail.contains("tok")); // tokens
    }

    #[test]
    fn stream_row_line_tones_narration_and_model_text_differently() {
        let narration = stream_row_line(&StreamRow {
            at: Duration::from_secs(3),
            kind: StreamRowKind::Narration,
            text: "thinking about it".to_string(),
        });
        let model_text = stream_row_line(&StreamRow {
            at: Duration::from_secs(3),
            kind: StreamRowKind::ModelText,
            text: "raw delta".to_string(),
        });
        assert_eq!(narration.at, Some(Duration::from_secs(3)));
        assert_eq!(model_text.at, Some(Duration::from_secs(3)));
        assert_eq!(narration.tone, tui::Tone::Default);
        assert_eq!(model_text.tone, tui::Tone::Muted);
    }

    #[test]
    fn entered_step_text_uses_the_active_ledger_step_and_initialization_phase() {
        let mut entered = step("stale", StepState::Done, None);
        entered.label = "Stale step".to_string();
        let mut active = step("build:iteration:2", StepState::Running, None);
        active.label = "Build artifact".to_string();
        active.active = true;
        let view = view_with(vec![entered, active]);

        assert_eq!(
            entered_step_text(&view, "Initialization").as_deref(),
            Some("[Build artifact] (Initialization)")
        );
    }

    #[test]
    fn entered_step_text_omits_an_empty_phase_and_uses_each_loop_iteration_key() {
        let mut first_round = step("work:iteration:0", StepState::Running, None);
        first_round.label = "Repeat work".to_string();
        first_round.active = true;
        let mut second_round = step("work:iteration:1", StepState::Running, None);
        second_round.label = "Repeat work".to_string();
        second_round.active = true;

        let first = view_with(vec![first_round]);
        let second = view_with(vec![second_round]);
        assert_ne!(first.steps[0].key, second.steps[0].key);
        assert_eq!(
            entered_step_text(&first, "").as_deref(),
            Some("[Repeat work]")
        );
        assert_eq!(
            entered_step_text(&second, "Initialization").as_deref(),
            Some("[Repeat work] (Initialization)")
        );
    }

    // P552: every history/current-activity row shares one formatter
    // (`event_row_line`) that reserves the fixed `HH:MM:SS ` prefix and
    // truncates only the tail, by display width, so wide/combining Unicode
    // never desyncs the truncation point from a plain byte/char count.
    #[test]
    fn event_row_line_truncates_only_the_tail_by_display_width() {
        let row = EventRow {
            at: Some(Duration::from_secs(5)),
            tail: "a".repeat(50),
            tone: tui::Tone::Default,
        };
        let line = event_row_line(&row, 20);
        let rendered: String = line.segments().map(|(text, _)| text).collect();
        assert!(rendered.starts_with("00:00:05 "));
        assert!(rendered.ends_with("..."));
        assert!(tui::display_width(&rendered) <= 20);
    }

    #[test]
    fn event_row_line_wide_unicode_tail_truncates_by_display_width_not_char_count() {
        // Each "文" is 2 display columns; a char-count truncation would
        // overflow the requested width, a display-width one will not.
        let row = EventRow {
            at: Some(Duration::from_secs(1)),
            tail: "文".repeat(30),
            tone: tui::Tone::Default,
        };
        let line = event_row_line(&row, 25);
        let rendered: String = line.segments().map(|(text, _)| text).collect();
        assert!(tui::display_width(&rendered) <= 25);
        assert!(rendered.ends_with("..."));
    }

    #[test]
    fn event_row_line_leaves_a_short_tail_unmarked() {
        let row = EventRow {
            at: Some(Duration::from_secs(9)),
            tail: "short".to_string(),
            tone: tui::Tone::Default,
        };
        let line = event_row_line(&row, 80);
        let rendered: String = line.segments().map(|(text, _)| text).collect();
        assert!(rendered.starts_with("00:00:09 short"));
        assert!(!rendered.ends_with("..."));
    }

    #[test]
    fn event_row_line_without_timestamp_uses_the_full_tail_budget() {
        let row = EventRow {
            at: None,
            tail: "a".repeat(30),
            tone: tui::Tone::Default,
        };
        let line = event_row_line(&row, 20);
        let rendered: String = line.segments().map(|(text, _)| text).collect();
        assert!(!rendered.contains("00:00:00"));
        assert!(!rendered.starts_with(EVENT_PREFIX_SEP));
        assert_eq!(rendered, "a".repeat(17) + "...");
        assert_eq!(tui::display_width(&rendered), 20);
    }

    #[test]
    fn latest_frame_current_activity_keeps_a_known_zero_timestamp() {
        let activity = ctx_traits_core::procedure::story::ActivityInput {
            events: vec![ctx_traits_core::procedure::story::TimedActivityEvent {
                at_epoch_ms: 0,
                event: activity_event("frame", ActivityKind::RunningTool, Some("working")),
            }],
            step_summaries: Vec::new(),
            skipped_lines: 0,
        };
        let rows = latest_frame_event_rows(&activity, None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].at, Some(Duration::ZERO));
        let rendered = line_text(&event_row_line(&rows[0], 80));
        assert!(rendered.starts_with("00:00:00 working"));
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
schema-version = "0.2"
version = "0.1.0"
name = "History Test"
summary = "A test trait."
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
schema-version = "0.2"
version = "0.1.0"
name = "Clock Test"
summary = "A test trait."
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

    /// P081 regression: an observer's ask-refusal notice (the `?`-without-a-
    /// handle branch in `poll_and_apply_keys`, run_view.rs:1521) must survive
    /// `RunPanel::refresh_from_ledger`'s periodic `apply_ledger_seed`, which
    /// otherwise rebuilds `current_stream` wholesale from the sidecar and
    /// erases anything pushed there directly — turning the draft's "visible
    /// refusal, never silence" rule into silence after one poll interval.
    /// `observer_notice` is the retained copy `apply_ledger_seed` re-appends
    /// after every rebuild; this constructs the SAME state the key handler
    /// would (rather than driving a real key through the pump, which the
    /// detached test pane cannot forward) and proves the notice is still
    /// present in `current_stream` after a refresh.
    #[test]
    fn observer_ask_refusal_survives_a_ledger_refresh() {
        let trait_ref: ctx_traits_core::Trait = toml::from_str(
            r#"
id = "ask-refusal-test"
schema-version = "0.2"
version = "0.1.0"
name = "Ask Refusal Test"
summary = "A test trait."
"#,
        )
        .expect("minimal trait parses");
        let plan = attribution_plan(vec![planned_item(
            "check",
            ctx_traits_core::procedure::run::PlannedSequenceKind::Check,
            0,
            0,
        )]);
        let session = session_with_history_revisions(Vec::new(), Vec::new());
        let ledger_path = camino::Utf8PathBuf::from(format!(
            "/tmp/ctx-traits-run-view-ask-refusal-{}.json",
            std::process::id()
        ));

        let panel = RunPanel::new_observer(
            "ask-refusal-test".to_string(),
            trait_ref,
            plan,
            session.clone(),
            ledger_path.clone(),
            RatatuiPane::new_detached_for_test(),
        );

        let refusal_text = "ask refused: this run is driven by another process — ask there";
        {
            let mut state = panel.state.lock().expect("state lock");
            assert!(state.observer && state.ask.is_none());
            let notice = StreamRow {
                at: state.run_started.elapsed(),
                kind: StreamRowKind::Narration,
                text: refusal_text.to_string(),
            };
            state.observer_notice = Some(notice.clone());
            state.current_stream.push_back(notice);
        }

        panel.refresh_from_ledger(&session, &ledger_path);

        let state = panel.state.lock().expect("state lock");
        assert!(
            state
                .current_stream
                .iter()
                .any(|row| row.text == refusal_text),
            "the ask refusal must still be present after a ledger refresh, got {:?}",
            state
                .current_stream
                .iter()
                .map(|row| row.text.as_str())
                .collect::<Vec<_>>()
        );
    }

    // P552: the CURRENT pane's in-flight line is not a special overlay
    // outside the event model — it folds into an `EventRow` using the
    // current run-relative timestamp, so it renders through the exact same
    // `event_row_line` prefix/truncation contract as every recorded event.
    #[test]
    fn overlay_event_row_uses_the_shared_event_contract() {
        let narration = RunNarration {
            label: "narrator".to_string(),
            text: "文".repeat(30),
            muted: false,
            finished: false,
        };
        let row = overlay_event_row(&narration, None, Duration::from_secs(65))
            .expect("narration text differs from last recorded row");
        assert_eq!(row.at, Some(Duration::from_secs(65)));
        assert!(row.tail.starts_with("narrator: "));
        let line = event_row_line(&row, 25);
        let rendered: String = line.segments().map(|(text, _)| text).collect();
        assert!(rendered.starts_with("00:01:05 "));
        assert!(tui::display_width(&rendered) <= 25);
        assert!(rendered.ends_with("..."));
    }

    #[test]
    fn overlay_event_row_suppresses_a_duplicate_of_the_last_recorded_row() {
        let narration = RunNarration {
            label: "narrator".to_string(),
            text: "same text".to_string(),
            muted: false,
            finished: true,
        };
        assert!(overlay_event_row(&narration, Some("same text"), Duration::from_secs(1)).is_none());
    }

    // P552: each logical history/current-activity input becomes exactly one
    // physical row — no `wrapped_lines` pass may re-split it.
    #[test]
    fn event_row_lines_produces_exactly_one_row_per_input() {
        let rows = vec![
            EventRow {
                at: Some(Duration::from_secs(1)),
                tail: "one".to_string(),
                tone: tui::Tone::Default,
            },
            EventRow {
                at: Some(Duration::from_secs(2)),
                tail: "x".repeat(200),
                tone: tui::Tone::Default,
            },
        ];
        let lines = event_row_lines(&rows, 30);
        assert_eq!(lines.len(), rows.len());
    }

    // P470 blocker `tree-follow-anchor-unit-mismatch`: the follow anchor
    // handed to the pane must be the RENDERED ROW index of the active step's
    // `render_step_summary` line, never its `view.steps` item index — each
    // step group is 3 rows (summary + two port lines) under a multi-row
    // header, so the two coordinate spaces disagree for any step past the
    // first.
    #[test]
    fn render_tree_lines_with_active_row_returns_a_row_index_not_a_step_index() {
        let mut active_step = step("c", StepState::Running, None);
        active_step.active = true;
        let view = view_with(vec![
            step("a", StepState::Done, None),
            step("b", StepState::Done, None),
            active_step,
            step("d", StepState::Pending, None),
        ]);
        let (lines, active_row) = journey_lines_with_active_row(&view);
        let active_row = active_row.expect("an active step must yield a row anchor");
        assert_eq!(active_row, 2, "each preceding step contributes one row");
        let rendered = journey_row_lines(&lines, 120);
        let label = rendered[active_row]
            .segments()
            .nth(2)
            .map(|(text, _)| text.to_string());
        assert_eq!(
            label,
            Some("c".to_string()),
            "row {active_row} must be the active step's own render_step_summary line"
        );
    }

    #[test]
    fn journey_content_omits_the_pane_title() {
        let lines = journey_lines(&view_with(vec![step("work", StepState::Running, None)]));
        assert!(
            journey_row_lines(&lines, 120)
                .iter()
                .all(|line| line_text(line) != "journey"),
            "the pane border is the only journey title"
        );
    }

    #[test]
    fn journey_step_full_line_labels_agent_variant_and_facts() {
        let mut step = step("deploy", StepState::Running, None);
        step.role = "reviewer".to_string();
        step.harness = Some("codex".to_string());
        step.tags = vec!["fast".to_string(), "branch-a".to_string()];
        step.status = "running".to_string();
        step.structured_count = 2;
        assert_eq!(
            line_text(&journey_step_line(&step, 120)),
            "~ deploy · [agent] reviewer@codex · [variant] fast ∙ branch-a · running · 00:00:05 · 1.2k tok · (2 open)"
        );
    }

    #[test]
    fn journey_step_degrades_by_display_width_without_losing_state() {
        let mut step = step("文文 deploy", StepState::Running, None);
        step.harness = Some("codex".to_string());
        step.tags = vec!["fast".to_string(), "branch-a".to_string()];
        step.status = "running".to_string();
        step.structured_count = 2;
        for (width, absent) in [
            (120, ""),
            (65, "[agent]"),
            (45, "branch-a"),
            (30, "worker@codex"),
            (25, "文文 deploy"),
        ] {
            let rendered = line_text(&journey_step_line(&step, width));
            assert!(rendered.contains('~'));
            assert!(rendered.contains("running"));
            assert!(rendered.contains("(2 open)"));
            assert!(tui::display_width(&rendered) <= width as usize);
            if !absent.is_empty() {
                assert!(!rendered.contains(absent), "{rendered}");
            }
        }
    }

    #[test]
    fn journey_steps_are_one_row_and_never_render_ports() {
        let view = view_with(vec![
            step("first", StepState::Done, None),
            step("second", StepState::Pending, None),
        ]);
        let rows = journey_lines(&view);
        let rendered = journey_row_lines(&rows, 120);
        assert_eq!(rendered.len(), 2);
        assert!(rendered.iter().all(|line| {
            let text = line_text(line);
            !text.starts_with("    in ") && !text.starts_with("    out ")
        }));
    }

    #[test]
    fn render_tree_lines_with_active_row_is_none_when_no_step_is_selectable() {
        let (_, active_row) = journey_lines_with_active_row(&view_with(Vec::new()));
        assert_eq!(active_row, None);
    }

    fn sample_lines(n: usize) -> Vec<tui::Line> {
        (0..n)
            .map(|index| {
                let mut line = tui::Line::blank();
                line.push(format!("line{index}"), tui::Tone::Default);
                line
            })
            .collect()
    }

    fn sample_journey_rows(n: usize) -> Vec<JourneyRow> {
        sample_lines(n)
            .into_iter()
            .map(|line| JourneyRow(JourneyRowKind::Line(line)))
            .collect()
    }

    fn sample_event_rows(n: usize) -> Vec<EventRow> {
        (0..n)
            .map(|index| {
                EventRow::new(
                    Some(Duration::from_secs(index as u64)),
                    format!("event{index}"),
                    tui::Tone::Default,
                )
            })
            .collect()
    }

    // P552: a narrow terminal stacks every populated pane instead of
    // dropping any of them — the pre-P552 `live_pane_tree` silently omitted
    // `history` at narrow widths (reviewer blocker `live-run-pane-contract-
    // absent`); this proves the replacement never does.
    #[test]
    fn narrow_full_data_stacks_every_pane_without_omission() {
        let progress = sample_lines(3);
        let journey = sample_journey_rows(5);
        let history = sample_event_rows(10);
        let current = sample_event_rows(4);
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&history),
            current: Some(&current),
            post_run: None,
            title: PaneTitleRow::Visible(&title_row_line(None, "trait", None)),
        };
        let area = Rect::new(0, 0, 80, 24);
        let tree = pane_tree(&LIVE_PANE_IDS, area, &data);
        assert_eq!(
            tree.leaf_ids(),
            vec![PROGRESS_PANE, JOURNEY_PANE, HISTORY_PANE, CURRENT_PANE]
        );
    }

    // Normal-width full data: the bounded-progress 2x2 grid — progress is
    // bounded to its own content height, journey receives the rest of the
    // left column, history retains up to half the right column, and current
    // receives the remainder while keeping its content floor.
    #[test]
    fn wide_full_data_produces_bounded_progress_2x2_grid() {
        let progress = sample_lines(3);
        let journey = sample_journey_rows(50);
        let history = sample_event_rows(100);
        let current = sample_event_rows(4);
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&history),
            current: Some(&current),
            post_run: None,
            title: PaneTitleRow::None,
        };
        let area = Rect::new(0, 0, 120, 24);
        let tree = pane_tree(&LIVE_PANE_IDS, area, &data);
        assert_eq!(
            tree.leaf_ids(),
            vec![PROGRESS_PANE, JOURNEY_PANE, HISTORY_PANE, CURRENT_PANE]
        );
        let layout = tree.resolve(area);
        let progress_rect = layout.rect(PROGRESS_PANE).expect("progress");
        let journey_rect = layout.rect(JOURNEY_PANE).expect("journey");
        let history_rect = layout.rect(HISTORY_PANE).expect("history");
        let current_rect = layout.rect(CURRENT_PANE).expect("current");
        assert_eq!(progress_rect.height, 5, "3 content rows + 2 borders");
        assert!(
            journey_rect.height > progress_rect.height,
            "journey must receive the rest of the left column"
        );
        assert_eq!(history_rect.height, area.height / 2);
        assert_eq!(current_rect.height, area.height - history_rect.height);
        assert!(history_rect.height >= HISTORY_MIN_OUTER_ROWS);
        assert!(current_rect.height >= CURRENT_MIN_OUTER_ROWS);
        assert!(tui_panes::pane_inner(current_rect).height >= 6);
        assert_eq!(progress_rect.x, journey_rect.x, "left column shares an x");
        assert_eq!(history_rect.x, current_rect.x, "right column shares an x");
        assert!(
            history_rect.x > progress_rect.x,
            "right column is to the right"
        );
        assert_eq!(journey_rect.y + journey_rect.height, area.y + area.height);
        assert_eq!(current_rect.y + current_rect.height, area.y + area.height);
        assert_eq!(journey_rect.x + journey_rect.width, history_rect.x);
        assert_eq!(current_rect.x + current_rect.width, area.x + area.width);
    }

    #[test]
    fn post_run_replaces_the_complete_right_column_after_completion() {
        let progress = sample_lines(3);
        let journey = sample_journey_rows(50);
        let history = sample_event_rows(100);
        let current = sample_event_rows(100);
        let post_run = sample_lines(20);
        let area = Rect::new(0, 0, 120, 24);
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&history),
            current: Some(&current),
            post_run: Some(&post_run),
            title: PaneTitleRow::None,
        };
        let tree = pane_tree(&LIVE_PANE_IDS, area, &data);
        let layout = tree.resolve(area);
        assert_eq!(
            tree.leaf_ids(),
            vec![PROGRESS_PANE, JOURNEY_PANE, CURRENT_PANE]
        );
        assert_eq!(tree.title(CURRENT_PANE), Some("post-run"));
        assert!(layout.rect(HISTORY_PANE).is_none());
        let post_run_rect = layout.rect(CURRENT_PANE).expect("post-run");
        assert_eq!(post_run_rect.y, area.y);
        assert_eq!(post_run_rect.height, area.height);
        assert_eq!(
            post_run_rect.x,
            layout.rect(PROGRESS_PANE).expect("progress").x + 72
        );
    }

    #[test]
    fn wide_short_history_yields_unused_rows_to_current_activity() {
        let progress = sample_lines(3);
        let journey = sample_journey_rows(50);
        let history = sample_event_rows(4);
        let current = sample_event_rows(4);
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&history),
            current: Some(&current),
            post_run: None,
            title: PaneTitleRow::None,
        };
        let area = Rect::new(0, 0, 120, 24);
        let layout = pane_tree(&LIVE_PANE_IDS, area, &data).resolve(area);

        assert_eq!(layout.rect(HISTORY_PANE).expect("history").height, 6);
        assert_eq!(layout.rect(CURRENT_PANE).expect("current").height, 18);
    }

    #[test]
    fn wide_current_activity_length_does_not_change_history_allocation() {
        let progress = sample_lines(3);
        let journey = sample_journey_rows(50);
        let history = sample_event_rows(100);
        let short_current = sample_event_rows(1);
        let long_current = sample_event_rows(100);
        let area = Rect::new(0, 0, 120, 24);
        let short_data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&history),
            current: Some(&short_current),
            post_run: None,
            title: PaneTitleRow::None,
        };
        let long_data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&history),
            current: Some(&long_current),
            post_run: None,
            title: PaneTitleRow::None,
        };

        let short = pane_tree(&LIVE_PANE_IDS, area, &short_data).resolve(area);
        let long = pane_tree(&LIVE_PANE_IDS, area, &long_data).resolve(area);
        assert_eq!(short.rect(HISTORY_PANE), long.rect(HISTORY_PANE));
    }

    #[test]
    fn smallest_supported_wide_body_keeps_history_and_current_floors() {
        let progress = sample_lines(3);
        let journey = sample_journey_rows(50);
        let history = sample_event_rows(100);
        let current = sample_event_rows(100);
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&history),
            current: Some(&current),
            post_run: None,
            title: PaneTitleRow::None,
        };
        let area = Rect::new(0, 0, 120, HISTORY_MIN_OUTER_ROWS + CURRENT_MIN_OUTER_ROWS);
        let layout = pane_tree(&LIVE_PANE_IDS, area, &data).resolve(area);
        let history = layout.rect(HISTORY_PANE).expect("history");
        let current = layout.rect(CURRENT_PANE).expect("current");

        assert_eq!(history.height, HISTORY_MIN_OUTER_ROWS);
        assert_eq!(current.height, CURRENT_MIN_OUTER_ROWS);
        assert_eq!(history.y + history.height, current.y);
        assert_eq!(current.y + current.height, area.y + area.height);
    }

    // Dashboard preview supplies only progress/journey — this must produce
    // exactly those two leaves, never four.
    #[test]
    fn preview_data_produces_exactly_progress_and_journey() {
        let progress = sample_lines(3);
        let journey = sample_journey_rows(5);
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: None,
            current: None,
            post_run: None,
            title: PaneTitleRow::None,
        };
        let area = Rect::new(0, 0, 120, 24);
        let tree = pane_tree(&LIVE_PANE_IDS, area, &data);
        assert_eq!(tree.leaf_ids(), vec![PROGRESS_PANE, JOURNEY_PANE]);
        assert_eq!(tree.title(PROGRESS_PANE), Some("information"));
        assert_eq!(tree.title(JOURNEY_PANE), Some("journey"));
    }

    #[test]
    fn information_title_and_pane_ids_survive_the_narrow_breakpoint() {
        let progress = sample_lines(5);
        let journey = sample_journey_rows(5);
        let history = sample_event_rows(5);
        let current = sample_event_rows(5);
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&history),
            current: Some(&current),
            post_run: None,
            title: PaneTitleRow::None,
        };
        for width in [108, 109] {
            let area = Rect::new(0, 0, width, 30);
            let tree = pane_tree(&LIVE_PANE_IDS, area, &data);
            assert_eq!(tree.title(PROGRESS_PANE), Some("information"));
            assert_eq!(
                tree.leaf_ids(),
                vec![PROGRESS_PANE, JOURNEY_PANE, HISTORY_PANE, CURRENT_PANE]
            );
            let layout = tree.resolve(area);
            for id in tree.leaf_ids() {
                assert!(
                    layout
                        .rect(id)
                        .is_some_and(|rect| rect.width > 0 && rect.height > 0)
                );
            }
        }
    }

    #[test]
    fn narrow_full_data_tiles_the_supplied_area_to_its_edges() {
        let progress = sample_lines(3);
        let journey = sample_journey_rows(5);
        let history = sample_event_rows(10);
        let current = sample_event_rows(4);
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&history),
            current: Some(&current),
            post_run: None,
            title: PaneTitleRow::None,
        };
        let area = Rect::new(0, 0, 80, 24);
        let layout = pane_tree(&LIVE_PANE_IDS, area, &data).resolve(area);
        let panes = [PROGRESS_PANE, JOURNEY_PANE, HISTORY_PANE, CURRENT_PANE]
            .map(|id| layout.rect(id).expect("all supplied panes resolve"));
        assert!(
            panes
                .iter()
                .all(|rect| rect.x + rect.width == area.x + area.width)
        );
        assert_eq!(panes[0].y, area.y);
        assert_eq!(panes[3].y + panes[3].height, area.y + area.height);
        assert!(
            panes
                .windows(2)
                .all(|pair| pair[0].y + pair[0].height == pair[1].y)
        );
    }

    // Source-driven omission (a legacy session with no activity sidecar):
    // only the missing pane is dropped, and focus reconciliation never
    // lands on an undrawn pane.
    #[test]
    fn source_omission_drops_only_the_missing_pane_leaving_focus_valid() {
        let progress = sample_lines(3);
        let journey = sample_journey_rows(5);
        let current = sample_event_rows(4);
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: None,
            current: Some(&current),
            post_run: None,
            title: PaneTitleRow::None,
        };
        let area = Rect::new(0, 0, 120, 24);
        let tree = pane_tree(&LIVE_PANE_IDS, area, &data);
        assert_eq!(
            tree.leaf_ids(),
            vec![PROGRESS_PANE, JOURNEY_PANE, CURRENT_PANE]
        );
        let layout = tree.resolve(area);
        let ids = drawable_pane_ids(&tree, &layout);
        let mut focus = FocusRing::new(vec![HISTORY_PANE]);
        focus.reconcile(ids.clone(), CURRENT_PANE);
        assert!(focus.current().is_some_and(|id| ids.contains(&id)));
    }

    #[test]
    fn focus_ring_contains_only_drawable_panes_at_small_sizes() {
        let progress = sample_lines(3);
        let journey = sample_journey_rows(50);
        let history = sample_event_rows(100);
        let current = sample_event_rows(4);
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&history),
            current: Some(&current),
            post_run: None,
            title: PaneTitleRow::None,
        };
        for area in [
            Rect::new(0, 0, 80, 6),
            Rect::new(0, 0, 80, 7),
            Rect::new(0, 0, 120, 6),
            Rect::new(0, 0, 120, 7),
        ] {
            let tree = pane_tree(&LIVE_PANE_IDS, area, &data);
            let layout = tree.resolve(area);
            let ids = drawable_pane_ids(&tree, &layout);
            let mut focus = FocusRing::new(vec![CURRENT_PANE]);
            focus.reconcile(ids.clone(), CURRENT_PANE);
            for _ in 0..ids.len() {
                assert!(ids.contains(&focus.current().expect("drawable focus")));
                focus.next();
            }
        }
    }

    #[test]
    fn pane_scrolls_preserve_independent_offsets() {
        let mut scrolls = PaneScrolls::new();
        scrolls.get_mut(PROGRESS_PANE).set_len(30);
        scrolls
            .get_mut(PROGRESS_PANE)
            .apply(tui_kit::ScrollDelta::Down(12), 10);
        scrolls.get_mut(CURRENT_PANE).set_len(30);
        scrolls
            .get_mut(CURRENT_PANE)
            .apply(tui_kit::ScrollDelta::Down(3), 10);
        assert_eq!(scrolls.get(PROGRESS_PANE).window(10), 12..22);
        assert_eq!(scrolls.get(CURRENT_PANE).window(10), 3..13);
    }

    // P552: proves scroll-key routing against JOURNEY (whose 20-line
    // fixture genuinely overflows its viewport) rather than PROGRESS —
    // PROGRESS's own outer height is bounded to exactly its content's size
    // (see `pane_tree`), so it can never leave "at bottom" no matter what
    // scroll key targets it, and is not a meaningful fixture for a
    // follow-release assertion.
    #[test]
    fn focus_keys_apply_before_the_same_frame_and_route_following_scroll() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let tree_lines = sample_journey_rows(20);
        let progress_sample_lines = sample_lines(3);
        let mut scrolls = PaneScrolls::new();
        let mut progress_follow = true;
        let mut journey_follow = true;
        let mut history_follow = true;
        let mut current_follow = true;
        let mut focus = FocusRing::new(vec![CURRENT_PANE]);
        // CURRENT -> (Tab) -> PROGRESS -> (Tab) -> JOURNEY -> (Down) scrolls
        // JOURNEY, in the leaf order [PROGRESS, JOURNEY, HISTORY, CURRENT].
        let mut keys = vec![
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        ];
        let mut terminal = Terminal::new(TestBackend::new(120, 12)).expect("test terminal");
        terminal
            .draw(|frame| {
                render_live_panes(
                    frame,
                    LiveFrame {
                        title_line: &tui::Line::blank(),
                        progress_lines: &progress_sample_lines,
                        journey_lines: &tree_lines,
                        active_row: Some(0),
                        history_rows: &[],
                        current_rows: &[],
                        post_run_lines: None,
                        scrolls: &mut scrolls,
                        progress_follow: &mut progress_follow,
                        journey_follow: &mut journey_follow,
                        history_follow: &mut history_follow,
                        current_follow: &mut current_follow,
                        focus: &mut focus,
                        pending_keys: &mut keys,
                        modal: None,
                        ask: None,
                    },
                );
            })
            .expect("draw");
        assert_eq!(focus.current(), Some(JOURNEY_PANE));
        assert!(!journey_follow);
        assert_eq!(scrolls.get(JOURNEY_PANE).window(10).start, 1);

        let mut keys = vec![KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE)];
        terminal
            .draw(|frame| {
                render_live_panes(
                    frame,
                    LiveFrame {
                        title_line: &tui::Line::blank(),
                        progress_lines: &progress_sample_lines,
                        journey_lines: &tree_lines,
                        active_row: Some(0),
                        history_rows: &[],
                        current_rows: &[],
                        post_run_lines: None,
                        scrolls: &mut scrolls,
                        progress_follow: &mut progress_follow,
                        journey_follow: &mut journey_follow,
                        history_follow: &mut history_follow,
                        current_follow: &mut current_follow,
                        focus: &mut focus,
                        pending_keys: &mut keys,
                        modal: None,
                        ask: None,
                    },
                );
            })
            .expect("draw");
        assert_eq!(focus.current(), Some(PROGRESS_PANE));
    }

    #[test]
    fn repeated_row_scrolls_fold_to_the_rendered_pane_budget_and_drain() {
        use crossterm::event::KeyModifiers;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let journey = sample_journey_rows(100);
        let progress = sample_lines(3);
        let mut scrolls = PaneScrolls::new();
        let mut progress_follow = true;
        let mut journey_follow = true;
        let mut history_follow = true;
        let mut current_follow = true;
        let mut focus = FocusRing::new(vec![CURRENT_PANE]);
        let mut keys = vec![
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        ];
        keys.extend(std::iter::repeat_n(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            100,
        ));
        let mut terminal = Terminal::new(TestBackend::new(120, 12)).expect("test terminal");

        terminal
            .draw(|frame| {
                render_live_panes(
                    frame,
                    LiveFrame {
                        title_line: &tui::Line::blank(),
                        progress_lines: &progress,
                        journey_lines: &journey,
                        active_row: Some(0),
                        history_rows: &[],
                        current_rows: &[],
                        post_run_lines: None,
                        scrolls: &mut scrolls,
                        progress_follow: &mut progress_follow,
                        journey_follow: &mut journey_follow,
                        history_follow: &mut history_follow,
                        current_follow: &mut current_follow,
                        focus: &mut focus,
                        pending_keys: &mut keys,
                        modal: None,
                        ask: None,
                    },
                );
            })
            .expect("draw");

        let area = Rect::new(0, 0, 120, 11);
        let title_line = title_row_line(None, "trait", None);
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&[]),
            current: Some(&[]),
            post_run: None,
            title: PaneTitleRow::Visible(&title_line),
        };
        let body = pane_body_area(area, &data.title);
        let layout = pane_tree(&LIVE_PANE_IDS, body, &data).resolve(body);
        let budget = tui_panes::pane_inner(layout.rect(JOURNEY_PANE).expect("journey pane")).height;

        assert!(
            keys.is_empty(),
            "all repeat events must drain in this frame"
        );
        assert_eq!(
            scrolls.get(JOURNEY_PANE).window(budget as usize).start,
            budget as usize
        );
        assert!(
            !journey_follow,
            "a folded movement off the tail releases follow"
        );
    }

    #[test]
    fn row_repeat_folding_stops_at_direction_and_tab_boundaries() {
        use crossterm::event::KeyModifiers;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let journey = sample_journey_rows(100);
        let history = sample_event_rows(100);
        let progress = sample_lines(3);
        let mut scrolls = PaneScrolls::new();
        let mut progress_follow = true;
        let mut journey_follow = true;
        let mut history_follow = true;
        let mut current_follow = true;
        let mut focus = FocusRing::new(vec![CURRENT_PANE]);
        let mut keys = vec![
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        ];
        keys.extend(std::iter::repeat_n(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            100,
        ));
        let mut terminal = Terminal::new(TestBackend::new(120, 22)).expect("test terminal");
        let area = Rect::new(0, 0, 120, 22);
        let regions = live_frame_regions(area);
        // The draw below renders a blank title line, so the body area this
        // test measures against has to have that row taken out of it. The
        // variant that consumes a row is `Visible`; `Reserved(None)` was the
        // spelling before `PaneTitleRow` collapsed to `None`/`Visible`.
        let title_line = tui::Line::blank();
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&history),
            current: Some(&[]),
            post_run: None,
            title: PaneTitleRow::Visible(&title_line),
        };
        let body = pane_body_area(regions[0], &data.title);
        let history_rect = pane_tree(&LIVE_PANE_IDS, body, &data)
            .resolve(body)
            .rect(HISTORY_PANE)
            .expect("history");

        terminal
            .draw(|frame| {
                render_live_panes(
                    frame,
                    LiveFrame {
                        title_line: &tui::Line::blank(),
                        progress_lines: &progress,
                        journey_lines: &journey,
                        active_row: Some(0),
                        history_rows: &history,
                        current_rows: &[],
                        post_run_lines: None,
                        scrolls: &mut scrolls,
                        progress_follow: &mut progress_follow,
                        journey_follow: &mut journey_follow,
                        history_follow: &mut history_follow,
                        current_follow: &mut current_follow,
                        focus: &mut focus,
                        pending_keys: &mut keys,
                        modal: None,
                        ask: None,
                    },
                );
            })
            .expect("draw");

        assert!(keys.is_empty());
        assert_eq!(focus.current(), Some(HISTORY_PANE));
        assert_eq!(scrolls.get(JOURNEY_PANE).window(1).start, 1);
        assert!(scrolls.get(HISTORY_PANE).window(1).start > 1);
        assert!(!journey_follow);
        assert!(!history_follow);
        assert_eq!(
            pane_tree(&LIVE_PANE_IDS, body, &data)
                .resolve(body)
                .rect(HISTORY_PANE),
            Some(history_rect),
            "scrolling history must not change its allocated rectangle"
        );
    }

    #[test]
    fn page_and_jump_keys_are_not_folded_as_row_repeats() {
        for key in [
            KeyEvent::new(KeyCode::PageUp, crossterm::event::KeyModifiers::NONE),
            KeyEvent::new(KeyCode::PageDown, crossterm::event::KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Home, crossterm::event::KeyModifiers::NONE),
            KeyEvent::new(KeyCode::End, crossterm::event::KeyModifiers::NONE),
        ] {
            assert_eq!(repeat_row_scroll_key(&key), None);
            assert!(tui_kit::scroll_key(&key).is_some());
        }
        assert_eq!(
            capped_repeat_delta(tui_kit::ScrollDelta::Down(10), None, 3),
            tui_kit::ScrollDelta::Down(10),
        );
    }

    #[test]
    fn post_run_exclusively_paints_and_jumps_its_rendered_rows() {
        use crossterm::event::KeyModifiers;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let progress = sample_lines(3);
        let journey = sample_journey_rows(3);
        let current = vec![EventRow {
            at: Some(Duration::ZERO),
            tail: "suppressed current activity".to_string(),
            tone: tui::Tone::Default,
        }];
        let post_run = (0..30)
            .map(|index| {
                let mut line = tui::Line::blank();
                line.push(format!("post-row-{index:02}"), tui::Tone::Default);
                line
            })
            .collect::<Vec<_>>();
        let mut scrolls = PaneScrolls::new();
        let mut progress_follow = false;
        let mut journey_follow = false;
        let mut history_follow = false;
        let mut current_follow = false;
        let mut focus = FocusRing::new(vec![CURRENT_PANE]);
        let mut terminal = Terminal::new(TestBackend::new(120, 12)).expect("test terminal");

        let mut draw = |keys: &mut Vec<KeyEvent>| {
            terminal
                .draw(|frame| {
                    render_live_panes(
                        frame,
                        LiveFrame {
                            title_line: &tui::Line::blank(),
                            progress_lines: &progress,
                            journey_lines: &journey,
                            active_row: None,
                            history_rows: &[],
                            current_rows: &current,
                            post_run_lines: Some(&post_run),
                            scrolls: &mut scrolls,
                            progress_follow: &mut progress_follow,
                            journey_follow: &mut journey_follow,
                            history_follow: &mut history_follow,
                            current_follow: &mut current_follow,
                            focus: &mut focus,
                            pending_keys: keys,
                            modal: None,
                            ask: None,
                        },
                    );
                })
                .expect("draw");
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        };

        let mut keys = vec![KeyEvent::new(KeyCode::Home, KeyModifiers::NONE)];
        let rendered = draw(&mut keys);
        assert!(rendered.contains("post-run"));
        assert!(rendered.contains("post-row-00"));
        assert!(!rendered.contains("suppressed current"));

        let mut keys = vec![KeyEvent::new(KeyCode::End, KeyModifiers::NONE)];
        let rendered = draw(&mut keys);
        assert!(rendered.contains("post-row-29"));
        let area = Rect::new(0, 0, 120, 11);
        let title = tui::Line::blank();
        let data = PaneData {
            progress: Some(&progress),
            journey: Some(&journey),
            history: Some(&[]),
            current: Some(&current),
            post_run: Some(&post_run),
            title: PaneTitleRow::Visible(&title),
        };
        let body = pane_body_area(area, &data.title);
        let rows = tui_panes::pane_inner(
            pane_tree(&LIVE_PANE_IDS, body, &data)
                .resolve(body)
                .rect(CURRENT_PANE)
                .expect("post-run pane"),
        )
        .height as usize;
        assert_eq!(scrolls.get(CURRENT_PANE).window(rows).end, post_run.len());
    }

    #[test]
    fn tiny_narrow_frame_keeps_current_activity_drawable() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let overlay = EventRow {
            at: Some(Duration::from_secs(0)),
            tail: "unique current activity".to_string(),
            tone: tui::Tone::Default,
        };
        let mut scrolls = PaneScrolls::new();
        let mut progress_follow = true;
        let mut journey_follow = true;
        let mut history_follow = true;
        let mut current_follow = true;
        let mut focus = FocusRing::new(vec![CURRENT_PANE]);
        let mut keys = Vec::new();
        // P552: narrow now stacks all four panes (never omitting any),
        // so this terminal must be tall enough for all four Min-bounded
        // constraints (3+3+3+`CURRENT_MIN_OUTER_ROWS`) plus the title/
        // footer rows — a 6-row terminal (the pre-P552 two-pane fixture)
        // could no longer fit even the four panes' own minimums.
        let mut terminal = Terminal::new(TestBackend::new(80, 22)).expect("test terminal");
        terminal
            .draw(|frame| {
                render_live_panes(
                    frame,
                    LiveFrame {
                        title_line: &tui::Line::blank(),
                        progress_lines: &[],
                        journey_lines: &[],
                        active_row: None,
                        history_rows: &[],
                        current_rows: std::slice::from_ref(&overlay),
                        post_run_lines: None,
                        scrolls: &mut scrolls,
                        progress_follow: &mut progress_follow,
                        journey_follow: &mut journey_follow,
                        history_follow: &mut history_follow,
                        current_follow: &mut current_follow,
                        focus: &mut focus,
                        pending_keys: &mut keys,
                        modal: None,
                        ask: None,
                    },
                )
            })
            .expect("draw");
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("unique current"));
    }

    /// P552: pending titles use a stable visible row; resolved titles render
    /// bold with the trait name and start clock following them.
    #[test]
    fn title_row_is_pending_before_success_and_bold_after() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::style::Modifier;

        let mut scrolls = PaneScrolls::new();
        let mut progress_follow = true;
        let mut journey_follow = true;
        let mut history_follow = true;
        let mut current_follow = true;
        let mut focus = FocusRing::new(vec![PROGRESS_PANE]);
        let mut keys = Vec::new();
        let pending_title_line = title_row_line(None, "implement-phase", Some(3_723));
        let mut terminal = Terminal::new(TestBackend::new(120, 12)).expect("test terminal");
        terminal
            .draw(|frame| {
                render_live_panes(
                    frame,
                    LiveFrame {
                        title_line: &pending_title_line,
                        progress_lines: &[],
                        journey_lines: &[],
                        active_row: None,
                        history_rows: &[],
                        current_rows: &[],
                        post_run_lines: None,
                        scrolls: &mut scrolls,
                        progress_follow: &mut progress_follow,
                        journey_follow: &mut journey_follow,
                        history_follow: &mut history_follow,
                        current_follow: &mut current_follow,
                        focus: &mut focus,
                        pending_keys: &mut keys,
                        modal: None,
                        ask: None,
                    },
                );
            })
            .expect("draw");
        let pending_row: String = (0..terminal.backend().buffer().area.width)
            .map(|x| terminal.backend().buffer().cell((x, 0)).unwrap().symbol())
            .collect();
        assert_eq!(
            pending_row.trim_end(),
            "(Generating session title…) · implement-phase · Started at 01:02:03"
        );

        let title_state = ctx_traits_core::procedure::session::SessionTitleState::Resolved {
            attempts: 1,
            title: "Refactor the merge story".to_string(),
        };
        let title_line = title_row_line(Some(&title_state), "implement-phase", Some(3_723));
        terminal
            .draw(|frame| {
                render_live_panes(
                    frame,
                    LiveFrame {
                        title_line: &title_line,
                        progress_lines: &[],
                        journey_lines: &[],
                        active_row: None,
                        history_rows: &[],
                        current_rows: &[],
                        post_run_lines: None,
                        scrolls: &mut scrolls,
                        progress_follow: &mut progress_follow,
                        journey_follow: &mut journey_follow,
                        history_follow: &mut history_follow,
                        current_follow: &mut current_follow,
                        focus: &mut focus,
                        pending_keys: &mut keys,
                        modal: None,
                        ask: None,
                    },
                );
            })
            .expect("draw");
        let rendered_row: String = (0..terminal.backend().buffer().area.width)
            .map(|x| terminal.backend().buffer().cell((x, 0)).unwrap().symbol())
            .collect();
        assert!(rendered_row.contains("Refactor the merge story"));
        assert!(rendered_row.contains("implement-phase"));
        assert!(rendered_row.contains("Started at 01:02:03"));
        assert!(
            terminal
                .backend()
                .buffer()
                .cell((0, 0))
                .expect("title cell")
                .style()
                .add_modifier
                .contains(Modifier::BOLD)
        );

        let title_state = ctx_traits_core::procedure::session::SessionTitleState::Terminal {
            attempts: 3,
            reason: "attempt-limit-exhausted".to_string(),
        };
        let title_line = title_row_line(Some(&title_state), "implement-phase", Some(3_723));
        terminal
            .draw(|frame| {
                render_live_panes(
                    frame,
                    LiveFrame {
                        title_line: &title_line,
                        progress_lines: &[],
                        journey_lines: &[],
                        active_row: None,
                        history_rows: &[],
                        current_rows: &[],
                        post_run_lines: None,
                        scrolls: &mut scrolls,
                        progress_follow: &mut progress_follow,
                        journey_follow: &mut journey_follow,
                        history_follow: &mut history_follow,
                        current_follow: &mut current_follow,
                        focus: &mut focus,
                        pending_keys: &mut keys,
                        modal: None,
                        ask: None,
                    },
                );
            })
            .expect("draw");
        let terminal_row: String = (0..terminal.backend().buffer().area.width)
            .map(|x| terminal.backend().buffer().cell((x, 0)).unwrap().symbol())
            .collect();
        assert_eq!(
            terminal_row.trim_end(),
            "implement-phase · Started at 01:02:03"
        );
    }

    #[test]
    fn epoch_clock_utc_wraps_seconds_of_day() {
        assert_eq!(epoch_clock_utc(3_723), "01:02:03");
        assert_eq!(epoch_clock_utc(86_400), "00:00:00");
    }

    // P470 blocker `down-key-forces-follow-jump`: stepping up releases
    // follow, one step back down advances by exactly one row (not a jump to
    // the tail), and only a further down at the bottom edge re-engages
    // follow.
    #[test]
    fn apply_scroll_and_derive_follow_only_reengages_at_the_tail() {
        let mut scroll = tui_kit::ViewportScroll::new();
        scroll.set_len(30);
        let rows = 10;
        let mut follow = true;
        apply_scroll_and_derive_follow(
            &mut scroll,
            &mut follow,
            tui_kit::ScrollDelta::Down(100),
            rows,
            30,
            FollowTarget::Tail,
        );
        assert!(follow);
        let tail_window = scroll.window(rows);

        apply_scroll_and_derive_follow(
            &mut scroll,
            &mut follow,
            tui_kit::ScrollDelta::Up(3),
            rows,
            30,
            FollowTarget::Tail,
        );
        assert!(!follow, "scrolling up must release follow");
        let scrolled_up_window = scroll.window(rows);
        assert_ne!(scrolled_up_window, tail_window);

        apply_scroll_and_derive_follow(
            &mut scroll,
            &mut follow,
            tui_kit::ScrollDelta::Down(1),
            rows,
            30,
            FollowTarget::Tail,
        );
        assert!(
            !follow,
            "a single row down from mid-scroll must not re-engage follow"
        );
        let one_down_window = scroll.window(rows);
        assert_ne!(
            one_down_window, tail_window,
            "one row down must advance the window by one row, not jump to the tail"
        );
        assert_eq!(one_down_window.start, scrolled_up_window.start + 1);

        apply_scroll_and_derive_follow(
            &mut scroll,
            &mut follow,
            tui_kit::ScrollDelta::Down(2),
            rows,
            30,
            FollowTarget::Tail,
        );
        assert!(follow, "reaching the tail edge must re-engage follow");
        assert_eq!(scroll.window(rows), tail_window);
    }

    #[test]
    fn zero_height_follow_tail_reengages_at_end_only() {
        let mut scroll = tui_kit::ViewportScroll::new();
        scroll.set_len(30);
        let mut follow = false;

        apply_scroll_and_derive_follow(
            &mut scroll,
            &mut follow,
            tui_kit::ScrollDelta::End,
            0,
            30,
            FollowTarget::Tail,
        );
        assert!(follow, "end must reengage tail following at zero height");

        apply_scroll_and_derive_follow(
            &mut scroll,
            &mut follow,
            tui_kit::ScrollDelta::Start,
            0,
            30,
            FollowTarget::Tail,
        );
        assert!(!follow, "a non-bottom position must release tail following");
    }

    #[test]
    fn zero_height_follow_active_row_reengages_at_end_only() {
        let mut scroll = tui_kit::ViewportScroll::new();
        scroll.set_len(30);
        let mut follow = false;
        let target = FollowTarget::ActiveRow(Some(12));

        apply_scroll_and_derive_follow(
            &mut scroll,
            &mut follow,
            tui_kit::ScrollDelta::End,
            0,
            30,
            target,
        );
        assert!(
            follow,
            "end must reengage active-row following at zero height"
        );

        apply_scroll_and_derive_follow(
            &mut scroll,
            &mut follow,
            tui_kit::ScrollDelta::Start,
            0,
            30,
            target,
        );
        assert!(
            !follow,
            "a non-bottom position must release active-row following at zero height"
        );
    }

    #[test]
    fn tail_follow_advances_without_reset_and_stays_pinned_after_resize() {
        let mut scroll = tui_kit::ViewportScroll::new();
        follow_target(&mut scroll, true, FollowTarget::Tail, 30, 10);
        assert_eq!(scroll.window(10), 20..30);

        follow_target(&mut scroll, true, FollowTarget::Tail, 35, 10);
        assert_eq!(scroll.window(10), 25..35);
        assert_ne!(scroll.window(10).start, 0);

        follow_target(&mut scroll, true, FollowTarget::Tail, 35, 15);
        assert_eq!(scroll.window(15), 20..35);
    }

    #[test]
    fn non_following_tail_keeps_its_window_when_stream_grows() {
        let mut scroll = tui_kit::ViewportScroll::new();
        follow_target(&mut scroll, true, FollowTarget::Tail, 40, 10);
        let mut follow = false;
        apply_scroll_and_derive_follow(
            &mut scroll,
            &mut follow,
            tui_kit::ScrollDelta::Up(12),
            10,
            40,
            FollowTarget::Tail,
        );
        assert!(!follow);
        let stationary = scroll.window(10);

        follow_target(&mut scroll, false, FollowTarget::Tail, 55, 10);
        assert_eq!(scroll.window(10), stationary);
    }

    #[test]
    fn journey_tail_does_not_reengage_active_row_follow() {
        let mut scroll = tui_kit::ViewportScroll::new();
        let target = FollowTarget::ActiveRow(Some(12));
        follow_target(&mut scroll, true, target, 40, 10);
        assert_eq!(scroll.window(10), 3..13, "active row stays at the bottom");

        let mut follow = true;
        apply_scroll_and_derive_follow(
            &mut scroll,
            &mut follow,
            tui_kit::ScrollDelta::End,
            10,
            40,
            target,
        );
        assert!(!follow, "the final window is not the active-row alignment");
        assert_eq!(scroll.window(10), 30..40);

        // A render while released must leave the user at the pending tail.
        follow_target(&mut scroll, follow, target, 40, 10);
        assert_eq!(scroll.window(10), 30..40);
    }

    #[test]
    fn journey_reengages_only_at_active_row_alignment() {
        let mut scroll = tui_kit::ViewportScroll::new();
        let target = FollowTarget::ActiveRow(Some(12));
        follow_target(&mut scroll, true, target, 40, 10);
        let mut follow = true;

        apply_scroll_and_derive_follow(
            &mut scroll,
            &mut follow,
            tui_kit::ScrollDelta::Down(2),
            10,
            40,
            target,
        );
        assert!(
            !follow,
            "a nearby active row must not be enough to reengage"
        );
        assert_eq!(scroll.window(10), 5..15);

        apply_scroll_and_derive_follow(
            &mut scroll,
            &mut follow,
            tui_kit::ScrollDelta::Up(2),
            10,
            40,
            target,
        );
        assert!(follow, "returning to the exact alignment resumes following");
        assert_eq!(scroll.window(10), 3..13);
    }

    #[test]
    fn journey_follow_target_falls_back_to_tail_and_handles_tail_alignment() {
        assert_eq!(FollowTarget::ActiveRow(None).viewport_start(40, 10), 30);
        assert_eq!(FollowTarget::ActiveRow(Some(39)).viewport_start(40, 10), 30);
    }

    #[test]
    fn progress_follow_aligns_each_active_rendered_row_without_overshoot() {
        let mut scroll = tui_kit::ViewportScroll::new();
        follow_target(&mut scroll, true, FollowTarget::ActiveRow(Some(12)), 40, 10);
        assert_eq!(scroll.window(10), 3..13);

        follow_target(&mut scroll, true, FollowTarget::ActiveRow(Some(21)), 40, 10);
        assert_eq!(scroll.window(10), 12..22);

        follow_target(&mut scroll, true, FollowTarget::ActiveRow(Some(30)), 40, 10);
        assert_eq!(scroll.window(10), 21..31);
    }

    #[test]
    fn progress_follow_keeps_the_active_row_at_bottom_after_resize() {
        let mut scroll = tui_kit::ViewportScroll::new();
        follow_target(&mut scroll, true, FollowTarget::ActiveRow(Some(58)), 60, 10);
        assert_eq!(scroll.window(10), 49..59);

        follow_target(&mut scroll, true, FollowTarget::ActiveRow(Some(58)), 60, 20);
        assert_eq!(scroll.window(20), 39..59);
    }

    /// The regression this exists for: a window focus change alters the whole
    /// frame's styling without changing its content, so it has to count as a
    /// change. It used not to, and the dim then waited for the once-a-second
    /// clock refresh — or forever on an idle pane, which schedules no timer
    /// paint at all.
    #[test]
    fn a_focus_change_paints_immediately_even_when_nothing_is_running() {
        assert!(tick_must_paint(true, false, Duration::ZERO));
        assert!(tick_must_paint(true, true, Duration::ZERO));
    }

    #[test]
    fn an_unchanged_idle_tick_never_paints() {
        assert!(!tick_must_paint(false, false, Duration::ZERO));
        // Idle schedules no timer paint, so even a long wait must not force one.
        assert!(!tick_must_paint(false, false, Duration::from_secs(60)));
    }

    #[test]
    fn an_unchanged_running_tick_paints_only_on_the_one_second_clock() {
        assert!(!tick_must_paint(false, true, Duration::from_millis(999)));
        assert!(tick_must_paint(false, true, Duration::from_secs(1)));
    }

    #[test]
    fn panel_cadence_skips_idle_ticks_and_admits_each_input_generation() {
        let input = Arc::new(AtomicU64::new(0));
        let handled = Arc::new(AtomicU64::new(0));
        let cadence = PanelCadence::new(Arc::clone(&input), Arc::clone(&handled));
        cadence.painted();
        assert!(
            !cadence.should_run(),
            "idle observer ticks must avoid the panel lock"
        );
        input.fetch_add(1, Ordering::Release);
        assert!(
            cadence.should_run(),
            "a queued key or resize must bypass the timer throttle"
        );
        cadence.observe(TickOutcome {
            consumed_generation: input.load(Ordering::Acquire),
            ..TickOutcome::default()
        });
        assert!(!cadence.should_run());
        input.fetch_add(1, Ordering::Release);
        assert!(
            cadence.should_run(),
            "successive held-scroll events remain distinct"
        );
    }

    #[test]
    fn no_op_input_acknowledgement_preserves_or_disables_the_right_timer() {
        let input = Arc::new(AtomicU64::new(1));
        let handled = Arc::new(AtomicU64::new(0));
        let cadence = PanelCadence::new(Arc::clone(&input), Arc::clone(&handled));
        cadence.painted();
        let heartbeat = cadence.next_timer_paint_ms.load(Ordering::Acquire);
        cadence.observe(TickOutcome {
            consumed_generation: 1,
            ..TickOutcome::default()
        });
        assert_eq!(handled.load(Ordering::Acquire), 1);
        assert_eq!(
            cadence.next_timer_paint_ms.load(Ordering::Acquire),
            heartbeat
        );
        cadence.observe(TickOutcome {
            disable_timer: true,
            ..TickOutcome::default()
        });
        assert_eq!(
            cadence.next_timer_paint_ms.load(Ordering::Acquire),
            u64::MAX
        );
    }

    #[test]
    fn deferred_resize_is_retried_without_rearming_the_repaint_heartbeat() {
        let input = Arc::new(AtomicU64::new(0));
        let handled = Arc::new(AtomicU64::new(0));
        let cadence = PanelCadence::new(input, handled);
        cadence.painted();
        let heartbeat = cadence.next_timer_paint_ms.load(Ordering::Acquire);
        cadence.observe(TickOutcome {
            resize_retry: true,
            ..TickOutcome::default()
        });
        assert!(cadence.next_resize_retry_ms.load(Ordering::Acquire) < u64::MAX);
        assert_eq!(
            cadence.next_timer_paint_ms.load(Ordering::Acquire),
            heartbeat
        );
    }

    #[test]
    fn shared_paint_and_detach_transitions_close_timer_admission() {
        let input = Arc::new(AtomicU64::new(0));
        let handled = Arc::new(AtomicU64::new(0));
        let cadence = PanelCadence::new(input, handled);
        assert!(
            cadence.should_run(),
            "an uninitialized pane needs its first paint"
        );
        cadence.painted();
        assert!(
            !cadence.should_run(),
            "a token-driven paint must close the next observer admission"
        );
        cadence.inactive();
        assert!(
            !cadence.should_run(),
            "detachment must disable timer admission for the remaining frame"
        );
    }

    #[test]
    fn ask_pane_routes_open_edit_answer_and_collapse_visibility() {
        let mut ask = AskPane::default();
        assert!(ask_lines(&ask).is_empty());
        assert_eq!(
            apply_ask_presentation_key(&mut ask, &KeyEvent::from(KeyCode::Char('?'))),
            Some(true)
        );
        assert!(ask.open);
        assert!(matches!(
            ask.input
                .handle_key(false, &KeyEvent::from(KeyCode::Char('é'))),
            tui_kit::ModalOutcome::Pending
        ));
        assert_eq!(ask.input.cursor(), 1);
        assert!(matches!(
            ask.input.handle_key(false, &KeyEvent::from(KeyCode::Left)),
            tui_kit::ModalOutcome::Pending
        ));
        assert_eq!(ask.input.cursor(), 0);
        ask.generation = 1;
        ask.in_flight = true;
        ask.exchanges.push(GuideExchange {
            question: "é".to_string(),
            generation: 1,
            answer: None,
        });
        assert!(apply_ask_result(&mut ask, 1, Ok("answer".to_string())));
        assert_eq!(ask_lines(&ask), ["You: é", "Guide: answer"]);
        assert_eq!(
            apply_ask_presentation_key(&mut ask, &KeyEvent::from(KeyCode::Esc)),
            Some(true)
        );
        assert!(!ask.open);
        assert_eq!(ask_lines(&ask), ["You: é", "Guide: answer"]);
    }

    #[test]
    fn text_input_inline_ask_unicode_editing_survives_close_and_reopen() {
        let mut ask = AskPane::default();
        apply_ask_presentation_key(&mut ask, &KeyEvent::from(KeyCode::Char('?')));
        for key in ['文', 'é'] {
            assert!(matches!(
                ask.input
                    .handle_key(false, &KeyEvent::from(KeyCode::Char(key))),
                tui_kit::ModalOutcome::Pending
            ));
        }
        assert!(matches!(
            ask.input.handle_key(false, &KeyEvent::from(KeyCode::Left)),
            tui_kit::ModalOutcome::Pending
        ));
        assert!(matches!(
            ask.input
                .handle_key(false, &KeyEvent::from(KeyCode::Backspace)),
            tui_kit::ModalOutcome::Pending
        ));
        assert_eq!(ask.input.text(), "é");
        apply_ask_presentation_key(&mut ask, &KeyEvent::from(KeyCode::Esc));
        apply_ask_presentation_key(&mut ask, &KeyEvent::from(KeyCode::Char('?')));
        assert_eq!(ask.input.text(), "é");
        assert_eq!(ask.input.cursor(), 0);
    }

    #[test]
    fn ask_pane_layout_regions_are_disjoint_at_wide_narrow_and_short_sizes() {
        for area in [
            Rect::new(0, 0, 160, 40),
            Rect::new(0, 0, 70, 20),
            Rect::new(0, 0, 70, 6),
        ] {
            let regions = live_frame_regions(area);
            assert_eq!(regions[1].width, area.width);
            assert!(regions[0].height >= 3, "body must retain usable rows");
            assert_eq!(regions[1].height, 1);
            assert!(regions[0].bottom() <= regions[1].y);
        }
    }

    #[test]
    fn ask_footer_renders_every_hint_at_narrow_width_boundary() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        for width in [NARROW_WIDTH_THRESHOLD - 1, NARROW_WIDTH_THRESHOLD] {
            let mut terminal = Terminal::new(TestBackend::new(width, 12)).expect("test terminal");
            let mut scrolls = PaneScrolls::new();
            let mut progress_follow = true;
            let mut journey_follow = true;
            let mut history_follow = true;
            let mut current_follow = true;
            let mut focus = FocusRing::new(vec![CURRENT_PANE]);
            let mut keys = Vec::new();
            let ask = AskPane::default();
            terminal
                .draw(|frame| {
                    render_live_panes(
                        frame,
                        LiveFrame {
                            title_line: &tui::Line::blank(),
                            progress_lines: &[],
                            journey_lines: &[],
                            active_row: None,
                            history_rows: &[],
                            current_rows: &[],
                            post_run_lines: None,
                            scrolls: &mut scrolls,
                            progress_follow: &mut progress_follow,
                            journey_follow: &mut journey_follow,
                            history_follow: &mut history_follow,
                            current_follow: &mut current_follow,
                            focus: &mut focus,
                            pending_keys: &mut keys,
                            modal: None,
                            ask: Some(&GuideChatHandle(Arc::new(Mutex::new(GuideChat {
                                ask,
                                dispatch: Arc::new(|_, _| Ok(String::new())),
                                tokens: Default::default(),
                                results: None,
                                wake: None,
                                context: String::new(),
                            })))),
                        },
                    );
                })
                .expect("draw");
            let rendered: String = (0..width)
                .map(|x| terminal.backend().buffer().cell((x, 11)).unwrap().symbol())
                .collect();
            for hint in [
                "[?] ask",
                "[up/down] scroll",
                "[pg] page",
                "[home/end] jump",
                "[tab] pane",
                "[d] dash",
                "[q] exit",
                "[ctrl-c] kill",
            ] {
                assert!(
                    rendered.contains(hint),
                    "width {width} omits {hint}: {rendered}"
                );
            }
            assert!(!rendered.contains("Ask the guide"));
        }
    }

    #[test]
    fn ask_pane_normalizes_and_bounds_multiline_answers() {
        let answer = format!(
            "first\n\nsecond\tthird {}",
            "x".repeat(MAX_GUIDE_ANSWER_CHARS)
        );
        let display = displayable_guide_answer(&answer);
        assert_eq!(
            display.split_whitespace().take(3).collect::<Vec<_>>(),
            ["first", "second", "third"]
        );
        assert!(!display.contains(['\n', '\r']));
        assert!(display.chars().count() <= MAX_GUIDE_ANSWER_CHARS + 3);
        assert!(display.ends_with("..."));
    }

    #[test]
    fn ask_pane_sanitizes_untrusted_answer_controls() {
        let answer = format!(
            "\x1b[31mfirst\x1b[0m\nsecond\u{0007}\u{202e}third {}",
            "x".repeat(MAX_GUIDE_ANSWER_CHARS)
        );
        let display = displayable_guide_answer(&answer);

        assert_eq!(
            display.split_whitespace().take(3).collect::<Vec<_>>(),
            ["first", "second", "third"]
        );
        assert!(!display.contains('\x1b'));
        assert!(!display.chars().any(|ch| ch.is_control()));
        assert!(!display.contains('\u{202e}'));
        assert!(display.chars().count() <= MAX_GUIDE_ANSWER_CHARS + 3);
        assert!(display.ends_with("..."));
    }

    #[test]
    fn ask_pane_worker_completion_wakes_the_cadence_immediately() {
        let input = Arc::new(AtomicU64::new(0));
        let handled = Arc::new(AtomicU64::new(0));
        let cadence = PanelCadence::new(Arc::clone(&input), handled);
        cadence.painted();
        assert!(!cadence.should_run());
        // This is the notification emitted after the detached worker queues
        // its result, before its weak-panel tick is attempted.
        input.fetch_add(1, Ordering::Release);
        assert!(cadence.should_run());
    }

    #[test]
    fn guide_call_lifecycle_retains_hidden_completion_and_rejects_unknown_generation() {
        let mut ask = AskPane {
            open: true,
            in_flight: true,
            generation: 7,
            exchanges: vec![GuideExchange {
                question: "question".to_string(),
                generation: 7,
                answer: None,
            }],
            ..AskPane::default()
        };
        apply_ask_presentation_key(&mut ask, &KeyEvent::from(KeyCode::Esc));
        assert!(ask.in_flight, "collapsing must not permit another call");
        assert!(apply_ask_result(&mut ask, 7, Ok("settled".to_string())));
        assert!(!ask.open);
        assert_eq!(ask.exchanges[0].answer.as_deref(), Some("settled"));
        assert!(!ask.in_flight);
        assert!(!apply_ask_result(&mut ask, 8, Ok("stale".to_string())));
    }

    #[test]
    fn stale_guide_result_does_not_clear_current_in_flight() {
        let mut ask = AskPane {
            in_flight: true,
            generation: 2,
            exchanges: vec![
                GuideExchange {
                    question: "first".to_string(),
                    generation: 1,
                    answer: Some("answered".to_string()),
                },
                GuideExchange {
                    question: "second".to_string(),
                    generation: 2,
                    answer: None,
                },
            ],
            ..AskPane::default()
        };
        assert!(!apply_ask_result(&mut ask, 1, Ok("stale".to_string())));
        assert_eq!(ask.exchanges[0].answer.as_deref(), Some("answered"));
        assert!(ask.exchanges[1].answer.is_none());
        assert!(ask.in_flight);
    }

    #[test]
    fn pending_guide_exchange_has_single_label() {
        let ask = AskPane {
            exchanges: vec![GuideExchange {
                question: "question".to_string(),
                generation: 1,
                answer: None,
            }],
            ..AskPane::default()
        };
        assert_eq!(ask_lines(&ask), ["You: question", "Guide: thinking..."]);
    }

    #[test]
    fn guide_call_lifecycle_blocks_reopen_submission_until_the_reserved_call_settles() {
        let mut ask = AskPane {
            open: true,
            in_flight: true,
            generation: 3,
            exchanges: vec![GuideExchange {
                question: "question".to_string(),
                generation: 3,
                answer: None,
            }],
            ..AskPane::default()
        };
        apply_ask_presentation_key(&mut ask, &KeyEvent::from(KeyCode::Esc));
        apply_ask_presentation_key(&mut ask, &KeyEvent::from(KeyCode::Char('?')));
        assert!(ask.open);
        assert!(ask.in_flight);
        // The presentation router consumes Enter while the reservation is
        // live, so reopening cannot launch another call.
        // A live router checks this guard before it can reserve another call.
        assert!(ask.in_flight);
        assert!(apply_ask_result(&mut ask, 3, Ok("settled".to_string())));
        assert!(!ask.in_flight);
    }

    #[test]
    fn guide_chat_scroll_uses_rendered_viewport_rows() {
        let chat = GuideChatHandle::test_handle();
        {
            let mut state = chat.lock();
            state.ask.open = true;
            state.ask.scroll.set_len(30);
            state.ask.body_rows = 3;
            state.ask.scroll.apply(tui_kit::ScrollDelta::End, 3);
            state.ask.follow = true;
        }
        chat.handle_key(&KeyEvent::from(KeyCode::Up), 3);
        {
            let state = chat.lock();
            assert_eq!(state.ask.scroll.window(3), 26..29);
            assert!(!state.ask.follow);
        }
        chat.handle_key(&KeyEvent::from(KeyCode::Down), 3);
        assert!(chat.lock().ask.follow);

        // A resize changes both the clamp and the tail position; the same one
        // row key must use the new rendered body height, not a fixed value.
        chat.lock().ask.scroll.apply(tui_kit::ScrollDelta::End, 7);
        chat.handle_key(&KeyEvent::from(KeyCode::Up), 7);
        let state = chat.lock();
        assert_eq!(state.ask.scroll.window(7), 22..29);
        assert!(!state.ask.follow);
    }

    #[test]
    fn guide_header_tokens_labels_live_and_reconstructed_usage() {
        let mut view = view_with(Vec::new());
        view.header.output_tokens = Some(1_000);
        view.header.narrator_tokens = Some(2_000);
        view.header.guide_tokens = Some(3_000);
        let mut lines = Vec::new();
        render_header(&mut lines, &view.header);
        let rendered = lines.iter().map(line_text).collect::<String>();
        assert!(rendered.contains("1.0k"));
        assert!(rendered.contains("narrator 2.0k"));
        assert!(rendered.contains("guide 3.0k"));
        view.header.completed = true;
        let mut completed = Vec::new();
        render_header(&mut completed, &view.header);
        assert!(
            completed
                .iter()
                .map(line_text)
                .collect::<String>()
                .contains("guide 3.0k")
        );
    }
}
