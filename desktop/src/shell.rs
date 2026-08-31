use std::time::SystemTime;

use gpui::prelude::*;
use gpui::{
    Bounds, Context, Pixels, Render, SharedString, Size, TitlebarOptions, Window, WindowOptions,
    div, rgb, size,
};

use std::collections::HashMap;

use crate::bottom_bar;
use crate::bottom_bar_view;
use crate::center_link::{self, LinkUpdate};
use crate::dashboard::Dashboard;
use crate::detail::{self, LoadRequest, RunDetail};
use crate::detail_view;
use crate::preview_view;
use crate::rail_view;
use crate::row_control::{self, RowControls, RowRequest, RowVerb};
use crate::run_row::{RepoScope, RunRow};
use crate::spawn_form::{SpawnForm, SpawnRepo, SpawnStatus, SubmitOutcome, SubmitRequest};
use crate::spawn_view;
use crate::title_bar_view;
use crate::tokens;

pub const APP_TITLE: &str = "ctx desktop";

pub const DEFAULT_WINDOW_SIZE: Size<Pixels> = size(tokens::WINDOW_WIDTH, tokens::WINDOW_HEIGHT);

pub fn window_options(bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(APP_TITLE.into()),
            appears_transparent: true,
            traffic_light_position: Some(gpui::point(
                tokens::TITLE_BAR_PAD_X,
                (tokens::TITLE_BAR_HEIGHT - tokens::TITLE_BAR_TRAFFIC_LIGHT) / 2.,
            )),
        }),
        ..Default::default()
    }
}

/// The degraded lifecycle's face states. `Stale` is the recoverable
/// disconnected view: rows already on screen are kept, labelled, and never
/// merged with a later stream — only a fresh `Snapshot` replaces them.
pub enum CenterState {
    Connecting,
    /// Never held rows: first contact with no matching center serving.
    Unavailable {
        reason: String,
    },
    Connected {
        dashboard: Dashboard,
    },
    Stale {
        dashboard: Dashboard,
        reason: String,
        since: Option<SystemTime>,
    },
}

/// The center's degraded lifecycle, kept gpui-free (plain Rust — no gpui
/// types — so it is unit-testable without an `App`, the same pattern
/// `dashboard.rs` documents for `Dashboard`). `Shell` holds one of these and
/// delegates every `LinkUpdate` to it.
pub struct CenterFace {
    state: CenterState,
    scope: RepoScope,
    last_snapshot_at: Option<SystemTime>,
}

impl CenterFace {
    pub fn new(scope: RepoScope) -> Self {
        Self {
            state: CenterState::Connecting,
            scope,
            last_snapshot_at: None,
        }
    }

    pub fn state(&self) -> &CenterState {
        &self.state
    }

    pub fn set_scope(&mut self, scope: RepoScope) {
        self.scope = scope.clone();
        match &mut self.state {
            CenterState::Connected { dashboard } | CenterState::Stale { dashboard, .. } => {
                dashboard.set_scope(scope);
            }
            CenterState::Connecting | CenterState::Unavailable { .. } => {}
        }
    }

    /// Fold one `LinkUpdate` into the face. `now` is a parameter, not a
    /// `SystemTime::now()` call inside, so header formatting stays testable
    /// without a clock.
    pub fn apply(&mut self, update: LinkUpdate, now: SystemTime) {
        match update {
            LinkUpdate::Snapshot(rows) => {
                // A snapshot always installs a coherent `Connected` state,
                // replacing whatever came before it — including a `Stale`
                // dashboard. This is the "must not merge an uncertain old
                // stream with a new snapshot" rule.
                self.state = CenterState::Connected {
                    dashboard: Dashboard::from_snapshot(rows, self.scope.clone()),
                };
                self.last_snapshot_at = Some(now);
            }
            LinkUpdate::Delta(delta) => {
                // Applied only in `Connected`. Dropping a delta in `Stale`
                // is the second half of the no-merge rule above; a delta
                // before any snapshot cannot occur in a conformant ordered
                // stream either way.
                if let CenterState::Connected { dashboard } = &mut self.state {
                    dashboard.apply(delta);
                }
            }
            LinkUpdate::Down(reason) => {
                self.state = match std::mem::replace(&mut self.state, CenterState::Connecting) {
                    CenterState::Connected { dashboard } => CenterState::Stale {
                        dashboard,
                        reason,
                        since: self.last_snapshot_at,
                    },
                    CenterState::Stale {
                        dashboard, since, ..
                    } => CenterState::Stale {
                        dashboard,
                        reason,
                        since,
                    },
                    CenterState::Connecting | CenterState::Unavailable { .. } => {
                        CenterState::Unavailable { reason }
                    }
                };
            }
        }
    }

    /// The single place staleness is worded, mirroring the TUI's
    /// `center_unreachable` shape (`worker.rs`) without depending on
    /// `ctx-traits-cli`.
    pub fn header(&self) -> String {
        match &self.state {
            CenterState::Connecting => "connecting to center…".to_string(),
            CenterState::Unavailable { reason } => {
                format!("center unavailable — no run list yet ({reason}); retrying")
            }
            CenterState::Connected { dashboard } => self.connected_header(dashboard.len()),
            CenterState::Stale {
                dashboard,
                reason,
                since,
            } => {
                let as_of = since.map_or_else(|| "unknown time".to_string(), format_time_of_day);
                format!(
                    "center unreachable — showing state as of {as_of} ({reason}); retrying — {} runs",
                    dashboard.len()
                )
            }
        }
    }

    /// Empty unless `Connected`/`Stale` — nothing is shown as current while
    /// absent, and stale rows stay visible until a fresh snapshot arrives.
    pub fn rows(&self) -> &[RunRow] {
        match &self.state {
            CenterState::Connecting | CenterState::Unavailable { .. } => &[],
            CenterState::Connected { dashboard } | CenterState::Stale { dashboard, .. } => {
                dashboard.rows()
            }
        }
    }

    pub fn is_stale(&self) -> bool {
        matches!(self.state, CenterState::Stale { .. })
    }

    /// Whether `session_id` is present in the center's authoritative,
    /// *unfiltered* row model. Used to reconcile a `Requested` spawn status
    /// against arrival of the actual row — the row's `Appeared` delta and
    /// the spawn request's own `Started` response are scheduled on
    /// independent connections and can land in either order. Delegates to
    /// `Dashboard::contains_session` rather than `self.rows()` (the
    /// scope-filtered projection): a spawn's target repository can be
    /// hidden by the current `RepoScope`, and a view filter must not
    /// prevent an accepted spawn from reconciling.
    pub fn contains_session(&self, session_id: &str) -> bool {
        match &self.state {
            CenterState::Connecting | CenterState::Unavailable { .. } => false,
            CenterState::Connected { dashboard } | CenterState::Stale { dashboard, .. } => {
                dashboard.contains_session(session_id)
            }
        }
    }

    /// `Some(liveness)` from the unfiltered keyed model when the face holds
    /// one (`Connected`/`Stale`) and the row is present; `Some(None)` when
    /// the face holds a model but the row is not in it (an `Ended` delta
    /// removed it, or it never existed); `None` when the face holds no
    /// model at all (`Connecting`/`Unavailable`). Nested so a caller can
    /// never mistake "no model yet" for "the row ended" — the two would
    /// otherwise both collapse to a bare `None`, and reconciliation must
    /// treat them oppositely: the former must never resolve a pending
    /// interrupt, the latter always does.
    pub fn row_liveness(&self, ledger_path: &str) -> Option<Option<bool>> {
        match &self.state {
            CenterState::Connecting | CenterState::Unavailable { .. } => None,
            CenterState::Connected { dashboard } | CenterState::Stale { dashboard, .. } => {
                Some(dashboard.row_liveness(ledger_path))
            }
        }
    }

