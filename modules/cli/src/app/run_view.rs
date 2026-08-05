//! Live run-journey presentation for driven procedure sessions.
//!
//! This module is CLI presentation only. It maps an already-built dry plan plus
//! live session state into styled terminal lines; it never mutates the run ledger
//! or changes driver/report semantics.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent};
use ctx_traits_core::procedure::activity::ActivityEvent;

use crate::app::tui;
use crate::app::tui_kit;
use crate::app::tui_panes::{self, FocusRing, PaneScrolls};
use crate::app::tui_ratatui::RatatuiPane;
#[cfg(test)]
use ratatui::layout::Rect;

mod guide;
mod model;
mod planned;
mod projection;
mod render;
mod session_text;

pub(crate) use guide::GuideChatHandle;
use guide::GuideDispatch;
use guide::apply_ask_key;

#[allow(unused_imports)]
pub(crate) use projection::{
    LedgerPaneProjection, SidecarActivitySummary, load_sidecar_activity_summary,
    post_run_lines_from_frames, render_ledger_run_view,
};
use projection::{apply_ledger_seed, run_view};

use render::*;
#[allow(unused_imports)]
pub(crate) use render::{
    EventRow, PaneFollow, PaneRenderState, PaneTitleRow, pane_body_area, pane_tree,
    render_pane_body,
};

use model::*;
#[allow(unused_imports)]
pub(crate) use model::{CompletedStepContext, JourneyRow, PaneData, PaneIds, journey_line};

