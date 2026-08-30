use std::time::SystemTime;

use gpui::prelude::*;
use gpui::{
    Bounds, Context, Pixels, Render, SharedString, Size, TitlebarOptions, Window, WindowOptions,
    div, px, size,
};

use std::collections::HashMap;

use crate::center_link::{self, LinkUpdate};
use crate::dashboard::Dashboard;
use crate::detail::{self, LoadRequest, RunDetail};
use crate::detail_view;
use crate::interrupt::{self, InterruptOutcome, InterruptRequest, Interrupts};
use crate::run_row::{RepoScope, RunRow};
use crate::spawn_form::{SpawnForm, SpawnRepo, SpawnStatus, SubmitOutcome, SubmitRequest};
use crate::spawn_view;

pub const APP_TITLE: &str = "ctx desktop";

pub const DEFAULT_WINDOW_SIZE: Size<Pixels> = size(px(960.), px(640.));

pub fn window_options(bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(APP_TITLE.into()),
            ..Default::default()
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

/// Resolve every pending `Requested` interrupt entry against `face`'s
/// current unfiltered row model. The row's own delta and the control
/// response are scheduled on independent connections and can land in
/// either order, so both call sites — the update loop and `interrupt_row`'s
/// settle callback — run this one shared implementation, mirroring
/// `reconcile_spawn_status` exactly. Early-returns without touching any
/// entry when the face holds no model at all
/// (`Connecting`/`Unavailable`): a `None` there means "no model yet," never
/// "the row ended," and must not be read as an observed stop.
pub fn reconcile_interrupts(face: &CenterFace, interrupts: &mut Interrupts) -> bool {
    let mut changed = false;
    for ledger_path in interrupts.pending_ledger_paths() {
        let Some(liveness) = face.row_liveness(&ledger_path) else {
            // No model at all yet — cannot distinguish "row ended" from
            // "we haven't connected", so every pending entry is left alone.
            return changed;
        };
        changed |= interrupts.observe(&ledger_path, liveness);
    }
    changed
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
    interrupts: Interrupts,
    /// Keyed by `ledger_path`, not a single `Option` slot like `spawn_task`:
    /// several rows can be interrupted independently, and a single slot
    /// would cancel an unrelated in-flight interrupt when a second row's
    /// request is issued. The entry for a `ledger_path` is removed once
    /// that row's request settles.
    interrupt_tasks: HashMap<String, gpui::Task<()>>,
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
                    reconcile_interrupts(&shell.face, &mut shell.interrupts);
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
            interrupts: Interrupts::default(),
            interrupt_tasks: HashMap::new(),
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

    /// Request an interrupt for the row carrying `ledger_path`. Looks the
    /// row up in the face's current unfiltered row list, asks
    /// `Interrupts::request` for client-side eligibility, and on `Some`
    /// dispatches the center round trip through `control_existing` on
    /// gpui's background executor — `control_existing` inherits
    /// `ACTION_TIMEOUT` (600s) and must never run on the UI thread, the same
    /// hazard `submit_spawn` documents. On completion, `settle` on the UI
    /// thread, then run `reconcile_interrupts` (the response may have lost
    /// the race to the row's own delta), and `cx.notify()` only on real
    /// change — the `settled || reconciled` pattern `submit_spawn` already
    /// uses.
    pub fn interrupt_row(&mut self, ledger_path: String, cx: &mut Context<Self>) {
        let Some(row) = self
            .face
            .rows()
            .iter()
            .find(|row| row.ledger_path == ledger_path)
        else {
            return;
        };
        let Some(request) = self.interrupts.request(row) else {
            cx.notify();
            return;
        };
        self.dispatch_interrupt(request, cx);
        cx.notify();
    }

    fn dispatch_interrupt(&mut self, request: InterruptRequest, cx: &mut Context<Self>) {
        let InterruptRequest {
            ledger_path,
            session_id,
            repo_key,
            generation,
        } = request;
        let ledger_path_for_task = ledger_path.clone();
        let task = cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    match ctx_traits_io::center::control_existing(
                        &session_id,
                        Some(&repo_key),
                        ctx_traits_io::center::ControlAction::Interrupt,
                    ) {
                        Ok(result) => InterruptOutcome::Result(result),
                        Err(error) => InterruptOutcome::Failed(error.to_string()),
                    }
                })
                .await;
            let _ = this.update(cx, |shell, cx| {
                let settled = shell.interrupts.settle(&ledger_path, generation, outcome);
                // The row's own delta may have already carried it into a
                // non-live state, winning the race against this response
                // arriving on its own connection.
                let reconciled = reconcile_interrupts(&shell.face, &mut shell.interrupts);
                shell.interrupt_tasks.remove(&ledger_path);
                if settled || reconciled {
                    cx.notify();
                }
            });
        });
        self.interrupt_tasks.insert(ledger_path_for_task, task);
    }
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = self.face.header();
        let stale = self.face.is_stale();
        let selected_key = self.detail.selected_key().map(str::to_string);
        let mut list = div()
            .id("run-list")
            .flex()
            .flex_col()
            .size_full()
            .overflow_y_scroll();
        for row in self.face.rows() {
            let path = row.ledger_path.clone();
            let live = row.state == crate::run_row::RowState::Live;
            let mut stop = div()
                .id(SharedString::from(format!("stop-{}", row.ledger_path)))
                .child("stop");
            if live {
                let stop_path = row.ledger_path.clone();
                stop = stop.on_click(cx.listener(move |shell, _event, _window, cx| {
                    shell.interrupt_row(stop_path.clone(), cx);
                }));
            }
            let status = self
                .interrupts
                .status(&row.ledger_path)
                .map(interrupt::status_text)
                .unwrap_or_default();
            let mut item = div()
                .id(SharedString::from(row.ledger_path.clone()))
                .flex()
                .flex_row()
                .gap_2()
                .on_click(cx.listener(move |shell, _event, _window, cx| {
                    shell.select_ledger(path.clone(), cx);
                }))
                .child(row.repo_label.clone())
                .child(row.title.clone())
                .child(format!("{} / {}", row.run_id, row.trait_id))
                .child(row.state_text.clone())
                .child(row.detail_text.clone())
                .child(row.elapsed_text.clone())
                .child(row.tokens_text.clone())
                .child(stop)
                .child(status);
            if stale {
                item = item.opacity(0.6);
            }
            if selected_key.as_deref() == Some(row.ledger_path.as_str()) {
                item = item.bg(gpui::rgb(0x333333));
            }
            list = list.child(item);
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
                    item = item.bg(gpui::rgb(0x333333));
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
        column.child(detail_pane)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_io::center::CenterPublicRow;
    use ctx_traits_io::run_summary::RunSummary;

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

        let mut interrupts = Interrupts::default();
        let row = live_run_row("repo-b", "/repo-b/session.json", "run-2");
        let request = interrupts.request(&row).expect("a live row is eligible");
        interrupts.settle(
            &request.ledger_path,
            request.generation,
            InterruptOutcome::Result(ctx_traits_io::center::ControlResult::Acknowledged),
        );
        assert!(matches!(
            interrupts.status("/repo-b/session.json"),
            Some(crate::interrupt::InterruptStatus::Requested { .. })
        ));

        assert!(
            !reconcile_interrupts(&face, &mut interrupts),
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
            reconcile_interrupts(&face, &mut interrupts),
            "reconciliation must see the row through the unfiltered model, not the scoped projection"
        );
        assert!(interrupts.status("/repo-b/session.json").is_none());
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

        let mut interrupts = Interrupts::default();
        let row = live_run_row("repo", "/repo/session.json", "run-x");
        let request = interrupts.request(&row).expect("a live row is eligible");
        interrupts.settle(
            &request.ledger_path,
            request.generation,
            InterruptOutcome::Result(ctx_traits_io::center::ControlResult::Acknowledged),
        );

        face.apply(LinkUpdate::Down("subscription closed".to_string()), now);
        assert!(matches!(face.state(), CenterState::Stale { .. }));
        assert!(
            !reconcile_interrupts(&face, &mut interrupts),
            "a Down alone must not resolve a pending interrupt"
        );
        assert!(matches!(
            interrupts.status("/repo/session.json"),
            Some(crate::interrupt::InterruptStatus::Requested { .. })
        ));

        // Recovery snapshot no longer carries the row: it ended while the
        // center was unreachable.
        face.apply(LinkUpdate::Snapshot(vec![]), now);
        assert!(
            reconcile_interrupts(&face, &mut interrupts),
            "a recovery snapshot that no longer carries the row must resolve the pending interrupt"
        );
        assert!(interrupts.status("/repo/session.json").is_none());
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
}