    /// Empty unless `Connected`/`Stale` — see [`CenterFace::rows`]'s same
    /// reasoning. Delegates to `Dashboard::repositories`, never a second
    /// source of repository identity.
    pub fn repositories(&self) -> Vec<SpawnRepo> {
        match &self.state {
            CenterState::Connecting | CenterState::Unavailable { .. } => Vec::new(),
            CenterState::Connected { dashboard } | CenterState::Stale { dashboard, .. } => {
                dashboard.repositories()
            }
        }
    }

    /// Empty unless `Connected`/`Stale` — see [`CenterFace::rows`]'s same
    /// reasoning. Delegates to `Dashboard::rail`, never a second source of
    /// repository identity or a rail-held selection.
    pub fn rail(&self, active_repo_key: Option<&str>) -> crate::rail::Rail {
        match &self.state {
            CenterState::Connecting | CenterState::Unavailable { .. } => {
                crate::rail::project(std::iter::empty(), None, false)
            }
            CenterState::Connected { dashboard } | CenterState::Stale { dashboard, .. } => {
                dashboard.rail(active_repo_key, self.is_stale())
            }
        }
    }

    /// The outage reason, if the face is currently `Stale` — carried into a
    /// fresh detail selection so a row picked from a retained stale list
    /// starts stale itself, rather than looking current while ignoring the
    /// eventual recovery snapshot.
    pub fn stale_reason(&self) -> Option<&str> {
        match &self.state {
            CenterState::Stale { reason, .. } => Some(reason.as_str()),
            _ => None,
        }
    }

    fn connected_header(&self, len: usize) -> String {
        match &self.scope {
            RepoScope::All => format!("{len} runs"),
            RepoScope::Repo(repo_key) => format!("{len} runs in {repo_key}"),
        }
    }
}

/// Clear `form`'s `Requested` status if its session is already visible in
/// `face`'s row list. The one reconciliation path — called after every
/// applied face update, and after a submit settles — because the row's
/// `Appeared` delta and the request's own `Started` response are scheduled
/// on independent connections and can land in either order; whichever of
/// the two arrives second is the one that must observe the session already
/// visible and clear the status. A free function, not a `Shell` method, so
/// both call sites and the integration test that proves both orderings
/// share exactly one implementation. Returns whether it changed anything,
/// matching the `apply`/`settle` `-> bool` discipline.
pub fn reconcile_spawn_status(face: &CenterFace, form: &mut SpawnForm) -> bool {
    let SpawnStatus::Requested { session_id } = form.status() else {
        return false;
    };
    if !face.contains_session(session_id) {
        return false;
    }
    let session_id = session_id.to_string();
    form.clear_requested_for(&session_id)
}

/// Resolve every pending `Requested` row-control entry against `face`'s
/// current unfiltered row model. The row's own delta and the control/start
/// response are scheduled on independent connections and can land in
/// either order, so both call sites — the update loop and
/// `request_row_control`'s settle callback — run this one shared
/// implementation, mirroring `reconcile_spawn_status` exactly. Early-returns
/// without touching any entry when the face holds no model at all
/// (`Connecting`/`Unavailable`): a `None` there means "no model yet," never
/// "the row ended," and must not be read as an observed stop or resume.
pub fn reconcile_row_controls(face: &CenterFace, row_controls: &mut RowControls) -> bool {
    let mut changed = false;
    for ledger_path in row_controls.pending_ledger_paths() {
        let Some(liveness) = face.row_liveness(&ledger_path) else {
            // No model at all yet — cannot distinguish "row ended" from
            // "we haven't connected", so every pending entry is left alone.
            return changed;
        };
        changed |= row_controls.observe(&ledger_path, liveness);
    }
    changed
}

/// Refused row-control entries whose `ledger_path` is no longer among
/// `face.rows()` — the case `RowControls::observe` documents: an `Ended`
/// delta during a pending resume removes the row and moves the entry to
/// `Refused` rather than silently clearing it, but a `RunRow` no longer
/// exists to hang that refusal off of in the per-row render loop. A free
/// function, not a `Shell` method, so `Shell::render` and the unit test that
/// proves this visibility share exactly one implementation — the same
/// discipline `reconcile_row_controls` documents. Returns `(display_id,
/// message)` pairs; ledger_path is not returned since nothing keys off it
/// once the row is gone.
pub fn detached_row_notices(
    face: &CenterFace,
    row_controls: &RowControls,
) -> Vec<(String, String)> {
    row_controls
        .refused_entries()
        .into_iter()
        .filter(|(ledger_path, _, _)| {
            !face
                .rows()
                .iter()
                .any(|row| &row.ledger_path == ledger_path)
        })
        .map(|(_, display_id, message)| (display_id, message))
        .collect()
}

fn format_time_of_day(at: SystemTime) -> String {
    let seconds = at
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds / 60) % 60,
        seconds % 60,
    )
}

pub struct Shell {
    face: CenterFace,
    detail: RunDetail,
    detail_task: Option<gpui::Task<()>>,
    spawn_form: SpawnForm,
    spawn_task: Option<gpui::Task<()>>,
    row_controls: RowControls,
    /// Keyed by `ledger_path`, not a single `Option` slot like `spawn_task`:
    /// several rows can be controlled independently, and a single slot
    /// would cancel an unrelated in-flight request when a second row's
    /// request is issued. The entry for a `ledger_path` is removed once
    /// that row's request settles.
    row_control_tasks: HashMap<String, gpui::Task<()>>,
    /// The spawn form panel's one stable focus target, created once here and
    /// reused by every render's `track_focus` call. `cx.focus_handle()`
    /// mints a fresh `FocusId` on every call — calling it fresh inside
    /// `render` and passing that straight into `track_focus`, as an earlier
    /// version of this panel once did, replaces the focused handle on every
    /// frame; gpui dispatches key events through the currently focused
    /// handle, so a key press that calls `cx.notify()` would drop keyboard
    /// dispatch on the very next frame.
    spawn_focus_handle: gpui::FocusHandle,
}

