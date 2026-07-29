//! Live run-journey presentation for driven procedure sessions.
//!
//! This module is CLI presentation only. It maps an already-built dry plan plus
//! live session state into styled terminal lines; it never mutates the run ledger
//! or changes driver/report semantics.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
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

#[derive(Clone)]
pub(crate) struct RunPanel {
    state: Arc<Mutex<RunPanelState>>,
    cadence: Arc<PanelCadence>,
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
    tree_follow: bool,
    history_follow: bool,
    stream_follow: bool,
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
}

/// One row of the CURRENT step's verbatim message/thinking stream.
struct StreamRow {
    /// Elapsed since the run pane started, rendered as `HH:MM:SS`.
    at: Duration,
    kind: StreamRowKind,
    text: String,
}

enum StreamRowKind {
    /// A narrator summary line (narrated mode).
    Narration,
    /// A drained raw model-text delta (narrator-free/passthrough mode).
    ModelText,
}

const PROGRESS_PANE: PaneId = "progress";
const HISTORY_PANE: PaneId = "history";
const CURRENT_PANE: PaneId = "current";
const CURRENT_MIN_OUTER_ROWS: u16 = 8;

const CURRENT_STREAM_CAP: usize = 400;

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
    narration: Option<RunNarration>,
    outputs: Vec<RunOutput>,
    /// P549: merge stage rows, rendered directly after `steps` in the same
    /// pane the run's own journey draws in — empty for every run that never
    /// hands a [`RunPanel`] into a merge span. Carried on `RunPanelState`
    /// itself (not derived by [`run_view`]) since a merge event has nothing
    /// to do with the plan/session `run_view` otherwise projects from.
    merge_rows: Vec<MergeRowView>,
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
    run_id: String,
    session_id: String,
    trait_name: String,
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
    run_started: Instant,
}