use planned::active_loop_container_keys;
use session_text::active_key;
pub(crate) use session_text::{
    phase_text, session_status, stop_reason_summary, title_prompt_context_for,
};

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
    /// Throttle stamp for the live pane's own ledger re-read (see
    /// [`maybe_reload_from_ledger`]); meaningless for observers, whose
    /// caller drives reloads through [`RunPanel::refresh_from_ledger`].
    last_ledger_reload: Instant,
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
    /// `Some` only while `modal` is an [`RunPanel::request_input`] prompt —
    /// distinguishes it from the plain `q` confirm-quit modal, which shares
    /// the same `modal` slot but has no reply channel. The drive thread that
    /// opened the prompt blocks on the receiver end; `poll_and_apply_keys`
    /// sends the submitted text (or `None` on cancel) exactly once, on the
    /// key that resolves the modal, and clears both fields together so a
    /// stale sender is never left installed under an unrelated modal.
    pending_input_reply: Option<mpsc::Sender<Option<String>>>,
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
                live_drive: true,
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
            last_ledger_reload: now,
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
            pending_input_reply: None,
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

    /// Opens a text-input modal asking for a missing input port's value and
    /// returns the receiver end of its reply channel. The pane's own input
    /// thread (see `install_input_wake` in [`Self::new_with_pane`]) applies
    /// keys and resolves the modal independently of whatever thread is
    /// blocked reading the receiver — the drive loop parks on `recv()` while
    /// this pane keeps painting and taking keystrokes, the same split the
    /// guide chat's dispatch thread and the command-frame `tick_observer`
    /// already rely on. `Ok(Some(text))` is a submitted answer, `Ok(None)`
    /// is an explicit cancel (Esc), and `Err` means the pane's state lock is
    /// gone (panel closed out from under the request) — a caller must treat
    /// the latter two the same way (fall back to the non-interactive path).
    pub(crate) fn request_input(
        &self,
        title: String,
        body: String,
    ) -> mpsc::Receiver<Option<String>> {
        let _handoff = self.handoff_driver();
        let (sender, receiver) = mpsc::channel();
        if let Ok(mut state) = self.state.lock() {
            state.modal = Some(tui_kit::Modal::text_input_with_body(
                title,
                body,
                String::new(),
                false,
            ));
            state.pending_input_reply = Some(sender);
            render_locked(&mut state);
        }
        receiver
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

    /// Install the persisted ledger path for a LIVE drive panel so `tick`
    /// can re-derive journey rows while a blocking command frame runs: the
    /// drive thread is inside `run::call` for the whole command chain and
    /// cannot call [`Self::refresh`], but the ledger on disk is already past
    /// the acceptance that preceded the commands. Observers install their
    /// path in [`Self::new_observer`] and are refreshed by their caller
    /// through [`Self::refresh_from_ledger`] instead.
    pub(crate) fn set_live_ledger_path(&self, path: camino::Utf8PathBuf) {
        if let Ok(mut state) = self.state.lock() {
            state.ledger_path = Some(path);
        }
    }

    pub(crate) fn tick(&self) {
        let _handoff = self.handoff_driver();
        if !self.cadence.should_run() {
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        maybe_reload_from_ledger(&mut state);
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

/// Cadence for the live pane's own ledger re-read below — matches the
/// dashboard observer's reload interval.
const LIVE_LEDGER_RELOAD_INTERVAL: Duration = Duration::from_secs(2);

/// While the drive thread is blocked inside a command frame, no
/// [`RunPanel::refresh`] can arrive: `run::call` drains every consecutive
/// command frame (branch resolution, project steps, the commands themselves)
/// in one blocking call, and the only signal out is the tick observer. The
/// pane would keep rendering the pre-command snapshot — the already-finished
/// prompt step reading "in-progress" for however long the command runs (a
/// plannotator gate holds it for hours). The ledger on disk is already past
/// the acceptance, so a throttled re-read moves the journey onto the command
/// row and materializes any branch-arm rows the decision created.
///
/// Deliberately leaves `active_key`/`active_started` untouched:
/// [`RunPanel::refresh`] stays the sole transition detector (P455), so
/// step-summary narration and duration crediting behave exactly as before.
/// A transient read/parse error (including a torn mid-write read) skips the
/// cycle and keeps the last frame, mirroring the dashboard's degrade
/// discipline.
fn maybe_reload_from_ledger(state: &mut RunPanelState) {
    if state.observer {
        return;
    }
    let Some(path) = state.ledger_path.clone() else {
        return;
    };
    if state.last_ledger_reload.elapsed() < LIVE_LEDGER_RELOAD_INTERVAL {
        return;
    }
    state.last_ledger_reload = Instant::now();
    let Ok(session) = ctx_traits_io::run_session::read_run_session(&path) else {
        return;
    };
    if session.state_digest.as_str() == state.session.state_digest.as_str() {
        return;
    }
    state.session = session;
    let narration = state.view.narration.clone();
    rebuild_view(state, narration);
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
    // Task 0023: a drag or mouseup wakes this tick the same way a key or
    // resize does (the pump's mouse arms bump the input generation), but
    // left no trace `changed` recognised until `take_mouse_changed` — so the
    // drag highlight and the mouseup's copy went unpainted until an
    // unrelated key/resize/focus forced the next frame. Same failure class
    // `focus_changed` above was extracted to fix.
    let mouse_changed = state.repaint.take_mouse_changed();
    let changed = poll_and_apply_keys(state) || resized || focus_changed || mouse_changed;
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
            live_drive: !state.observer,
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

/// Routes one key through `state`'s open modal, if any — the `q` confirm-quit
/// dialog and an [`RunPanel::request_input`] prompt share this one slot.
/// Returns `true` iff a modal was open and consumed the key (the caller must
/// not fall through to any other key handling for it), so the state machine
/// itself can be exercised directly without a real terminal forwarding keys
/// through `state.repaint`.
fn apply_open_modal_key(state: &mut RunPanelState, key: &KeyEvent) -> bool {
    let Some(modal) = state.modal.as_mut() else {
        return false;
    };
    match modal.handle_key(key) {
        tui_kit::ModalOutcome::Confirmed => {
            state.modal = None;
            state.repaint.quit();
            // P081: an observer's `q` returns to the dashboard
            // automatically — this message is only accurate for the
            // live view's own quit, which leaves no dashboard to
            // return to.
            if !state.observer {
                eprintln!("live view closed; run continues — reattach with ctx traits dashboard");
            }
        }
        tui_kit::ModalOutcome::Cancelled => {
            state.modal = None;
            if let Some(reply) = state.pending_input_reply.take() {
                let _ = reply.send(None);
            }
        }
        tui_kit::ModalOutcome::Submitted(text) => {
            if let Some(reply) = state.pending_input_reply.take() {
                state.modal = None;
                let _ = reply.send(Some(text));
            }
        }
        tui_kit::ModalOutcome::Pending => {}
    }
    true
}

fn poll_and_apply_keys(state: &mut RunPanelState) -> bool {
    let mut changed = false;
    if let Some(ask) = state.ask.as_ref() {
        changed |= ask.poll_results();
    }
    let keys = state.repaint.poll_detach();
    for key in keys {
        if apply_open_modal_key(state, &key) {
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

/// 0082: `Ladder` replaces the old bottom-pinned `ActiveRow` alignment.
/// `rows` is the ladder from [`journey_lines_with_active_row`] — every row on
/// the active path, outermost first, with the active row itself appended as
/// the last (innermost) element; empty when there is no active row. A slice
/// ref (not an owned `Vec`) keeps this `Copy`, since its lifetime never
/// outlives the render pass that resolved it.
#[derive(Clone, Copy)]
enum FollowTarget<'a> {
    Tail,
    Ladder(&'a [usize]),
}

impl FollowTarget<'_> {
    /// The viewport start which puts this target at its follow alignment,
    /// clamped using the same bounds as `ViewportScroll`.
    fn viewport_start(self, len: usize, rows: usize) -> usize {
        let desired_start = match self {
            Self::Tail => len.saturating_sub(rows),
            // Empty ladder: no active row, so there is nothing to pin to the
            // top — this must equal the old `ActiveRow(None)` bottom-pin
            // exactly, for parity on the anchor-less preview surface.
            Self::Ladder([]) => len.saturating_sub(rows),
            Self::Ladder(ladder) => {
                let (&anchor, ancestors) = ladder
                    .split_last()
                    .expect("non-empty ladder has a last element");
                // Outermost enclosing row that still leaves the anchor
                // visible — the first (outermost) ladder row whose header-to-
                // anchor span fits within `rows`; the anchor itself is the
                // floor when none do.
                ancestors
                    .iter()
                    .copied()
                    .find(|&row| anchor.saturating_sub(row) <= rows.saturating_sub(1))
                    .unwrap_or(anchor)
            }
        };
        desired_start.min(len.saturating_sub(rows.min(len)))
    }

    fn is_following(self, scroll: &tui_kit::ViewportScroll, len: usize, rows: usize) -> bool {
        match self {
            Self::Tail => scroll.is_at_bottom(rows),
            // `window(0)` is always empty, so its start cannot describe the
            // persisted offset. At zero height, bottom remains the only
            // observable follow alignment.
            Self::Ladder(_) if rows == 0 => scroll.is_at_bottom(rows),
            Self::Ladder(_) => scroll.window(rows).start == self.viewport_start(len, rows),
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
    target: FollowTarget<'_>,
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

#[cfg(test)]
mod tests {
    use super::*;

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
            on_active_path: false,
            position_path: Vec::new(),
            run_index: 0,
            structured_count: 0,
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

    /// `RunPanel::request_input`'s reply channel resolves on submit — a
    /// caller blocked on the receiver (the drive loop) unblocks with the
    /// typed text once `apply_open_modal_key` routes the terminating `Enter`
    /// through the open `Modal::TextInput`.
    #[test]
    fn request_input_submit_resolves_the_reply_channel_with_the_typed_text() {
        let trait_ref: ctx_traits_core::Trait = toml::from_str(
            r#"
id = "request-input-test"
schema-version = "0.2"
version = "0.1.0"
name = "Request Input Test"
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
        let panel = RunPanel::new_with_pane(
            "request-input-test".to_string(),
            trait_ref,
            plan,
            session,
            RatatuiPane::new_detached_for_test(),
        );

        let receiver = panel.request_input(
            "Missing input: port:missing".to_string(),
            "The prompt to run against.".to_string(),
        );
        {
            let state = panel.state.lock().expect("state lock");
            assert!(state.modal.is_some(), "request_input must open a modal");
            assert!(
                state.pending_input_reply.is_some(),
                "request_input must install a reply sender"
            );
        }

        for ch in "hello".chars() {
            let mut state = panel.state.lock().expect("state lock");
            apply_open_modal_key(
                &mut state,
                &KeyEvent::new(KeyCode::Char(ch), crossterm::event::KeyModifiers::NONE),
            );
        }
        {
            let mut state = panel.state.lock().expect("state lock");
            apply_open_modal_key(
                &mut state,
                &KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE),
            );
            assert!(state.modal.is_none(), "submit must close the modal");
            assert!(
                state.pending_input_reply.is_none(),
                "submit must consume the reply sender"
            );
        }

        assert_eq!(
            receiver.recv().expect("reply sent"),
            Some("hello".to_string())
        );
    }

    /// The cancel (Esc) path resolves the SAME channel with `None`, rather
    /// than leaving the drive loop's `recv()` blocked forever — this is the
    /// path the live drive loop falls back to today's fail-fast
    /// `awaiting-input` exit through.
    #[test]
    fn request_input_cancel_resolves_the_reply_channel_with_none() {
        let trait_ref: ctx_traits_core::Trait = toml::from_str(
            r#"
id = "request-input-cancel-test"
schema-version = "0.2"
version = "0.1.0"
name = "Request Input Cancel Test"
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
        let panel = RunPanel::new_with_pane(
            "request-input-cancel-test".to_string(),
            trait_ref,
            plan,
            session,
            RatatuiPane::new_detached_for_test(),
        );

        let receiver =
            panel.request_input("Missing input: port:missing".to_string(), String::new());
        {
            let mut state = panel.state.lock().expect("state lock");
            apply_open_modal_key(
                &mut state,
                &KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE),
            );
            assert!(state.modal.is_none(), "cancel must close the modal");
        }

        assert_eq!(receiver.recv().expect("reply sent"), None);
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
                        journey_ladder: &[0],
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
                        journey_ladder: &[0],
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
                        journey_ladder: &[0],
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
                        journey_ladder: &[0],
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
                            journey_ladder: &[],
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
    fn zero_height_follow_ladder_reengages_at_end_only() {
        let mut scroll = tui_kit::ViewportScroll::new();
        scroll.set_len(30);
        let mut follow = false;
        let target = FollowTarget::Ladder(&[12]);

        apply_scroll_and_derive_follow(
            &mut scroll,
            &mut follow,
            tui_kit::ScrollDelta::End,
            0,
            30,
            target,
        );
        assert!(follow, "end must reengage ladder following at zero height");

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
            "a non-bottom position must release ladder following at zero height"
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
    fn journey_tail_does_not_reengage_ladder_follow() {
        let mut scroll = tui_kit::ViewportScroll::new();
        let target = FollowTarget::Ladder(&[12]);
        follow_target(&mut scroll, true, target, 40, 10);
        assert_eq!(scroll.window(10), 12..22, "the anchor row is the top line");

        let mut follow = true;
        apply_scroll_and_derive_follow(
            &mut scroll,
            &mut follow,
            tui_kit::ScrollDelta::End,
            10,
            40,
            target,
        );
        assert!(!follow, "the final window is not the ladder alignment");
        assert_eq!(scroll.window(10), 30..40);

        // A render while released must leave the user at the pending tail.
        follow_target(&mut scroll, follow, target, 40, 10);
        assert_eq!(scroll.window(10), 30..40);
    }

    #[test]
    fn journey_reengages_only_at_ladder_alignment() {
        let mut scroll = tui_kit::ViewportScroll::new();
        let target = FollowTarget::Ladder(&[12]);
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
            "a nearby ladder alignment must not be enough to reengage"
        );
        assert_eq!(scroll.window(10), 14..24);

        apply_scroll_and_derive_follow(
            &mut scroll,
            &mut follow,
            tui_kit::ScrollDelta::Up(2),
            10,
            40,
            target,
        );
        assert!(follow, "returning to the exact alignment resumes following");
        assert_eq!(scroll.window(10), 12..22);
    }

    #[test]
    fn journey_follow_target_falls_back_to_tail_and_handles_tail_alignment() {
        assert_eq!(FollowTarget::Ladder(&[]).viewport_start(40, 10), 30);
        assert_eq!(FollowTarget::Ladder(&[39]).viewport_start(40, 10), 30);
    }

    #[test]
    fn journey_follow_pins_the_anchor_row_to_the_top_without_overshoot() {
        let mut scroll = tui_kit::ViewportScroll::new();
        follow_target(&mut scroll, true, FollowTarget::Ladder(&[12]), 40, 10);
        assert_eq!(scroll.window(10), 12..22);

        follow_target(&mut scroll, true, FollowTarget::Ladder(&[21]), 40, 10);
        assert_eq!(scroll.window(10), 21..31);

        follow_target(&mut scroll, true, FollowTarget::Ladder(&[30]), 40, 10);
        assert_eq!(
            scroll.window(10),
            30..40,
            "clamped: 30 is the last valid start"
        );
    }

    #[test]
    fn journey_follow_keeps_the_anchor_visible_after_resize_near_the_end() {
        let mut scroll = tui_kit::ViewportScroll::new();
        follow_target(&mut scroll, true, FollowTarget::Ladder(&[58]), 60, 10);
        assert_eq!(
            scroll.window(10),
            50..60,
            "clamped, but the anchor stays visible"
        );

        follow_target(&mut scroll, true, FollowTarget::Ladder(&[58]), 60, 20);
        assert_eq!(
            scroll.window(20),
            40..60,
            "clamped, but the anchor stays visible"
        );
    }

    // 0082: `journey_lines_with_active_row` resolves the ladder from
    // `RunStep::on_active_path`, pre-order (outermost first), with the
    // active row appended as the floor.
    #[test]
    fn journey_ladder_resolves_outermost_first_with_the_active_row_as_the_floor() {
        let mut outer_loop = step("outer loop", StepState::Running, None);
        outer_loop.on_active_path = true;
        let mut inner_loop = step("inner loop", StepState::Running, None);
        inner_loop.on_active_path = true;
        let mut active_step = step("body", StepState::Running, None);
        active_step.active = true;
        let view = view_with(vec![outer_loop, inner_loop, active_step]);
        let (_, active_row, ladder) = journey_lines_with_active_row(&view);
        assert_eq!(active_row, Some(2));
        assert_eq!(
            ladder,
            vec![0, 1, 2],
            "outermost container first, active row last"
        );
    }

    #[test]
    fn journey_ladder_is_empty_when_no_step_is_active() {
        let view = view_with(vec![step("a", StepState::Pending, None)]);
        let (_, active_row, ladder) = journey_lines_with_active_row(&view);
        assert_eq!(active_row, None);
        assert!(ladder.is_empty());
    }

    #[test]
    fn ladder_viewport_start_prefers_the_outermost_row_that_fits() {
        let ladder = [0usize, 5, 12];
        // Tall viewport: the outer loop header (row 0) fits above the anchor.
        assert_eq!(FollowTarget::Ladder(&ladder).viewport_start(40, 15), 0);
        // Shrink until only the inner header still fits.
        assert_eq!(FollowTarget::Ladder(&ladder).viewport_start(40, 8), 5);
        // Shrink below that: the step itself is the top line, never scrolled out.
        assert_eq!(FollowTarget::Ladder(&ladder).viewport_start(40, 5), 12);
    }

    #[test]
    fn ladder_viewport_start_clamps_near_the_end_of_the_list_but_keeps_the_anchor_visible() {
        let ladder = [0usize, 5, 38];
        let start = FollowTarget::Ladder(&ladder).viewport_start(40, 10);
        assert_eq!(start, 30);
        assert!(
            (start..start + 10).contains(&38),
            "the anchor must stay visible"
        );
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
}