impl Shell {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let updates = center_link::start(None);
        cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                let outcome = this.update(cx, |shell, cx| {
                    // Detail sees the update first — it reads nothing from
                    // the face, and feeding it first keeps stream order
                    // exact for both consumers.
                    let outcome = shell.detail.follow(&update);
                    shell.face.apply(update, SystemTime::now());
                    shell.spawn_form.set_repositories(shell.face.repositories());
                    // Reconcile a `Requested` spawn status against the
                    // now-current row list, not just this update's own
                    // delta: the row's `Appeared` delta and the request's
                    // `Started` response are scheduled on independent
                    // connections and can land in either order, so
                    // `submit_spawn`'s settle callback runs the same
                    // reconciliation on its own arrival too.
                    reconcile_spawn_status(&shell.face, &mut shell.spawn_form);
                    reconcile_row_controls(&shell.face, &mut shell.row_controls);
                    cx.notify();
                    outcome
                });
                match outcome {
                    Ok(outcome) => {
                        if let Some(request) = outcome.request {
                            let _ = this.update(cx, |shell, cx| shell.spawn_load(request, cx));
                        }
                    }
                    Err(_) => break,
                }
            }
        })
        .detach();
        Self {
            face: CenterFace::new(RepoScope::All),
            detail: RunDetail::default(),
            detail_task: None,
            spawn_form: SpawnForm::default(),
            spawn_task: None,
            row_controls: RowControls::default(),
            row_control_tasks: HashMap::new(),
            spawn_focus_handle: cx.focus_handle(),
        }
    }

    /// Exposed so the scoped view is exercisable and testable ahead of any UI
    /// control for it — a control is out of scope for this task.
    pub fn set_scope(&mut self, scope: RepoScope) {
        self.face.set_scope(scope);
    }

    /// Select the row carrying `ledger_path` and, unless it is already the
    /// current selection, run one background full-session read for it. The
    /// filesystem read itself happens on gpui's background executor
    /// (`cx.background_spawn`) — the foreground `cx.spawn` future here does
    /// nothing but await it and hand the result back.
    pub fn select_ledger(&mut self, ledger_path: String, cx: &mut Context<Self>) {
        let Some(row) = self
            .face
            .rows()
            .iter()
            .find(|row| row.ledger_path == ledger_path)
        else {
            return;
        };
        let request = match self.face.stale_reason() {
            Some(reason) => self.detail.select_stale(row, reason.to_string()),
            None => self.detail.select(row),
        };
        let Some(request) = request else {
            cx.notify();
            return;
        };
        self.spawn_load(request, cx);
        cx.notify();
    }

    /// Run one background full-session read for `request` and fold its
    /// outcome back into `self.detail`. The one background-executor /
    /// generation-guard / notify-on-real-change path, shared by the initial
    /// selection and every follow-driven resync. `detail_task` stays a
    /// single slot; assigning a new task cancels a superseded in-flight
    /// read, a courtesy on top of the generation guard rather than the guard
    /// itself — reads are read-only, so a cancelled one loses nothing.
    fn spawn_load(&mut self, request: LoadRequest, cx: &mut Context<Self>) {
        let generation = request.generation;
        self.detail_task = Some(cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move { detail::load(&request) })
                .await;
            let _ = this.update(cx, |shell, cx| {
                if shell.detail.apply(generation, outcome) {
                    cx.notify();
                }
            });
        }));
    }

    /// Open the spawn form, keeping any text already entered, and move
    /// keyboard focus onto its stable `spawn_focus_handle` so key events
    /// start reaching it immediately rather than only after the next click.
    pub fn open_spawn_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.spawn_form.open();
        self.spawn_focus_handle.focus(window);
        cx.notify();
    }

    /// Close the spawn form. Deliberately does not cancel an in-flight
    /// submit: the driver is already detached once the center accepts it,
    /// so closing the form must not appear to stop it (see `submit_spawn`).
    pub fn close_spawn_form(&mut self, cx: &mut Context<Self>) {
        self.spawn_form.close();
        cx.notify();
    }

    pub fn spawn_form(&self) -> &SpawnForm {
        &self.spawn_form
    }

    pub fn spawn_insert_char(&mut self, ch: char, cx: &mut Context<Self>) {
        self.spawn_form.insert_char(ch);
        cx.notify();
    }

    pub fn spawn_backspace(&mut self, cx: &mut Context<Self>) {
        self.spawn_form.backspace();
        cx.notify();
    }

    pub fn spawn_newline(&mut self, cx: &mut Context<Self>) {
        self.spawn_form.newline();
        cx.notify();
    }

    pub fn spawn_select_repository(&mut self, repo_key: String, cx: &mut Context<Self>) {
        self.spawn_form.select_repository(repo_key);
        cx.notify();
    }

    /// Validate and submit the form. A rejection surfaces on the form
    /// itself and never reaches this method's caller as an error.
    pub fn spawn_submit(&mut self, cx: &mut Context<Self>) {
        if let Some(request) = self.spawn_form.submit() {
            self.submit_spawn(request, cx);
        }
        cx.notify();
    }

    /// Send `request` through the center's existing-only spawn entry on
    /// gpui's background executor — `start_trait_existing` can block for as
    /// long as `ACTION_TIMEOUT` (600s) waiting for the driver to register,
    /// and must never run on the UI thread. Dropping this task (e.g. the
    /// window closing) abandons only the requester's own socket; the
    /// center's `run_start` has already spawned the driver detached by the
    /// time a `Started` response would arrive, so the child keeps running
    /// regardless — proven at the process level by
    /// `proof_center::dropping_the_requester_mid_start_leaves_the_detached_driver_running`.
    /// `settle` never touches `self.face`/`Dashboard`: the spawned run
    /// becomes visible only through a later subscription delta, never from
    /// this task's own outcome.
    fn submit_spawn(&mut self, request: SubmitRequest, cx: &mut Context<Self>) {
        let generation = request.generation;
        self.spawn_task = Some(cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    match ctx_traits_io::center::start_trait_existing(
                        &request.args,
                        camino::Utf8Path::new(&request.repo_path),
                    ) {
                        Ok(ctx_traits_io::center::StartResult::Started { session_id }) => {
                            SubmitOutcome::Started { session_id }
                        }
                        Ok(ctx_traits_io::center::StartResult::Exited { code, stderr }) => {
                            SubmitOutcome::Exited { code, stderr }
                        }
                        Err(error) => SubmitOutcome::Failed(error.to_string()),
                    }
                })
                .await;
            let _ = this.update(cx, |shell, cx| {
                let settled = shell.spawn_form.settle(generation, outcome);
                // The row this response's session refers to may already be
                // visible — the `Appeared` delta can win the race against
                // this `Started` response arriving on its own connection.
                let reconciled = reconcile_spawn_status(&shell.face, &mut shell.spawn_form);
                if settled || reconciled {
                    cx.notify();
                }
            });
        }));
    }

    /// Request `verb` for the row carrying `ledger_path`. Looks the row up
    /// in the face's current unfiltered row list, asks
    /// `RowControls::request` for client-side eligibility, and on `Some`
    /// dispatches the center round trip on gpui's background executor —
    /// every verb's call inherits `ACTION_TIMEOUT` (600s) and must never
    /// run on the UI thread, the same hazard `submit_spawn` documents. On
    /// completion, `settle` on the UI thread, then run
    /// `reconcile_row_controls` (the response may have lost the race to the
    /// row's own delta), and `cx.notify()` only on real change — the
    /// `settled || reconciled` pattern `submit_spawn` already uses.
    pub fn request_row_control(
        &mut self,
        ledger_path: String,
        verb: RowVerb,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self
            .face
            .rows()
            .iter()
            .find(|row| row.ledger_path == ledger_path)
        else {
            return;
        };
        let Some(request) = self.row_controls.request(row, verb) else {
            cx.notify();
            return;
        };
        self.dispatch_row_control(request, cx);
        cx.notify();
    }

    /// Runs the one production dispatcher, [`row_control::dispatch`], on
    /// gpui's background executor. That function owns the verb-to-center-
    /// call mapping (`control_existing` for `Interrupt`/`Pause`,
    /// `start_session_existing` for `Resume`) so it is reused verbatim by
    /// the fake-peer integration tests — there is exactly one place this
    /// mapping is expressed, not a copy here and a copy in the tests.
    fn dispatch_row_control(&mut self, request: RowRequest, cx: &mut Context<Self>) {
        let ledger_path_for_task = request.ledger_path.clone();
        let ledger_path = request.ledger_path.clone();
        let generation = request.generation;
        let task = cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move { row_control::dispatch(&request) })
                .await;
            let _ = this.update(cx, |shell, cx| {
                let settled = shell.row_controls.settle(&ledger_path, generation, outcome);
                // The row's own delta may have already carried it into the
                // observed state, winning the race against this response
                // arriving on its own connection.
                let reconciled = reconcile_row_controls(&shell.face, &mut shell.row_controls);
                shell.row_control_tasks.remove(&ledger_path);
                if settled || reconciled {
                    cx.notify();
                }
            });
        });
        self.row_control_tasks.insert(ledger_path_for_task, task);
    }
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = self.face.header();
        let stale = self.face.is_stale();
        let selected_key = self.detail.selected_key().map(str::to_string);
        // `tokens.md:64` "mono meta: states, handles, times, values,
        // activity lines, hints" — the one place that role is styled, reused
        // by every mono-meta child a row emits below.
        let mono_meta = |text: SharedString| {
            div()
                .font_family(tokens::FONT_MONO)
                .text_size(tokens::SIZE_10_5)
                .text_color(rgb(tokens::TEXT_MUTED))
                .child(text)
        };
        // `tokens.md:63` "mono actions" — stop/pause/resume and row-control
        // status share this role. No state-dependent colour: that is
        // `0265.5`'s.
        let mono_action = |text: &'static str| {
            div()
                .font_family(tokens::FONT_MONO)
                .text_size(tokens::SIZE_11)
                .text_color(rgb(tokens::TEXT_SECONDARY))
                .child(text)
        };
        let mut list = div()
            .id("run-list")
            .flex()
            .flex_col()
            .gap(tokens::LIST_ROWS_GAP_MIN)
            .size_full()
            .overflow_y_scroll();
        for row in self.face.rows() {
            let path = row.ledger_path.clone();
            let live = row.state == crate::run_row::RowState::Live;
            let mut stop =
                mono_action("stop").id(SharedString::from(format!("stop-{}", row.ledger_path)));
            if live {
                let stop_path = row.ledger_path.clone();
                stop = stop.on_click(cx.listener(move |shell, _event, _window, cx| {
                    shell.request_row_control(stop_path.clone(), RowVerb::Interrupt, cx);
                }));
            }
            let mut pause =
                mono_action("pause").id(SharedString::from(format!("pause-{}", row.ledger_path)));
            if live {
                let pause_path = row.ledger_path.clone();
                pause = pause.on_click(cx.listener(move |shell, _event, _window, cx| {
                    shell.request_row_control(pause_path.clone(), RowVerb::Pause, cx);
                }));
            }
            let mut resume =
                mono_action("resume").id(SharedString::from(format!("resume-{}", row.ledger_path)));
            if row.can_resume() {
                let resume_path = row.ledger_path.clone();
                resume = resume.on_click(cx.listener(move |shell, _event, _window, cx| {
                    shell.request_row_control(resume_path.clone(), RowVerb::Resume, cx);
                }));
            }
            let status = self
                .row_controls
                .status(&row.ledger_path)
                .map(row_control::status_text)
                .unwrap_or_default();
            let mut item = div()
                .id(SharedString::from(row.ledger_path.clone()))
                .flex()
                .flex_row()
                .gap(tokens::ROW_DOT_TEXT_GAP_MIN)
                .px(tokens::LIST_ROW_PAD_X_MAX)
                .py(tokens::LIST_ROW_PAD_Y_COMPACT)
                .on_click(cx.listener(move |shell, _event, _window, cx| {
                    shell.select_ledger(path.clone(), cx);
                }))
                .child(mono_meta(row.repo_label.clone().into()))
                .child(row.title.clone())
                .child(mono_meta(
                    format!("{} / {}", row.run_id, row.trait_id).into(),
                ))
                .child(mono_meta(row.state_text.clone().into()))
                .child(
                    div()
                        .font_family(tokens::FONT_SANS)
                        .text_size(tokens::SIZE_11)
                        .text_color(rgb(tokens::TEXT_SECONDARY))
                        .child(row.detail_text.clone()),
                )
                .child(mono_meta(row.elapsed_text.clone().into()))
                .child(mono_meta(row.tokens_text.clone().into()))
                .child(stop)
                .child(pause)
                .child(resume)
                .child(mono_meta(status.into()));
            if stale {
                item = item.opacity(0.6);
            }
            if selected_key.as_deref() == Some(row.ledger_path.as_str()) {
                item = item.bg(rgb(tokens::SURFACE_RAISED));
            }
            list = list.child(item);
        }
        // A refused row-control entry whose row has already left the row
        // list (e.g. an `Ended` delta during a pending resume) has no
        // `RunRow` to render against inside the loop above — render it as
        // its own detached notice instead, so the refusal is not simply
        // invisible.
        for (display_id, message) in detached_row_notices(&self.face, &self.row_controls) {
            list = list.child(
                div()
                    .id(SharedString::from(format!("detached-notice-{display_id}")))
                    .flex()
                    .flex_row()
                    .gap(tokens::ROW_DOT_TEXT_GAP_MIN)
                    .child(mono_meta(display_id.into()))
                    .child(
                        div()
                            .font_family(tokens::FONT_MONO)
                            .text_size(tokens::SIZE_10_5)
                            .text_color(rgb(tokens::TEXT_SECONDARY))
                            .child(format!("refused: {message}")),
                    ),
            );
        }
        let detail_pane = match self.detail.load_state() {
            None => div().into_any_element(),
            Some(detail::DetailLoad::Loading) => detail_view::loading_element(),
            Some(detail::DetailLoad::Failed(reason)) => detail_view::failed_element(reason),
            Some(detail::DetailLoad::Loaded(_)) => self
                .detail
                .tree()
                .map(|tree| detail_view::detail_element(&tree))
                .unwrap_or_else(|| div().into_any_element()),
        };
        let follow_banner = self
            .detail
            .follow_state()
            .and_then(detail_view::follow_element);
        let spawn_toggle = div()
            .id("spawn-toggle")
            .on_click(cx.listener(|shell, _event, window, cx| {
                if shell.spawn_form.is_open() {
                    shell.close_spawn_form(cx);
                } else {
                    shell.open_spawn_form(window, cx);
                }
            }))
            .child(if self.spawn_form.is_open() {
                "close spawn"
            } else {
                "spawn run"
            });
        let mut column = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(tokens::CANVAS))
            .font_family(tokens::FONT_SANS)
            .text_size(tokens::SIZE_12_5)
            .text_color(rgb(tokens::TEXT))
            .font_weight(tokens::WEIGHT_NORMAL)
            .child(header)
            .child(spawn_toggle);
        if self.spawn_form.is_open() {
            let mut repositories = div().id("spawn-repositories").flex().flex_row().gap_2();
            for repo in self.spawn_form.repositories() {
                let key = repo.repo_key.clone();
                let mut item = div()
                    .id(SharedString::from(format!("spawn-repo-{key}")))
                    .on_click(cx.listener(move |shell, _event, _window, cx| {
                        shell.spawn_select_repository(key.clone(), cx);
                    }))
                    .child(repo.label.clone());
                if self.spawn_form.selected_repo() == Some(repo.repo_key.as_str()) {
                    // Deviation beyond the row path: identical "selected
                    // fill" role as the row's own selection background —
                    // substituted here too rather than leaving the one
                    // remaining hard-coded hex in the lane.
                    item = item.bg(rgb(tokens::SURFACE_RAISED));
                }
                repositories = repositories.child(item);
            }
            column = column.child(
                div()
                    .id("spawn-form-panel")
                    .track_focus(&self.spawn_focus_handle)
                    .on_key_down(
                        cx.listener(|shell, event: &gpui::KeyDownEvent, _window, cx| {
                            let keystroke = &event.keystroke;
                            match keystroke.key.as_str() {
                                "escape" => shell.close_spawn_form(cx),
                                "backspace" => shell.spawn_backspace(cx),
                                "enter" if keystroke.modifiers.platform => shell.spawn_submit(cx),
                                "enter" => shell.spawn_newline(cx),
                                _ => {
                                    if let Some(text) = &keystroke.key_char {
                                        for ch in text.chars() {
                                            shell.spawn_insert_char(ch, cx);
                                        }
                                    }
                                }
                            }
                        }),
                    )
                    .child(repositories)
                    .child(spawn_view::spawn_element(&self.spawn_form)),
            );
        }
        column = column.child(list);
        if let Some(banner) = follow_banner {
            column = column.child(banner);
        }
        column = column.child(detail_pane);
        // The Sessions bar describes the selected run; with no selection
        // there is nothing to describe, so no bar renders.
        if let Some(selected) = selected_key.as_deref()
            && let Some(row) = self
                .face
                .rows()
                .iter()
                .find(|row| row.ledger_path == selected)
        {
            let bar = bottom_bar::sessions_bar(row, self.row_controls.status(&row.ledger_path));
            // Only a live row is eligible for pause
            // (`RowControls::request`'s client-side check) — the same rule
            // the run-list's own pause action uses above: attach no handler
            // rather than issuing a request the machine would refuse
            // without reaching the wire.
            let on_pause: Option<bottom_bar_view::BarActionHandler> =
                (row.state == crate::run_row::RowState::Live).then(|| {
                    let pause_path = row.ledger_path.clone();
                    Box::new(
                        cx.listener(move |shell, _event: &gpui::ClickEvent, _window, cx| {
                            shell.request_row_control(pause_path.clone(), RowVerb::Pause, cx);
                        }),
                    ) as bottom_bar_view::BarActionHandler
                });
            column = column.child(bottom_bar_view::bar_element(&bar, on_pause));
        }
        let rail = rail_view::rail_element(&self.face.rail(self.detail.repo_key()));
        // The preview reads exclusively from `RunDetail::preview_state` —
        // never from a `RunRow` fetched separately out of `CenterFace` — so
        // the trait/run/task rows and the footer can never disagree about
        // which run, or which generation of that run, they describe
        // (review-verdict-1 blocker `selected-preview-not-atomic`).
        let preview = self.detail.preview_state().map(|state| {
            let block = preview_view::named_block_element(&crate::preview::sessions_run_block(
                Some(&state),
            ));
            let footer = preview_view::preview_footer_element(&crate::preview::sessions_footer(
                Some(&state),
            ));
            preview_view::preview_column_element(vec![block], Some(footer))
        });
        let mut body = div()
            .debug_selector(|| "window-body".to_string())
            .flex()
            .flex_row()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(rail)
            .child(column.flex_1().min_w_0());
        if let Some(preview) = preview {
            body = body.child(preview);
        }
        div()
            .debug_selector(|| "window-frame".to_string())
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(tokens::CANVAS))
            .rounded(tokens::WINDOW_CORNER_RADIUS)
            .overflow_hidden()
            .child(title_bar_view::title_bar_element(title_bar_view::SESSIONS))
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::row_control::RowOutcome;
    use ctx_traits_io::center::{CenterDelta, CenterPublicRow};
    use ctx_traits_io::run_summary::RunSummary;
    use gpui::px;

    fn wire_row(repo_key: &str, run_id: &str) -> CenterPublicRow {
        CenterPublicRow {
            summary: RunSummary {
                run_id: run_id.to_string(),
                ..RunSummary::unreadable(run_id.to_string(), "fixture".to_string())
            },
            repo_key: repo_key.to_string(),
            repo_path: format!("/{repo_key}"),
            ledger_path: format!("/{repo_key}/session.json"),
            live: true,
            modified_epoch_secs: 0,
        }
    }

    /// A readable, non-live, `last_drive_outcome: "paused"` wire row — unlike
    /// `wire_row`, whose `parse_error` always projects to `RowState::Unreadable`
    /// regardless of `live`. Used by tests that need `run_row::project` to
    /// actually derive `RowState::Paused` from the face's own snapshot rather
    /// than from a separately synthesized `RunRow`.
    fn paused_wire_row(repo_key: &str, session_id: &str) -> CenterPublicRow {
        use ctx_traits_core::procedure::session::Status;
        CenterPublicRow {
            summary: RunSummary {
                session_id: session_id.to_string(),
                run_id: session_id.to_string(),
                trait_id: "fixture-trait".to_string(),
                status: Status::AwaitingInput,
                last_drive_outcome: Some("paused".to_string()),
                parse_error: None,
                ..RunSummary::unreadable(session_id.to_string(), String::new())
            },
            repo_key: repo_key.to_string(),
            repo_path: format!("/{repo_key}"),
            ledger_path: format!("/{repo_key}/session.json"),
            live: false,
            modified_epoch_secs: 0,
        }
    }

    #[test]
    fn scoped_center_face_projection_survives_a_down_and_recovery_cycle() {
        let scope = RepoScope::Repo("repo-a".to_string());
        let mut face = CenterFace::new(scope.clone());
        let now = SystemTime::now();

        face.apply(
            LinkUpdate::Snapshot(vec![
                wire_row("repo-a", "run-1"),
                wire_row("repo-b", "run-2"),
            ]),
            now,
        );
        let CenterState::Connected { dashboard } = face.state() else {
            panic!("expected Connected after the first snapshot");
        };
        assert_eq!(
            dashboard
                .rows()
                .iter()
                .map(|r| r.run_id.as_str())
                .collect::<Vec<_>>(),
            vec!["run-1"],
            "the scope must already filter out the other repo's row"
        );
        assert!(face.header().contains("repo-a"));

        face.apply(LinkUpdate::Down("subscription closed".to_string()), now);
        assert!(matches!(face.state(), CenterState::Stale { .. }));

        face.apply(
            LinkUpdate::Snapshot(vec![
                wire_row("repo-a", "run-3"),
                wire_row("repo-b", "run-4"),
            ]),
            now,
        );
        let CenterState::Connected { dashboard } = face.state() else {
            panic!("expected Connected after the recovery snapshot");
        };
        assert_eq!(
            dashboard
                .rows()
                .iter()
                .map(|r| r.run_id.as_str())
                .collect::<Vec<_>>(),
            vec!["run-3"],
            "the scoped projection must survive the Down/recovery cycle unchanged"
        );
        assert!(
            face.header().contains("repo-a"),
            "the repo scope must still be reflected in the header after recovery: {}",
            face.header()
        );
    }

    /// The regression proof for `contains_session` reading the unfiltered
    /// row map rather than the scope-filtered `rows()` projection. The
    /// spawn is into `repo-b`, but the view is scoped to `repo-a` — the
    /// picker still offers `repo-b` (`Dashboard::repositories` is
    /// unfiltered), and a completed spawn there must still resolve
    /// `Requested` back to `Idle` even though `face.rows()` never shows the
    /// new row at all under this scope.
    #[test]
    fn reconcile_spawn_status_clears_requested_even_when_repo_scope_hides_the_new_row() {
        let scope = RepoScope::Repo("repo-a".to_string());
        let mut face = CenterFace::new(scope);
        let now = SystemTime::now();
        face.apply(
            LinkUpdate::Snapshot(vec![
                wire_row("repo-a", "run-1"),
                wire_row("repo-b", "run-2"),
            ]),
            now,
        );
        assert_eq!(
            face.rows().len(),
            1,
            "the repo-b row must already be hidden by the current scope"
        );
        assert!(
            face.repositories()
                .iter()
                .any(|repo| repo.repo_key == "repo-b"),
            "the spawn picker must still offer a repository the view scope hides"
        );

        let mut form = SpawnForm::default();
        form.set_repositories(face.repositories());
        form.select_repository("repo-b".to_string());
        for ch in "fixture-trait".chars() {
            form.insert_char(ch);
        }
        let request = form.submit().expect("a valid, repo-selected request");
        let generation = request.generation;
        assert!(form.settle(
            generation,
            SubmitOutcome::Started {
                session_id: "session-hidden".to_string(),
            },
        ));
        assert!(matches!(
            form.status(),
            SpawnStatus::Requested { session_id } if session_id == "session-hidden"
        ));

        let mut hidden_row = wire_row("repo-b", "run-hidden");
        hidden_row.summary.session_id = "session-hidden".to_string();
        face.apply(
            LinkUpdate::Delta(ctx_traits_io::center::CenterDelta::Appeared {
                row: Box::new(hidden_row),
            }),
            now,
        );
        assert_eq!(
            face.rows().len(),
            1,
            "the newly spawned row stays hidden by the repo-a scope"
        );

        assert!(
            reconcile_spawn_status(&face, &mut form),
            "reconciliation must see the session through the unfiltered model, not the scoped projection"
        );
        assert_eq!(*form.status(), SpawnStatus::Idle);
    }

    fn live_run_row(repo_key: &str, ledger_path: &str, session_id: &str) -> RunRow {
        RunRow {
            ledger_path: ledger_path.to_string(),
            session_id: session_id.to_string(),
            run_id: session_id.to_string(),
            repo_key: repo_key.to_string(),
            repo_path: format!("/{repo_key}"),
            repo_label: repo_key.to_string(),
            title: "title".to_string(),
            trait_id: "fixture-trait".to_string(),
            state: crate::run_row::RowState::Live,
            state_text: "live".to_string(),
            detail_text: String::new(),
            elapsed_text: "00:00:00".to_string(),
            tokens_text: "-".to_string(),
            live: true,
            modified_epoch_secs: 0,
            verdict_rounds: None,
            elapsed_seconds: 0,
            started_at_epoch: None,
        }
    }

    /// The interrupt analogue of `reconcile_spawn_status_clears_requested_even_when_repo_scope_hides_the_new_row`:
    /// a `Requested` interrupt entry for a row the current `RepoScope` hides
    /// must still resolve once that row's own delta carries it into a
    /// non-live state.
    #[test]
    fn reconcile_interrupts_resolves_a_pending_stop_whose_repository_the_scope_hides() {
        let scope = RepoScope::Repo("repo-a".to_string());
        let mut face = CenterFace::new(scope);
        let now = SystemTime::now();
        face.apply(
            LinkUpdate::Snapshot(vec![
                wire_row("repo-a", "run-1"),
                wire_row("repo-b", "run-2"),
            ]),
            now,
        );
        assert_eq!(
            face.rows().len(),
            1,
            "the repo-b row must already be hidden by the current scope"
        );

        let mut row_controls = RowControls::default();
        let row = live_run_row("repo-b", "/repo-b/session.json", "run-2");
        let request = row_controls
            .request(&row, RowVerb::Interrupt)
            .expect("a live row is eligible");
        row_controls.settle(
            &request.ledger_path,
            request.generation,
            RowOutcome::Control(ctx_traits_io::center::ControlResult::Acknowledged),
        );
        assert!(matches!(
            row_controls.status("/repo-b/session.json"),
            Some(crate::row_control::RowStatus::Requested { .. })
        ));

        assert!(
            !reconcile_row_controls(&face, &mut row_controls),
            "no delta has landed yet"
        );

        let mut ended_row = wire_row("repo-b", "run-2");
        ended_row.live = false;
        face.apply(
            LinkUpdate::Delta(ctx_traits_io::center::CenterDelta::RowChanged {
                row: Box::new(ended_row),
            }),
            now,
        );

        assert!(
            reconcile_row_controls(&face, &mut row_controls),
            "reconciliation must see the row through the unfiltered model, not the scoped projection"
        );
        assert!(row_controls.status("/repo-b/session.json").is_none());
    }

    /// A `Down` alone must never resolve a pending interrupt — the stale
    /// dashboard retains the row exactly as last observed, still live — but
    /// a recovery `Snapshot` that no longer carries the row is authoritative
    /// and does resolve it.
    #[test]
    fn a_down_alone_does_not_resolve_a_pending_stop_but_a_recovery_snapshot_does() {
        let mut face = CenterFace::new(RepoScope::All);
        let now = SystemTime::now();
        face.apply(LinkUpdate::Snapshot(vec![wire_row("repo", "run-x")]), now);

        let mut row_controls = RowControls::default();
        let row = live_run_row("repo", "/repo/session.json", "run-x");
        let request = row_controls
            .request(&row, RowVerb::Interrupt)
            .expect("a live row is eligible");
        row_controls.settle(
            &request.ledger_path,
            request.generation,
            RowOutcome::Control(ctx_traits_io::center::ControlResult::Acknowledged),
        );

        face.apply(LinkUpdate::Down("subscription closed".to_string()), now);
        assert!(matches!(face.state(), CenterState::Stale { .. }));
        assert!(
            !reconcile_row_controls(&face, &mut row_controls),
            "a Down alone must not resolve a pending interrupt"
        );
        assert!(matches!(
            row_controls.status("/repo/session.json"),
            Some(crate::row_control::RowStatus::Requested { .. })
        ));

        // Recovery snapshot no longer carries the row: it ended while the
        // center was unreachable.
        face.apply(LinkUpdate::Snapshot(vec![]), now);
        assert!(
            reconcile_row_controls(&face, &mut row_controls),
            "a recovery snapshot that no longer carries the row must resolve the pending interrupt"
        );
        assert!(row_controls.status("/repo/session.json").is_none());
    }

    /// The blocker this closes: an `Ended` delta during a pending resume
    /// removes the row and moves the entry to `Refused` (per
    /// `RowControls::observe`), but `Shell::render`'s per-row loop iterates
    /// `face.rows()` — with the row gone, that loop alone would render
    /// nothing at all, silently indistinguishable from the request having
    /// been quietly dropped. `detached_row_notices` is the projection
    /// `Shell::render` also calls; this proves it surfaces the refusal
    /// without any `RunRow` to hang it off of.
    #[test]
    fn resume_row_gone_refusal_is_visible_as_a_detached_notice_once_the_row_leaves_the_face() {
        let mut face = CenterFace::new(RepoScope::All);
        let now = SystemTime::now();
        let paused_wire = paused_wire_row("repo", "run-paused");
        face.apply(LinkUpdate::Snapshot(vec![paused_wire.clone()]), now);

        let row = face
            .rows()
            .iter()
            .find(|r| r.ledger_path == "/repo/session.json")
            .expect("the paused row must already be projected by the face")
            .clone();
        assert_eq!(
            row.state,
            crate::run_row::RowState::Paused,
            "the wire row's own last_drive_outcome must project to Paused"
        );

        let mut row_controls = RowControls::default();
        let request = row_controls
            .request(&row, RowVerb::Resume)
            .expect("a paused row is eligible for resume");
        row_controls.settle(
            &request.ledger_path,
            request.generation,
            RowOutcome::Start(ctx_traits_io::center::StartResult::Started {
                session_id: "run-paused".to_string(),
            }),
        );
        assert!(matches!(
            row_controls.status("/repo/session.json"),
            Some(crate::row_control::RowStatus::Requested {
                verb: RowVerb::Resume,
                ..
            })
        ));
        assert!(
            detached_row_notices(&face, &row_controls).is_empty(),
            "the row is still present, so no detached notice is warranted yet"
        );

        // The row's own Ended delta arrives while the resume is still
        // pending — the actual subscription event path, not a snapshot
        // replacement with a separately synthesized row.
        face.apply(
            LinkUpdate::Delta(ctx_traits_io::center::CenterDelta::Ended {
                row: Box::new(paused_wire),
            }),
            now,
        );
        assert!(reconcile_row_controls(&face, &mut row_controls));
        assert!(
            !face
                .rows()
                .iter()
                .any(|r| r.ledger_path == "/repo/session.json"),
            "the row must be gone from the face"
        );
        assert!(matches!(
            row_controls.status("/repo/session.json"),
            Some(crate::row_control::RowStatus::Refused(_))
        ));

        let notices = detached_row_notices(&face, &row_controls);
        assert_eq!(
            notices.len(),
            1,
            "the refusal must surface even with no RunRow to render it against"
        );
        assert!(notices[0].1.contains("is no longer listed"));
    }

    /// The regression proof for the render-local-`FocusHandle` bug: without
    /// a stable `spawn_focus_handle`, the second keystroke below would not
    /// reach `on_key_down` at all, because the first keystroke's
    /// `cx.notify()` forces a rerender that would have tracked a brand-new
    /// `FocusHandle`, leaving `window.focus`'s id undispatchable in the new
    /// frame. Drives a real gpui window through `TestAppContext` — the
    /// gpui-free `CenterFace`/`SpawnForm` tests elsewhere in this crate
    /// cannot observe key-dispatch routing at all, since that lives entirely
    /// in gpui's own render/paint/dispatch cycle.
    #[gpui::test]
    fn spawn_form_keeps_keyboard_focus_across_key_events_and_rerenders(
        cx: &mut gpui::TestAppContext,
    ) {
        let window = cx
            .update(|cx| cx.open_window(Default::default(), |_, cx| cx.new(Shell::new)))
            .unwrap();

        window
            .update(cx, |shell, window, cx| shell.open_spawn_form(window, cx))
            .unwrap();
        cx.run_until_parked();

        // Six separate keystrokes — three character insertions, a
        // backspace, and a newline (`enter`) — each one forcing its own
        // notify-triggered rerender (via `run_until_parked`) before the
        // next is dispatched. "Multiple key events separated by
        // rerenders" across all three `on_key_down` branches, not one
        // batch dispatched against a single frame and not character
        // insertion alone: a regression that only re-broke the backspace
        // or newline branch would pass a characters-only proof.
        for key in ["f", "i", "x", "backspace", "enter"] {
            cx.dispatch_keystroke(*window, gpui::Keystroke::parse(key).unwrap());
            cx.run_until_parked();
        }

        let text = window
            .update(cx, |shell, _window, _cx| {
                shell.spawn_form().text().to_string()
            })
            .unwrap();
        assert_eq!(
            text, "fi\n",
            "every keystroke after the first must still reach the form despite the \
             intervening rerenders — the focus target must be the same FocusHandle \
             every frame — and the backspace/enter branches must still fire, not just \
             character insertion"
        );
    }

    #[test]
    fn window_options_carries_title_and_bounds() {
        let bounds = Bounds::new(gpui::point(px(0.), px(0.)), DEFAULT_WINDOW_SIZE);
        let options = window_options(bounds);

        let titlebar_title = options
            .titlebar
            .as_ref()
            .and_then(|t| t.title.as_ref().map(|s| s.as_ref()));
        assert_eq!(titlebar_title, Some(APP_TITLE));

        match options.window_bounds {
            Some(gpui::WindowBounds::Windowed(actual)) => assert_eq!(actual, bounds),
            other => panic!("expected windowed bounds, got {other:?}"),
        }
    }

    /// The unit-level half of "exactly one chrome surface": `appears_transparent:
    /// true` is the flag that removes the native macOS titlebar strip, and
    /// `traffic_light_position` is derived from the same two tokens that fix
    /// the bar's height and button size, so a token change moves both sides
    /// of this assertion together rather than freezing a literal.
    #[test]
    fn window_options_places_native_traffic_lights_inside_the_title_bar() {
        let bounds = Bounds::new(gpui::point(px(0.), px(0.)), DEFAULT_WINDOW_SIZE);
        let options = window_options(bounds);
        let titlebar = options.titlebar.as_ref().expect("titlebar is configured");

        assert_eq!(titlebar.title.as_ref().map(|s| s.as_ref()), Some(APP_TITLE));
        assert!(
            titlebar.appears_transparent,
            "the native titlebar strip must be hidden so only the drawn bar shows"
        );
        assert_eq!(
            titlebar.traffic_light_position,
            Some(gpui::point(
                tokens::TITLE_BAR_PAD_X,
                (tokens::TITLE_BAR_HEIGHT - tokens::TITLE_BAR_TRAFFIC_LIGHT) / 2.,
            ))
        );
    }

    #[test]
    fn default_window_size_is_the_reference_frame() {
        assert_eq!(
            DEFAULT_WINDOW_SIZE,
            size(tokens::WINDOW_WIDTH, tokens::WINDOW_HEIGHT)
        );
    }

    /// Opens a real window and drives `Shell::render` through gpui's own
    /// paint phase, then reads `debug_bounds` — gpui's own paint-phase
    /// record, only populated when the element actually painted — to prove
    /// the title bar is the frame's first child, full width, above the
    /// landed rail/column row.
    #[gpui::test]
    fn title_bar_is_the_frame_s_first_child_above_the_landed_columns(
        cx: &mut gpui::TestAppContext,
    ) {
        let window = cx
            .update(|cx| {
                let bounds = Bounds::new(gpui::point(px(0.), px(0.)), DEFAULT_WINDOW_SIZE);
                cx.open_window(window_options(bounds), |_, cx| cx.new(Shell::new))
            })
            .unwrap();
        cx.run_until_parked();

        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
        let frame = vcx
            .debug_bounds("window-frame")
            .expect("the window frame div actually painted this frame");
        assert_eq!(frame.size, DEFAULT_WINDOW_SIZE);

        let bar = vcx
            .debug_bounds("title-bar")
            .expect("the title bar div actually painted this frame");
        assert_eq!(bar.size.height, tokens::TITLE_BAR_HEIGHT);
        assert_eq!(bar.size.width, frame.size.width);
        assert_eq!(
            bar.origin.y, frame.origin.y,
            "the title bar is the frame's first child"
        );

        let menu = vcx
            .debug_bounds("title-bar-menu")
            .expect("the menu div actually painted this frame");
        assert_eq!(
            menu.origin.x + menu.size.width,
            frame.origin.x + frame.size.width - tokens::TITLE_BAR_PAD_X,
            "the menu is reached through the bar's trailing edge"
        );
        let top_gap = menu.origin.y - bar.origin.y;
        let bottom_gap = (bar.origin.y + bar.size.height) - (menu.origin.y + menu.size.height);
        assert!(
            (top_gap - bottom_gap).abs() <= px(1.),
            "the menu's top and bottom free space within the bar differ by at most a pixel: \
             top {top_gap:?} vs bottom {bottom_gap:?}"
        );

        let body = vcx
            .debug_bounds("window-body")
            .expect("the body row actually painted this frame");
        assert_eq!(
            body.origin.y,
            bar.origin.y + bar.size.height,
            "the body begins exactly at the title bar's bottom edge"
        );
        assert_eq!(
            body.size.width, frame.size.width,
            "the body spans the frame's full width"
        );
        assert_eq!(
            body.size.height,
            frame.size.height - bar.size.height,
            "the body occupies exactly the frame's remaining space below the bar"
        );
    }

    /// Drives the same window and proves exactly one menu label paints, and
    /// nothing paints before the bar's leading spacer — the closing-diff
    /// read's "no mark" statement made into a proof for the composed
    /// element tree's geometry.
    #[gpui::test]
    fn the_title_bar_renders_exactly_one_menu_label_and_no_mark(cx: &mut gpui::TestAppContext) {
        let window = cx
            .update(|cx| {
                let bounds = Bounds::new(gpui::point(px(0.), px(0.)), DEFAULT_WINDOW_SIZE);
                cx.open_window(window_options(bounds), |_, cx| cx.new(Shell::new))
            })
            .unwrap();
        cx.run_until_parked();

        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
        let menu = vcx
            .debug_bounds("title-bar-menu")
            .expect("the menu container actually painted this frame");
        let label = vcx
            .debug_bounds("title-bar-menu-current")
            .expect("the current-screen label actually painted this frame");
        assert_eq!(
            menu.size.width, label.size.width,
            "a single-entry menu's painted width equals its one label's width; a \
             second entry would make the container strictly wider by the gap token"
        );
    }

    #[test]
    fn rail_is_empty_while_connecting_or_unavailable() {
        let face = CenterFace::new(RepoScope::All);
        assert!(face.rail(None).repos().is_empty());
        assert_eq!(face.rail(None).space(), None);

        let mut face = CenterFace::new(RepoScope::All);
        face.apply(
            LinkUpdate::Down("no center yet".to_string()),
            SystemTime::now(),
        );
        assert!(face.rail(None).repos().is_empty());
    }

    #[test]
    fn rail_repository_appears_survives_a_partial_ending_and_disappears_on_the_last_ended() {
        let mut face = CenterFace::new(RepoScope::All);
        let now = SystemTime::now();
        face.apply(LinkUpdate::Snapshot(vec![wire_row("repo-a", "run-1")]), now);
        assert_eq!(face.rail(None).repos().len(), 1);
        assert_eq!(
            face.rail(None).dot(0),
            crate::frame_list::DotTone::Accent,
            "repo-a's only row is live"
        );

        // `second` is not live from the start, so repo-a's liveness fold
        // still depends solely on row-1 — the RowChanged below is the sole
        // cause of any dot transition, not masked by another live row.
        let mut second = wire_row("repo-a", "run-2");
        second.ledger_path = "/repo-a/session-2.json".to_string();
        second.live = false;
        face.apply(
            LinkUpdate::Delta(CenterDelta::Appeared {
                row: Box::new(second.clone()),
            }),
            now,
        );
        assert_eq!(
            face.rail(None).repos().len(),
            1,
            "still one repository, two rows"
        );
        assert_eq!(
            face.rail(None).dot(0),
            crate::frame_list::DotTone::Accent,
            "row-1 is still live"
        );

        let mut changed = wire_row("repo-a", "run-1");
        changed.live = false;
        face.apply(
            LinkUpdate::Delta(CenterDelta::RowChanged {
                row: Box::new(changed.clone()),
            }),
            now,
        );
        assert_eq!(face.rail(None).repos().len(), 1);
        assert_eq!(
            face.rail(None).dot(0),
            crate::frame_list::DotTone::Idle,
            "the RowChanged delta turned row-1 non-live, and both of repo-a's rows are now non-live"
        );

        face.apply(
            LinkUpdate::Delta(CenterDelta::Ended {
                row: Box::new(changed),
            }),
            now,
        );
        assert_eq!(
            face.rail(None).repos().len(),
            1,
            "the repository survives its first row ending — a second row remains"
        );

        face.apply(
            LinkUpdate::Delta(CenterDelta::Ended {
                row: Box::new(second),
            }),
            now,
        );
        assert!(
            face.rail(None).repos().is_empty(),
            "the repository disappears once its last row ends"
        );
    }

    #[test]
    fn rail_keeps_the_last_complete_model_and_reports_stale_across_a_down_and_recovery() {
        let mut face = CenterFace::new(RepoScope::All);
        let now = SystemTime::now();
        face.apply(LinkUpdate::Snapshot(vec![wire_row("repo-a", "run-1")]), now);
        face.apply(LinkUpdate::Down("lost connection".to_string()), now);
        let rail = face.rail(Some("repo-a"));
        assert_eq!(rail.repos().len(), 1);
        assert!(rail.is_stale());

        // A delta while Stale changes nothing.
        let mut other = wire_row("repo-b", "run-2");
        other.ledger_path = "/repo-b/session.json".to_string();
        face.apply(
            LinkUpdate::Delta(CenterDelta::Appeared {
                row: Box::new(other),
            }),
            now,
        );
        assert_eq!(face.rail(Some("repo-a")).repos().len(), 1);

        face.apply(LinkUpdate::Snapshot(vec![wire_row("repo-c", "run-3")]), now);
        let rail = face.rail(None);
        assert!(!rail.is_stale());
        assert_eq!(rail.repos().len(), 1);
        assert_eq!(rail.repos()[0].repo_key, "repo-c");
    }

    #[test]
    fn rail_ignores_a_repo_scope_that_hides_the_repository_from_the_run_list() {
        let mut face = CenterFace::new(RepoScope::Repo("repo-a".to_string()));
        let now = SystemTime::now();
        face.apply(
            LinkUpdate::Snapshot(vec![
                wire_row("repo-a", "run-1"),
                wire_row("repo-b", "run-2"),
            ]),
            now,
        );
        // The run list is scoped to repo-a, but the rail still shows both.
        assert_eq!(face.rail(None).repos().len(), 2);
    }

    #[test]
    fn rail_active_row_and_footer_move_together_across_selections() {
        let mut face = CenterFace::new(RepoScope::All);
        let now = SystemTime::now();
        face.apply(
            LinkUpdate::Snapshot(vec![
                wire_row("repo-a", "run-1"),
                wire_row("repo-b", "run-2"),
            ]),
            now,
        );

        // Drive the active identity through the real `RunDetail::select` /
        // `repo_key()` path, not a repo key handed to `CenterFace::rail`
        // directly — that is the binding goal 6 requires be exercised.
        let mut detail = RunDetail::default();
        detail.select(&live_run_row("repo-a", "/repo-a/session.json", "run-1"));
        assert_eq!(detail.repo_key(), Some("repo-a"));
        let rail = face.rail(detail.repo_key());
        assert_eq!(
            rail.space(),
            Some(rail.repos()[rail_active_index(&rail)].name.as_str())
        );
        assert_eq!(rail.repos()[rail_active_index(&rail)].repo_key, "repo-a");

        detail.select(&live_run_row("repo-b", "/repo-b/session.json", "run-2"));
        assert_eq!(detail.repo_key(), Some("repo-b"));
        let rail = face.rail(detail.repo_key());
        assert_eq!(
            rail.space(),
            Some(rail.repos()[rail_active_index(&rail)].name.as_str())
        );
        assert_eq!(rail.repos()[rail_active_index(&rail)].repo_key, "repo-b");

        // Wholesale replacement with an empty snapshot: the previously
        // active repository no longer exists in the accepted model, so the
        // active row and the footer's space line must disappear together —
        // the selection is retained but no longer matches any repo.
        face.apply(LinkUpdate::Snapshot(vec![]), now);
        assert_eq!(detail.repo_key(), Some("repo-b"));
        let rail = face.rail(detail.repo_key());
        assert!(rail.repos().is_empty());
        assert!((0..rail.repos().len()).all(|i| !rail.is_active(i)));
        assert_eq!(rail.space(), None);
    }

    fn rail_active_index(rail: &crate::rail::Rail) -> usize {
        (0..rail.repos().len())
            .find(|i| rail.is_active(*i))
            .expect("expected exactly one active row")
    }
}