impl RunPanel {
    pub(crate) fn new(
        trait_name: String,
        trait_ref: ctx_traits_core::Trait,
        plan: ctx_traits_core::procedure::run::Plan,
        session: ctx_traits_core::procedure::session::Session,
    ) -> std::io::Result<Self> {
        let repaint = RatatuiPane::new_inline()?;
        let input_generation = repaint.input_generation();
        let handled_generation = Arc::new(AtomicU64::new(0));
        let cadence = Arc::new(PanelCadence::new(
            Arc::clone(&input_generation),
            Arc::clone(&handled_generation),
        ));
        let active_key = active_key(&session);
        let now = Instant::now();
        let active_started = active_key.as_ref().map(|key| (key.clone(), now));
        let view = run_view(
            &trait_name,
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
                run_started: now,
            },
        );
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
            live: tui::LiveLine::default(),
            step_summaries: BTreeMap::new(),
            step_summary_at: BTreeMap::new(),
            current_stream: VecDeque::new(),
            view,
            scrolls: PaneScrolls::new(),
            tree_follow: true,
            history_follow: true,
            stream_follow: true,
            // The step list is what a watcher reads first, so it holds focus by
            // default; tab cycles to the activity panes.
            focus: FocusRing::new(vec![PROGRESS_PANE]),
            pending_keys: Vec::new(),
            modal: None,
            last_tree_lines: Vec::new(),
            merge_rows: Vec::new(),
        }));
        let panel = Self {
            state: Arc::clone(&state),
            cadence: Arc::clone(&cadence),
        };
        let weak_state = Arc::downgrade(&state);
        let wake_cadence = Arc::clone(&cadence);
        if let Ok(mut state) = state.lock() {
            state.repaint.install_input_wake(Arc::new(move || {
                tick_weak(&weak_state, &wake_cadence);
            }));
        }
        panel.render();
        panel.cadence.painted();
        Ok(panel)
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
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let tree_lines = state.last_tree_lines.clone();
        let _ = state.repaint.commit_inline_scrollback(&tree_lines);
        state.repaint.quit();
        state.cadence.inactive();
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
        let Ok(mut state) = self.state.lock() else {
            return None;
        };
        let active_key = active_key(session);
        let mut completed = None;
        if state.active_key != active_key {
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
                state.stream_follow = true;
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
        let display = state.live.current_display();
        let mut narration = narration_for(&state.session, display.clone());
        if display.finished
            && let (Some(current), Some(previous)) =
                (narration.as_mut(), state.view.narration.as_ref())
        {
            current.label.clone_from(&previous.label);
        }
        rebuild_view(&mut state, narration);
        render_locked(&mut state);
        mark_timer_painted(&mut state);
        completed
    }

    pub(crate) fn push_bytes(&self, chunk: &[u8]) {
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
        if !self.cadence.should_run() {
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let outcome = tick_locked(&mut state);
        self.cadence.observe(outcome);
    }

    pub(crate) fn add_output_tokens(&self, tokens: u64) {
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
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        fold_merge_event(&mut state.merge_rows, event, Instant::now());
        let narration = state.view.narration.clone();
        rebuild_view(&mut state, narration);
        render_locked(&mut state);
    }

    fn render(&self) {
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

fn tick_weak(state: &Weak<Mutex<RunPanelState>>, cadence: &PanelCadence) {
    if !cadence.should_run() {
        return;
    }
    let Some(state) = state.upgrade() else {
        return;
    };
    let Ok(mut state) = state.lock() else {
        return;
    };
    let outcome = tick_locked(&mut state);
    cadence.observe(outcome);
}

fn tick_locked(state: &mut RunPanelState) -> TickOutcome {
    let consumed_generation = state.input_generation.load(Ordering::Acquire);
    let resized = state.repaint.apply_resize();
    let changed = poll_and_apply_keys(state) || resized;
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
    let has_running_work = state.active_started.is_some()
        || state
            .merge_rows
            .iter()
            .any(|row| row.state == MergeRowState::Running);
    if !changed && (!has_running_work || state.last_timer_paint.elapsed() < Duration::from_secs(1))
    {
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

fn mark_timer_painted(state: &mut RunPanelState) {
    state.last_timer_paint = Instant::now();
    state.cadence.painted();
}

fn rebuild_view(state: &mut RunPanelState, narration: Option<RunNarration>) {
    state.view = run_view(
        &state.trait_name,
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
fn poll_and_apply_keys(state: &mut RunPanelState) -> bool {
    let keys = state.repaint.poll_detach();
    let mut changed = false;
    for key in keys {
        if let Some(modal) = state.modal.as_mut() {
            match modal.handle_key(&key) {
                tui_kit::ModalOutcome::Confirmed => {
                    state.modal = None;
                    state.repaint.quit();
                    eprintln!(
                        "live view closed; run continues — reattach with ctx traits dashboard"
                    );
                }
                tui_kit::ModalOutcome::Cancelled => {
                    state.modal = None;
                }
                tui_kit::ModalOutcome::Pending | tui_kit::ModalOutcome::Submitted(_) => {}
            }
            changed = true;
            continue;
        }
        if key.code == KeyCode::Char('q') {
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
        if tui_panes::tab_cycle_key(&key).is_some() || tui_kit::scroll_key(&key).is_some() {
            changed = true;
        }
        state.pending_keys.push(key);
    }
    changed
}

/// P470 blocker fix (`down-key-forces-follow-jump`): applies one scroll
/// delta and then derives `follow` from the RESULTING position, never from
/// the key's own direction — downward stepping through a scrolled region
/// only re-engages follow once the window actually reaches the tail.
fn apply_scroll_and_derive_follow(
    scroll: &mut tui_kit::ViewportScroll,
    follow: &mut bool,
    delta: tui_kit::ScrollDelta,
    budget: usize,
) {
    scroll.apply(delta, budget);
    *follow = scroll.is_at_bottom(budget);
}

fn push_stream_row(state: &mut RunPanelState, kind: StreamRowKind, text: String) {
    let at = state.run_started.elapsed();
    state.current_stream.push_back(StreamRow { at, kind, text });
    while state.current_stream.len() > CURRENT_STREAM_CAP {
        state.current_stream.pop_front();
    }
}

fn render_locked(state: &mut RunPanelState) {
    // Capture before draining: a wake that lands concurrently afterwards must
    // remain pending for a later tick rather than being acknowledged unseen.
    let input_generation = state.input_generation.load(Ordering::Acquire);
    state.repaint.apply_resize();
    poll_and_apply_keys(state);
    let (tree_lines, active_row) = render_tree_lines_with_active_row(&state.view);
    let history_lines = story_history_lines(&state.view);
    let stream_lines = story_stream_lines(state);
    let RunPanelState {
        repaint,
        scrolls,
        tree_follow,
        history_follow,
        stream_follow,
        focus,
        pending_keys,
        modal,
        ..
    } = state;
    let modal = modal.as_ref();
    let _ = repaint.draw(|frame| {
        render_live_panes(
            frame,
            LivePaneFrame {
                tree_lines: &tree_lines,
                active_row,
                history_lines: &history_lines,
                stream_lines: &stream_lines,
                scrolls,
                tree_follow,
                history_follow,
                stream_follow,
                focus,
                pending_keys,
                modal,
            },
        );
    });
    state.last_tree_lines = tree_lines;
    state
        .handled_generation
        .fetch_max(input_generation, Ordering::Release);
}

fn live_pane_tree(area: Rect, history_rows: usize) -> PaneTree {
    let progress = || PaneTree::Leaf {
        id: PROGRESS_PANE,
        title: "progress".to_string(),
    };
    let current = || PaneTree::Leaf {
        id: CURRENT_PANE,
        title: "current activity".to_string(),
    };
    if area.width < 109 {
        return PaneTree::Split {
            dir: Direction::Vertical,
            children: vec![
                (Constraint::Min(0), progress()),
                (Constraint::Min(CURRENT_MIN_OUTER_ROWS), current()),
            ],
        };
    }
    let story = if history_rows > 0 && area.height > CURRENT_MIN_OUTER_ROWS {
        let history_cap = area.height.saturating_sub(CURRENT_MIN_OUTER_ROWS) / 2;
        let history_height = u16::try_from(history_rows)
            .unwrap_or(u16::MAX)
            .saturating_add(2)
            .min(history_cap);
        PaneTree::Split {
            dir: Direction::Vertical,
            children: vec![
                (
                    Constraint::Max(history_height),
                    PaneTree::Leaf {
                        id: HISTORY_PANE,
                        title: "history".to_string(),
                    },
                ),
                (Constraint::Min(CURRENT_MIN_OUTER_ROWS), current()),
            ],
        }
    } else {
        current()
    };
    PaneTree::Split {
        dir: Direction::Horizontal,
        children: vec![
            (Constraint::Percentage(60), progress()),
            (Constraint::Percentage(40), story),
        ],
    }
}

struct LivePaneFrame<'a> {
    tree_lines: &'a [tui::Line],
    active_row: Option<usize>,
    history_lines: &'a [tui::Line],
    stream_lines: &'a [tui::Line],
    scrolls: &'a mut PaneScrolls,
    tree_follow: &'a mut bool,
    history_follow: &'a mut bool,
    stream_follow: &'a mut bool,
    focus: &'a mut FocusRing,
    pending_keys: &'a mut Vec<KeyEvent>,
    modal: Option<&'a tui_kit::Modal>,
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

fn render_live_panes(frame: &mut ratatui::Frame<'_>, state: LivePaneFrame<'_>) {
    let LivePaneFrame {
        tree_lines,
        active_row,
        history_lines,
        stream_lines,
        scrolls,
        tree_follow,
        history_follow,
        stream_follow,
        focus,
        pending_keys,
        modal,
    } = state;
    let full_area = frame.area();
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(full_area);
    frame.render_widget(
        tui_kit::keymap_footer(
            "[q] exit · [ctrl-c] kill · [up/down] scroll · [pgup/pgdn] page · [home/end] jump · [tab] cycle pane",
            None,
        ),
        regions[1],
    );
    let history_source = history_lines
        .iter()
        .map(tui_ratatui::render_line)
        .collect::<Vec<_>>();
    let history_width = regions[0]
        .width
        .saturating_mul(40)
        .saturating_div(100)
        .saturating_sub(2);
    let history_row_count = tui_panes::wrapped_lines(&history_source, history_width).len();
    let tree = live_pane_tree(regions[0], history_row_count);
    let layout = tree.resolve(regions[0]);
    focus.reconcile(drawable_pane_ids(&tree, &layout), CURRENT_PANE);
    let progress_outer = layout.rect(PROGRESS_PANE).expect("progress pane");
    let current_outer = layout.rect(CURRENT_PANE).expect("current pane");
    let history_outer = layout.rect(HISTORY_PANE);
    let progress_inner = tui_panes::pane_inner(progress_outer);
    let current_inner = tui_panes::pane_inner(current_outer);
    let history_inner = history_outer.map(tui_panes::pane_inner);
    let progress = tree_lines
        .iter()
        .map(tui_ratatui::render_line)
        .collect::<Vec<_>>();
    let history =
        tui_panes::wrapped_lines(&history_source, history_inner.map_or(0, |rect| rect.width));
    let stream = tui_panes::wrapped_lines(
        &stream_lines
            .iter()
            .map(tui_ratatui::render_line)
            .collect::<Vec<_>>(),
        current_inner.width,
    );
    for key in pending_keys.drain(..) {
        if let Some(step) = tui_panes::tab_cycle_key(&key) {
            match step {
                TabStep::Next => focus.next(),
                TabStep::Prev => focus.prev(),
            }
            continue;
        }
        let Some(delta) = tui_kit::scroll_key(&key) else {
            continue;
        };
        let Some(id) = focus.current() else {
            continue;
        };
        match id {
            PROGRESS_PANE => {
                let scroll = scrolls.get_mut(id);
                scroll.set_len(progress.len());
                apply_scroll_and_derive_follow(
                    scroll,
                    tree_follow,
                    delta,
                    progress_inner.height as usize,
                );
            }
            HISTORY_PANE => {
                let scroll = scrolls.get_mut(id);
                scroll.set_len(history.len());
                apply_scroll_and_derive_follow(
                    scroll,
                    history_follow,
                    delta,
                    history_inner.map_or(0, |rect| rect.height as usize),
                );
            }
            CURRENT_PANE => {
                let scroll = scrolls.get_mut(id);
                scroll.set_len(stream.len());
                apply_scroll_and_derive_follow(
                    scroll,
                    stream_follow,
                    delta,
                    current_inner.height as usize,
                );
            }
            _ => continue,
        }
    }
    tui_panes::render_pane(
        frame,
        progress_outer,
        tree.title(PROGRESS_PANE).expect("progress title"),
        focus.is_focused(PROGRESS_PANE),
    );
    tui_panes::render_pane(
        frame,
        current_outer,
        tree.title(CURRENT_PANE).expect("current title"),
        focus.is_focused(CURRENT_PANE),
    );
    if let Some(outer) = history_outer {
        tui_panes::render_pane(
            frame,
            outer,
            tree.title(HISTORY_PANE).expect("history title"),
            focus.is_focused(HISTORY_PANE),
        );
    }
    follow_progress(
        scrolls.get_mut(PROGRESS_PANE),
        *tree_follow,
        active_row,
        progress.len(),
        progress_inner.height as usize,
    );
    follow_tail(
        scrolls.get_mut(HISTORY_PANE),
        *history_follow,
        history.len(),
        history_inner.map_or(0, |rect| rect.height as usize),
    );
    follow_tail(
        scrolls.get_mut(CURRENT_PANE),
        *stream_follow,
        stream.len(),
        current_inner.height as usize,
    );
    tui_panes::render_wrapped_lines_pane(
        frame,
        progress_inner,
        &progress,
        scrolls.get_mut(PROGRESS_PANE),
    );
    if let Some(inner) = history_inner {
        tui_panes::render_wrapped_lines_pane(frame, inner, &history, scrolls.get_mut(HISTORY_PANE));
    }
    tui_panes::render_wrapped_lines_pane(
        frame,
        current_inner,
        &stream,
        scrolls.get_mut(CURRENT_PANE),
    );
    if let Some(modal) = modal {
        tui_kit::render_modal(frame, full_area, modal);
    }
}

fn follow_progress(
    scroll: &mut tui_kit::ViewportScroll,
    follow: bool,
    active_row: Option<usize>,
    len: usize,
    rows: usize,
) {
    scroll.set_len(len);
    scroll.clamp(rows);
    if follow {
        let target = active_row.unwrap_or_else(|| len.saturating_sub(1));
        align_scroll_start(scroll, target.saturating_sub(rows.saturating_sub(1)), rows);
    }
}

fn follow_tail(scroll: &mut tui_kit::ViewportScroll, follow: bool, len: usize, rows: usize) {
    scroll.set_len(len);
    scroll.clamp(rows);
    if follow {
        align_scroll_start(scroll, len.saturating_sub(rows), rows);
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

/// The story column's compressed history: one line per completed step, plan
/// order, `HH:MM:SS <label>: <summary>` when a P455 summary landed, otherwise
/// the truthful facts fallback `HH:MM:SS <label> · elapsed · tokens` — never a
/// placeholder. `HH:MM:SS` is when the row itself was stamped (the summary's own
/// landing time, or the step's own elapsed for the fallback).
fn story_history_lines(view: &RunView) -> Vec<tui::Line> {
    view.steps
        .iter()
        .filter(|step| step.state == StepState::Done)
        .map(story_row_line)
        .collect()
}

fn story_row_line(step: &RunStep) -> tui::Line {
    let mut line = tui::Line::blank();
    let at = step.summary_at.or(step.elapsed).unwrap_or_default();
    line.push(tui::elapsed_text(at), tui::Tone::Muted);
    line.push(" ", tui::Tone::Muted);
    match &step.summary {
        Some(summary) => {
            line.push(step.label.clone(), tui::Tone::Bold);
            line.push(": ", tui::Tone::Muted);
            line.push(tui::clean_live_text(summary), tui::Tone::Default);
        }
        None => {
            line.push(step.label.clone(), tui::Tone::Default);
            if let Some(elapsed) = step.elapsed {
                line.push(" \u{b7} ", tui::Tone::Muted);
                line.push(tui::elapsed_text(elapsed), tui::Tone::Muted);
            }
            if let Some(tokens) = step.output_tokens {
                line.push(" \u{b7} ", tui::Tone::Muted);
                line.push(tui::token_text(tokens), tui::Tone::Muted);
            }
        }
    }
    line
}

/// The CURRENT step's verbatim stream: every recorded row (narrations in
/// narrated mode, drained model-text deltas in passthrough mode) plus the
/// in-flight tail row — suppressed when its text duplicates the last
/// recorded row, so a just-landed narration is never shown twice.
fn story_stream_lines(state: &RunPanelState) -> Vec<tui::Line> {
    let mut lines = Vec::with_capacity(state.current_stream.len() + 1);
    let mut last_text: Option<&str> = None;
    for row in &state.current_stream {
        lines.push(stream_row_line(row));
        last_text = Some(row.text.as_str());
    }
    if let Some(narration) = display_narration(&state.view)
        && last_text != Some(narration.text.as_str())
    {
        lines.push(narration_line(narration));
    }
    lines
}

fn stream_row_line(row: &StreamRow) -> tui::Line {
    let mut line = tui::Line::blank();
    line.push(tui::elapsed_text(row.at), tui::Tone::Muted);
    line.push("  ", tui::Tone::Muted);
    let tone = match row.kind {
        StreamRowKind::Narration => tui::Tone::Default,
        StreamRowKind::ModelText => tui::Tone::Muted,
    };
    line.push(tui::clean_live_text(&row.text), tone);
    line
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
    trait_name: &str,
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
        run_id: session.run_id.as_str().to_string(),
        session_id: session.session_id.as_str().to_string(),
        trait_name: trait_name.to_string(),
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
        structured_count,
        structured_label: structured_producers
            .first()
            .map(|(_, _, _, port_id)| port_id.clone()),
        structured_verdict: crate::app::structured_output::producer_verdict(session),
    };
    RunView {
        header,
        steps,
        narration,
        outputs,
        // P549: never derived from `trait_ref`/`plan`/`session` — every
        // caller of this pure function overwrites this with the panel's own
        // folded `merge_rows` when a merge span is live.
        merge_rows: Vec::new(),
    }
}

/// P470: the tree (left) column's full, always-scrolled content — no line
/// cap, no compact-viewport degrade. Every step's mark/ports/facts renders in
/// full; the live narration itself moved to the story column's CURRENT
/// stream (see [`story_stream_lines`]) and is no longer inlined per-step.
/// Convenience wrapper over [`render_tree_lines_with_active_row`] for callers
/// that don't need the active row (e.g. [`render_ledger_run_view`], whose
/// ledger-only view never has an active step to anchor on).
fn render_tree_lines(view: &RunView) -> Vec<tui::Line> {
    render_tree_lines_with_active_row(view).0
}

/// Same output as [`render_tree_lines`], plus the rendered row index of the
/// ACTIVE step's [`render_step_summary`] line — the one true source of that
/// mapping, since this is the only place that knows how many rows each step
/// group emits. Follow-mode anchoring must derive from this row index, never
/// from `view.steps`' own item index (a different coordinate space: each
/// step group is 3 rows, `render_step_summary` + two `render_port_line`
/// calls, under a multi-row header the caller must not hand-count either).
fn render_tree_lines_with_active_row(view: &RunView) -> (Vec<tui::Line>, Option<usize>) {
    let mut lines = Vec::new();
    render_header(&mut lines, &view.header);
    lines.push(tui::Line::blank());
    muted_line(&mut lines, "journey");
    let target_step = active_step_index(view);
    let mut active_row = None;
    for (index, step) in view.steps.iter().enumerate() {
        if Some(index) == target_step {
            active_row = Some(lines.len());
        }
        render_step_group(&mut lines, step);
    }
    if let Some(narration) = completed_narration(view) {
        lines.push(narration_line(narration));
    }
    // P549: merge stage rows land directly after the run's own steps, so the
    // landing is one continuous journey rather than a separate section —
    // rendered before the completed-outputs box so a completed run whose
    // merge is still landing shows both.
    if !view.merge_rows.is_empty() {
        lines.push(tui::Line::blank());
        muted_line(&mut lines, "landing");
        for row in &view.merge_rows {
            render_merge_row(&mut lines, row);
        }
    }
    if view.header.completed {
        lines.push(tui::Line::blank());
        render_outputs_box(&mut lines, &view.outputs);
    }
    (lines, active_row)
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
pub(crate) fn render_ledger_run_view(
    trait_ref: &ctx_traits_core::Trait,
    plan: &ctx_traits_core::procedure::run::Plan,
    session: &ctx_traits_core::procedure::session::Session,
) -> Vec<tui::Line> {
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
    let view = run_view(
        trait_ref.name.as_str(),
        trait_ref,
        plan,
        session,
        None,
        PresentationState {
            active_started: &None,
            finished_durations: &BTreeMap::new(),
            output_tokens: &output_tokens,
            loop_elapsed: &BTreeMap::new(),
            loop_output_tokens: &BTreeMap::new(),
            step_summaries: &BTreeMap::new(),
            step_summary_at: &BTreeMap::new(),
            narrator_tokens,
            run_started,
        },
    );
    render_tree_lines(&view)
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

fn render_header(lines: &mut Vec<tui::Line>, header: &RunHeader) {
    let mut line = tui::Line::blank();
    line.push("run ", tui::Tone::Muted);
    line.push(header.run_id.clone(), tui::Tone::Default);
    line.push(" · trait ", tui::Tone::Muted);
    line.push(header.trait_name.clone(), tui::Tone::Default);
    lines.push(line);

    let mut line = tui::Line::blank();
    line.push("session ", tui::Tone::Muted);
    line.push(header.session_id.clone(), tui::Tone::Default);
    lines.push(line);

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

        // The session id already sits in the header; only the digest stamp is
        // completion-specific.
        let mut line = tui::Line::blank();
        line.push("digest-stamped ", tui::Tone::Muted);
        line.push(header.state_digest.clone(), tui::Tone::Default);
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

/// P470: the tree's per-step group is now facts-only (mark, ports) — the
/// active step's live narration moved to the story column's CURRENT stream
/// (see [`story_stream_lines`]) and is never inlined here.
fn render_step_group(lines: &mut Vec<tui::Line>, step: &RunStep) {
    render_step_summary(lines, step);
    render_port_line(lines, "in", &step.inputs);
    render_port_line(lines, "out", &step.outputs);
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

fn render_step_summary(lines: &mut Vec<tui::Line>, step: &RunStep) {
    let (mark, tone) = match step.state {
        StepState::Done => ("✓", tui::Tone::Pass),
        StepState::Running => ("~", tui::Tone::Warn),
        StepState::Pending => ("○", tui::Tone::Muted),
        StepState::Failed => ("×", tui::Tone::Fail),
    };
    let mut line = tui::Line::blank();
    line.push(mark, tone);
    line.push(" ", tui::Tone::Muted);
    line.push(step.label.clone(), tone);
    line.push("   ", tui::Tone::Muted);
    line.push(role_harness_text(step), tui::Tone::Muted);
    if !step.tags.is_empty() {
        line.push("   ", tui::Tone::Muted);
        line.push(step.tags.join(" ∙ "), tui::Tone::Muted);
    }
    line.push("   ", tui::Tone::Muted);
    line.push(step.status.clone(), status_tone(step));
    if let Some(elapsed) = step.elapsed {
        line.push(" (", tui::Tone::Muted);
        line.push(tui::elapsed_text(elapsed), tui::Tone::Muted);
        if let Some(tokens) = step.output_tokens {
            line.push(format!(" · {}", tui::token_text(tokens)), tui::Tone::Muted);
        }
        line.push(")", tui::Tone::Muted);
    }
    if step.structured_count > 0 {
        line.push(" ", tui::Tone::Muted);
        line.push(
            format!("({} open)", step.structured_count),
            tui::Tone::Default,
        );
    }
    lines.push(line);
}

fn render_port_line(lines: &mut Vec<tui::Line>, label: &str, ports: &[PortSlug]) {
    let mut line = tui::Line::blank();
    line.push(format!("    {label} "), tui::Tone::Muted);
    if ports.is_empty() {
        line.push("none", tui::Tone::Muted);
    } else {
        for (index, port) in ports.iter().enumerate() {
            if index > 0 {
                line.push(" ", tui::Tone::Muted);
            }
            line.push(
                port.slug.clone(),
                if port.satisfied {
                    tui::Tone::Pass
                } else {
                    tui::Tone::Muted
                },
            );
        }
    }
    lines.push(line);
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
        let Some(frame) = session.control_stack.get(depth) else {
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

fn muted_line(lines: &mut Vec<tui::Line>, text: &str) {
    let mut line = tui::Line::blank();
    line.push(text, tui::Tone::Muted);
    lines.push(line);
}

#[cfg(test)]
mod tests {
    use super::*;

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
                Some("pre-landing gate failed"),
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
            "merge:landing",
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

    fn view_with(steps: Vec<RunStep>) -> RunView {
        RunView {
            header: RunHeader {
                run_id: "run-1".to_string(),
                session_id: "sess-1".to_string(),
                trait_name: "demo".to_string(),
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
                structured_count: 0,
                structured_label: None,
                structured_verdict: None,
            },
            steps,
            narration: None,
            outputs: Vec::new(),
            merge_rows: Vec::new(),
        }
    }

    // P470 §6 "story derivation": N done steps -> exactly N rows in plan
    // order; running/pending steps produce no story row.
    #[test]
    fn story_history_lines_only_covers_done_steps_in_plan_order() {
        let view = view_with(vec![
            step("a", StepState::Done, None),
            step("b", StepState::Running, None),
            step("c", StepState::Done, Some("finished cleanly")),
            step("d", StepState::Pending, None),
        ]);
        let lines = story_history_lines(&view);
        assert_eq!(lines.len(), 2);
    }

    // A step with a landed P455 summary joins its row; one without falls
    // back to the truthful facts line — never a placeholder.
    #[test]
    fn story_row_line_prefers_summary_over_facts_fallback() {
        let with_summary = story_row_line(&step("a", StepState::Done, Some("did the thing")));
        let texts: Vec<&str> = with_summary.segments().map(|(text, _)| text).collect();
        assert_eq!(texts[0], "00:00:07");
        assert!(texts.contains(&"did the thing"));

        let without_summary = story_row_line(&step("b", StepState::Done, None));
        let texts: Vec<&str> = without_summary.segments().map(|(text, _)| text).collect();
        assert_eq!(texts[0], "00:00:05");
        assert!(texts.iter().any(|text| text.contains('5'))); // elapsed
        assert!(texts.iter().any(|text| text.contains("tok"))); // tokens
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
        let narration_tone = narration.segments().last().map(|(_, tone)| tone);
        let model_tone = model_text.segments().last().map(|(_, tone)| tone);
        assert_eq!(
            narration.segments().next().map(|(text, _)| text),
            Some("00:00:03")
        );
        assert_eq!(
            model_text.segments().next().map(|(text, _)| text),
            Some("00:00:03")
        );
        assert_eq!(narration_tone, Some(tui::Tone::Default));
        assert_eq!(model_tone, Some(tui::Tone::Muted));
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
        let (lines, active_row) = render_tree_lines_with_active_row(&view);
        let active_row = active_row.expect("an active step must yield a row anchor");
        // The active step's own `view.steps` index is 2; its row can never
        // be that small once a header, a blank line, a "journey" label, and
        // two 3-row `Done` step groups all precede it.
        assert_ne!(
            active_row, 2,
            "the anchor must be a rendered row index, not the step's item index"
        );
        let label = lines[active_row]
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
    fn render_tree_lines_with_active_row_is_none_when_no_step_is_selectable() {
        let (_, active_row) = render_tree_lines_with_active_row(&view_with(Vec::new()));
        assert_eq!(active_row, None);
    }

    #[test]
    fn narrow_live_tree_omits_history_and_reconciles_focus_to_current() {
        let tree = live_pane_tree(Rect::new(0, 0, 80, 24), 10);
        assert_eq!(tree.leaf_ids(), vec![PROGRESS_PANE, CURRENT_PANE]);
        let mut focus = FocusRing::new(vec![HISTORY_PANE]);
        focus.reconcile(tree.leaf_ids(), CURRENT_PANE);
        assert_eq!(focus.current(), Some(CURRENT_PANE));
    }

    #[test]
    fn wide_story_caps_history_and_preserves_current_content_floor() {
        let area = Rect::new(0, 0, 120, 24);
        let tree = live_pane_tree(area, 100);
        let layout = tree.resolve(area);
        let history = layout.rect(HISTORY_PANE).expect("history");
        let current = layout.rect(CURRENT_PANE).expect("current");
        assert!(history.height <= (area.height - CURRENT_MIN_OUTER_ROWS) / 2);
        assert!(current.height >= CURRENT_MIN_OUTER_ROWS);
        assert!(tui_panes::pane_inner(current).height >= 6);
    }

    #[test]
    fn focus_ring_contains_only_drawable_panes_at_small_sizes() {
        for area in [
            Rect::new(0, 0, 80, 6),
            Rect::new(0, 0, 80, 7),
            Rect::new(0, 0, 120, 6),
            Rect::new(0, 0, 120, 7),
        ] {
            let tree = live_pane_tree(area, 100);
            let layout = tree.resolve(area);
            let ids = drawable_pane_ids(&tree, &layout);
            let mut focus = FocusRing::new(vec![CURRENT_PANE]);
            focus.reconcile(ids.clone(), CURRENT_PANE);
            for _ in 0..ids.len() {
                assert!(ids.contains(&focus.current().expect("drawable focus")));
                focus.next();
            }
        }

        let area = Rect::new(0, 0, 120, 24);
        let tree = live_pane_tree(area, 100);
        let layout = tree.resolve(area);
        let ids = drawable_pane_ids(&tree, &layout);
        assert_eq!(ids, vec![PROGRESS_PANE, HISTORY_PANE, CURRENT_PANE]);
        let mut focus = FocusRing::new(ids.clone());
        for expected in ids.iter().cycle().take(ids.len() * 2) {
            assert_eq!(focus.current(), Some(*expected));
            focus.next();
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

    #[test]
    fn focus_keys_apply_before_the_same_frame_and_route_following_scroll() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::style::Modifier;

        let tree_lines = (0..20)
            .map(|index| {
                let mut line = tui::Line::blank();
                line.push(format!("progress {index}"), tui::Tone::Default);
                line
            })
            .collect::<Vec<_>>();
        let mut scrolls = PaneScrolls::new();
        let mut tree_follow = true;
        let mut history_follow = true;
        let mut stream_follow = true;
        let mut focus = FocusRing::new(vec![CURRENT_PANE]);
        let mut keys = vec![
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        ];
        let mut terminal = Terminal::new(TestBackend::new(120, 12)).expect("test terminal");
        terminal
            .draw(|frame| {
                render_live_panes(
                    frame,
                    LivePaneFrame {
                        tree_lines: &tree_lines,
                        active_row: Some(0),
                        history_lines: &[],
                        stream_lines: &[],
                        scrolls: &mut scrolls,
                        tree_follow: &mut tree_follow,
                        history_follow: &mut history_follow,
                        stream_follow: &mut stream_follow,
                        focus: &mut focus,
                        pending_keys: &mut keys,
                        modal: None,
                    },
                );
            })
            .expect("draw");
        assert_eq!(focus.current(), Some(PROGRESS_PANE));
        assert!(!tree_follow);
        assert_eq!(scrolls.get(PROGRESS_PANE).window(10).start, 1);
        assert!(
            terminal
                .backend()
                .buffer()
                .cell((1, 0))
                .expect("progress title cell")
                .style()
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            terminal
                .backend()
                .buffer()
                .cell((73, 0))
                .expect("current title cell")
                .style()
                .add_modifier
                .contains(Modifier::DIM)
        );

        let mut keys = vec![KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE)];
        terminal
            .draw(|frame| {
                render_live_panes(
                    frame,
                    LivePaneFrame {
                        tree_lines: &tree_lines,
                        active_row: Some(0),
                        history_lines: &[],
                        stream_lines: &[],
                        scrolls: &mut scrolls,
                        tree_follow: &mut tree_follow,
                        history_follow: &mut history_follow,
                        stream_follow: &mut stream_follow,
                        focus: &mut focus,
                        pending_keys: &mut keys,
                        modal: None,
                    },
                );
            })
            .expect("draw");
        assert_eq!(focus.current(), Some(CURRENT_PANE));
        assert!(
            terminal
                .backend()
                .buffer()
                .cell((1, 0))
                .expect("progress title cell")
                .style()
                .add_modifier
                .contains(Modifier::DIM)
        );
        assert!(
            terminal
                .backend()
                .buffer()
                .cell((73, 0))
                .expect("current title cell")
                .style()
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn tiny_narrow_frame_keeps_current_activity_drawable() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut line = tui::Line::blank();
        line.push("unique current activity", tui::Tone::Default);
        let mut scrolls = PaneScrolls::new();
        let mut tree_follow = true;
        let mut history_follow = true;
        let mut stream_follow = true;
        let mut focus = FocusRing::new(vec![CURRENT_PANE]);
        let mut keys = Vec::new();
        let mut terminal = Terminal::new(TestBackend::new(80, 6)).expect("test terminal");
        terminal
            .draw(|frame| {
                render_live_panes(
                    frame,
                    LivePaneFrame {
                        tree_lines: &[],
                        active_row: None,
                        history_lines: &[],
                        stream_lines: &[line],
                        scrolls: &mut scrolls,
                        tree_follow: &mut tree_follow,
                        history_follow: &mut history_follow,
                        stream_follow: &mut stream_follow,
                        focus: &mut focus,
                        pending_keys: &mut keys,
                        modal: None,
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
        );
        assert!(follow);
        let tail_window = scroll.window(rows);

        apply_scroll_and_derive_follow(&mut scroll, &mut follow, tui_kit::ScrollDelta::Up(3), rows);
        assert!(!follow, "scrolling up must release follow");
        let scrolled_up_window = scroll.window(rows);
        assert_ne!(scrolled_up_window, tail_window);

        apply_scroll_and_derive_follow(
            &mut scroll,
            &mut follow,
            tui_kit::ScrollDelta::Down(1),
            rows,
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
        );
        assert!(follow, "reaching the tail edge must re-engage follow");
        assert_eq!(scroll.window(rows), tail_window);
    }

    #[test]
    fn tail_follow_advances_without_reset_and_stays_pinned_after_resize() {
        let mut scroll = tui_kit::ViewportScroll::new();
        follow_tail(&mut scroll, true, 30, 10);
        assert_eq!(scroll.window(10), 20..30);

        follow_tail(&mut scroll, true, 35, 10);
        assert_eq!(scroll.window(10), 25..35);
        assert_ne!(scroll.window(10).start, 0);

        follow_tail(&mut scroll, true, 35, 15);
        assert_eq!(scroll.window(15), 20..35);
    }

    #[test]
    fn non_following_tail_keeps_its_window_when_stream_grows() {
        let mut scroll = tui_kit::ViewportScroll::new();
        follow_tail(&mut scroll, true, 40, 10);
        let mut follow = false;
        apply_scroll_and_derive_follow(&mut scroll, &mut follow, tui_kit::ScrollDelta::Up(12), 10);
        assert!(!follow);
        let stationary = scroll.window(10);

        follow_tail(&mut scroll, false, 55, 10);
        assert_eq!(scroll.window(10), stationary);
    }

    #[test]
    fn progress_follow_aligns_each_active_rendered_row_without_overshoot() {
        let mut scroll = tui_kit::ViewportScroll::new();
        follow_progress(&mut scroll, true, Some(12), 40, 10);
        assert_eq!(scroll.window(10), 3..13);

        follow_progress(&mut scroll, true, Some(21), 40, 10);
        assert_eq!(scroll.window(10), 12..22);

        follow_progress(&mut scroll, true, Some(30), 40, 10);
        assert_eq!(scroll.window(10), 21..31);
    }

    #[test]
    fn progress_follow_keeps_the_active_row_at_bottom_after_resize() {
        let mut scroll = tui_kit::ViewportScroll::new();
        follow_progress(&mut scroll, true, Some(58), 60, 10);
        assert_eq!(scroll.window(10), 49..59);

        follow_progress(&mut scroll, true, Some(58), 60, 20);
        assert_eq!(scroll.window(20), 39..59);
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
}
