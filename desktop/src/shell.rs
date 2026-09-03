use std::time::SystemTime;

use gpui::prelude::*;
use gpui::{
    Bounds, Context, Pixels, Render, SharedString, Size, TitlebarOptions, Window, WindowOptions,
    div, rgb, size,
};

use std::collections::HashMap;

use crate::board::{self, BoardState, NewTaskEntry};
use crate::bottom_bar;
use crate::bottom_bar_view;
use crate::center_link::{self, LinkUpdate};
use crate::config_preview::{self, SeatIdentity};
use crate::config_screen::{self, ConfigState};
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
use crate::trait_library::{self, LibraryState};
use crate::trait_preview;
use ctx_traits_io::library::{
    LibraryDetailResolution, LibraryDetailSelector, LibraryPresentationFace, LibraryRow,
};

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
            LinkUpdate::Board { .. } => {}
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

/// Repository identity is current only while the face has an accepted,
/// connected snapshot. Stale rows remain visible elsewhere but cannot title a
/// screen as if they were current.
fn connected_merge_scope<'a>(
    face: &'a CenterFace,
    selected_repo_key: Option<&str>,
) -> Option<(&'a str, &'a str)> {
    let CenterState::Connected { dashboard } = face.state() else {
        return None;
    };
    let rows = dashboard.rows();
    let repo_key = selected_repo_key.or_else(|| rows.first().map(|row| row.repo_key.as_str()))?;
    rows.iter()
        .find(|row| row.repo_key == repo_key)
        .map(|row| (row.repo_key.as_str(), row.repo_path.as_str()))
}

pub struct Shell {
    screen: Screen,
    face: CenterFace,
    detail: RunDetail,
    detail_task: Option<gpui::Task<()>>,
    board: BoardState,
    board_repo: Option<(String, String)>,
    board_task: Option<gpui::Task<()>>,
    board_generation: u64,
    board_stale_reason: Option<String>,
    /// Changes only when the active board is replaced, not when that board
    /// receives a newer answer.
    board_scope_generation: u64,
    new_task_entry: NewTaskEntry,
    create_task_task: Option<gpui::Task<()>>,
    create_task_generation: u64,
    selected_task: Option<String>,
    selected_merge: usize,
    task_detail: crate::task_preview::TaskDetailState,
    task_detail_task: Option<gpui::Task<()>>,
    task_detail_generation: u64,
    library: LibraryState,
    library_task: Option<gpui::Task<()>>,
    library_generation: u64,
    config: ConfigState,
    config_task: Option<gpui::Task<()>>,
    config_generation: u64,
    config_seat_selection: Option<SeatIdentity>,
    trait_selection: Option<LibraryDetailSelector>,
    trait_detail: Option<Result<LibraryDetailResolution, String>>,
    trait_detail_task: Option<gpui::Task<()>>,
    trait_detail_generation: u64,
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
    new_task_focus_handle: gpui::FocusHandle,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Sessions,
    Tasks,
    Traits,
    Merges,
    Config,
}

impl Screen {
    fn title(self) -> &'static str {
        match self {
            Self::Sessions => title_bar_view::SESSIONS,
            Self::Tasks => title_bar_view::TASKS,
            Self::Traits => title_bar_view::TRAITS,
            Self::Merges => title_bar_view::MERGES,
            Self::Config => title_bar_view::CONFIG,
        }
    }
}

impl Shell {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self::new_inner(cx, true)
    }

    #[doc(hidden)]
    pub fn new_for_test(cx: &mut Context<Self>) -> Self {
        Self::new_inner(cx, false)
    }

    fn new_inner(cx: &mut Context<Self>, connect_center: bool) -> Self {
        if connect_center {
            let updates = center_link::start(None);
            cx.spawn(async move |this, cx| {
                while let Ok(update) = updates.recv().await {
                    let outcome = this.update(cx, |shell, cx| {
                        shell.apply_link_update(update, SystemTime::now(), cx)
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
        }
        Self {
            screen: Screen::Sessions,
            face: CenterFace::new(RepoScope::All),
            detail: RunDetail::default(),
            detail_task: None,
            board: BoardState::Failed("select a repository to view its tasks".to_string()),
            board_repo: None,
            board_task: None,
            board_generation: 0,
            board_stale_reason: None,
            board_scope_generation: 0,
            new_task_entry: NewTaskEntry::default(),
            create_task_task: None,
            create_task_generation: 0,
            selected_task: None,
            selected_merge: crate::merges::INITIAL_SELECTION,
            task_detail: crate::task_preview::TaskDetailState::Loading,
            task_detail_task: None,
            task_detail_generation: 0,
            library: LibraryState::Failed("select a repository to view its traits".to_string()),
            library_task: None,
            library_generation: 0,
            config: ConfigState::Failed("select a repository to view its config".to_string()),
            config_task: None,
            config_generation: 0,
            config_seat_selection: None,
            trait_selection: None,
            trait_detail: None,
            trait_detail_task: None,
            trait_detail_generation: 0,
            spawn_form: SpawnForm::default(),
            spawn_task: None,
            row_controls: RowControls::default(),
            row_control_tasks: HashMap::new(),
            spawn_focus_handle: cx.focus_handle(),
            new_task_focus_handle: cx.focus_handle(),
        }
    }

    pub fn switch_screen(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.screen = screen;
        if screen == Screen::Tasks {
            self.load_board(cx);
        }
        if screen == Screen::Traits {
            self.load_library(cx);
        }
        if screen == Screen::Config {
            self.load_config(cx);
        }
        cx.notify();
    }

    fn select_merge(&mut self, index: usize, cx: &mut Context<Self>) {
        let sections = crate::merges::merge_sections();
        self.selected_merge = index.min(crate::merges::row_count(&sections).saturating_sub(1));
        cx.notify();
    }

    /// Ask the existing center for the selected repository's served board.
    /// The UI supplies neither a path nor a filesystem fallback.
    fn load_board(&mut self, cx: &mut Context<Self>) {
        self.new_task_entry.deactivated();
        self.board_scope_generation += 1;
        let repo_key = self
            .detail
            .repo_key()
            .map(str::to_owned)
            .or_else(|| self.face.rows().first().map(|row| row.repo_key.clone()));
        let Some(repo_key) = repo_key else {
            self.board = BoardState::Failed("select a repository to view its tasks".to_string());
            self.board_repo = None;
            return;
        };
        let repo_path = self
            .face
            .rows()
            .iter()
            .find(|row| row.repo_key == repo_key)
            .map(|row| row.repo_path.clone());
        let Some(repo_path) = repo_path else {
            self.board = BoardState::Failed("repository path is unavailable".to_string());
            self.board_repo = None;
            return;
        };
        self.board_generation += 1;
        let generation = self.board_generation;
        self.board = BoardState::Loading;
        self.board_repo = Some((repo_key.clone(), repo_path));
        self.board_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    ctx_traits_io::center::board_existing(&repo_key)
                        .map_err(|error| error.to_string())
                })
                .await;
            let _ = this.update(cx, |shell, cx| {
                if shell.settle_board_load(generation, result) {
                    cx.notify();
                }
            });
        }));
    }

    fn settle_board_load(
        &mut self,
        generation: u64,
        result: Result<ctx_traits_io::center::BoardWireResult, String>,
    ) -> bool {
        if self.board_generation != generation {
            return false;
        }
        self.board = match result {
            // A one-shot answer can still populate the board while its
            // subscription is stale; the stale marker describes that link.
            Ok(answer) => self.accepted_board(answer),
            Err(reason) => BoardState::Failed(reason),
        };
        true
    }

    fn accepted_board(&mut self, answer: ctx_traits_io::center::BoardWireResult) -> BoardState {
        if self
            .selected_task
            .as_ref()
            .is_some_and(|key| !matches!(answer.sections.get(key), Some(Some(_))))
        {
            self.selected_task = None;
            self.task_detail_generation += 1;
            self.task_detail = crate::task_preview::TaskDetailState::Loading;
        }
        BoardState::Accepted {
            answer,
            stale: self.board_stale_reason.clone(),
        }
    }

    fn accept_board_update(
        &mut self,
        repo_key: String,
        answer: ctx_traits_io::center::BoardWireResult,
    ) {
        if self.board_repo.as_ref().map(|(key, _)| key) != Some(&repo_key) {
            return;
        }
        let previous = match &self.board {
            BoardState::Accepted { answer, .. } => Some(answer.clone()),
            BoardState::Loading | BoardState::Failed(_) => None,
        };
        self.board_generation += 1;
        self.board_stale_reason = None;
        self.new_task_entry
            .board_accepted(previous.as_ref(), &answer);
        self.board = self.accepted_board(answer);
    }

    /// Apply one subscription update to every surface that follows the center.
    /// Keeping this reducer shared with the pump prevents lifecycle tests from
    /// bypassing the production `LinkUpdate` routing.
    fn apply_link_update(
        &mut self,
        update: LinkUpdate,
        now: SystemTime,
        cx: &mut Context<Self>,
    ) -> detail::FollowOutcome {
        // Detail sees the update first — it reads nothing from the face, and
        // feeding it first keeps stream order exact for both consumers.
        let outcome = self.detail.follow(&update);
        let board_update = match &update {
            LinkUpdate::Board { repo_key, board } => Some((repo_key.clone(), (**board).clone())),
            _ => None,
        };
        let subscription_down = match &update {
            LinkUpdate::Down(reason) => Some(reason.clone()),
            _ => None,
        };
        let library_changed = matches!(
            &update,
            LinkUpdate::Delta(ctx_traits_io::center::CenterDelta::LibraryChanged { repo_keys })
                if matches!(&self.library, LibraryState::Accepted { answer, .. } if repo_keys.contains(&answer.repo_key))
        );
        let config_changed = matches!(
            &update,
            LinkUpdate::Delta(ctx_traits_io::center::CenterDelta::ConfigChanged { repo_keys })
                if matches!(&self.config, ConfigState::Accepted { answer, .. } if repo_keys.contains(&answer.repo_key))
        );
        self.face.apply(update, now);
        if let Some((repo_key, board)) = board_update {
            self.accept_board_update(repo_key, board);
        }
        if let Some(reason) = subscription_down {
            self.new_task_entry.subscription_down();
            self.mark_board_stale(reason);
        }
        self.spawn_form.set_repositories(self.face.repositories());
        reconcile_spawn_status(&self.face, &mut self.spawn_form);
        reconcile_row_controls(&self.face, &mut self.row_controls);
        if library_changed {
            self.load_library(cx);
            if let Some(selector) = self.trait_selection.clone() {
                self.request_trait_detail(selector, cx);
            }
        }
        if config_changed {
            self.load_config(cx);
        }
        cx.notify();
        outcome
    }

    fn mark_board_stale(&mut self, reason: String) {
        self.board_stale_reason = Some(reason.clone());
        if let BoardState::Accepted { stale, .. } = &mut self.board {
            *stale = Some(reason);
        }
    }

    /// Ask the existing center for the selected repository's served library.
    /// The UI supplies neither a path nor a filesystem fallback.
    fn load_library(&mut self, cx: &mut Context<Self>) {
        let repo_key = self
            .detail
            .repo_key()
            .map(str::to_owned)
            .or_else(|| self.face.rows().first().map(|row| row.repo_key.clone()));
        let Some(repo_key) = repo_key else {
            self.library = trait_library::fold_library_result(
                &self.library,
                Err("select a repository to view its traits".to_string()),
            );
            return;
        };
        self.library_generation += 1;
        let generation = self.library_generation;
        if !matches!(self.library, LibraryState::Accepted { .. }) {
            self.library = LibraryState::Loading;
        }
        self.library_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    ctx_traits_io::center::library_existing(&repo_key)
                        .map_err(|error| error.to_string())
                })
                .await;
            let _ = this.update(cx, |shell, cx| {
                if shell.library_generation == generation {
                    shell.library = trait_library::fold_library_result(&shell.library, result);
                    cx.notify();
                }
            });
        }));
    }

    fn load_config(&mut self, cx: &mut Context<Self>) {
        let repo_path = self
            .detail
            .repo_key()
            .and_then(|key| {
                self.face
                    .rows()
                    .iter()
                    .find(|row| row.repo_key == key)
                    .map(|row| row.repo_path.clone())
            })
            .or_else(|| self.face.rows().first().map(|row| row.repo_path.clone()));
        let Some(repo_path) = repo_path else {
            self.config = config_screen::fold_config_result(
                &self.config,
                Err("select a repository to view its config".to_string()),
            );
            return;
        };
        self.config_generation += 1;
        let generation = self.config_generation;
        if !matches!(self.config, ConfigState::Accepted { .. }) {
            self.config = ConfigState::Loading;
        }
        self.config_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    ctx_traits_io::center::config_existing(camino::Utf8Path::new(&repo_path))
                        .map_err(|error| error.to_string())
                })
                .await;
            let _ = this.update(cx, |shell, cx| {
                if shell.config_generation == generation {
                    shell.config = config_screen::fold_config_result(&shell.config, result);
                    shell.reconcile_config_selection();
                    cx.notify();
                }
            });
        }));
    }

    fn reconcile_config_selection(&mut self) {
        let Some(selection) = &self.config_seat_selection else {
            return;
        };
        let present = matches!(&self.config, ConfigState::Accepted { answer, .. } if matches!(&answer.resolution, ctx_traits_io::config_view::ConfigResolution::Resolved(view) if view.seats.iter().any(|seat| seat.role == selection.role && seat.seat_index == selection.seat_index)));
        if !present {
            self.config_seat_selection = None;
        }
    }

    fn select_config_seat(&mut self, selection: SeatIdentity, cx: &mut Context<Self>) {
        if self.config_seat_selection.as_ref() != Some(&selection) {
            self.config_seat_selection = Some(selection);
            cx.notify();
        }
    }

    fn select_trait(&mut self, selector: LibraryDetailSelector, cx: &mut Context<Self>) {
        if self.trait_selection.as_ref() == Some(&selector) {
            return;
        }
        self.trait_selection = Some(selector.clone());
        self.request_trait_detail(selector, cx);
    }

    fn request_trait_detail(&mut self, selector: LibraryDetailSelector, cx: &mut Context<Self>) {
        self.trait_detail = None;
        self.trait_detail_generation += 1;
        let generation = self.trait_detail_generation;
        let repo_key = match &self.library {
            LibraryState::Accepted { answer, .. } => answer.repo_key.clone(),
            LibraryState::Loading | LibraryState::Failed(_) => return,
        };
        self.trait_detail_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    ctx_traits_io::center::library_detail_existing(&repo_key, selector)
                        .map_err(|error| error.to_string())
                })
                .await;
            let _ = this.update(cx, |shell, cx| {
                if shell.trait_detail_generation == generation {
                    shell.trait_detail = Some(result);
                    cx.notify();
                }
            });
        }));
    }

    fn select_task(&mut self, task_key: String, cx: &mut Context<Self>) {
        if self.selected_task.as_deref() == Some(task_key.as_str()) {
            return;
        }
        let Some((repo_key, _)) = &self.board_repo else {
            return;
        };
        self.selected_task = Some(task_key.clone());
        self.task_detail_generation += 1;
        let generation = self.task_detail_generation;
        let repo_key = repo_key.clone();
        let request_key = task_key.clone();
        self.task_detail = crate::task_preview::TaskDetailState::Loading;
        self.task_detail_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    ctx_traits_io::center::task_detail_existing(&repo_key, &request_key)
                        .map_err(|error| error.to_string())
                })
                .await;
            let _ = this.update(cx, |shell, cx| {
                if shell.task_detail_generation == generation
                    && shell.selected_task.as_deref() == Some(task_key.as_str())
                {
                    shell.task_detail = match result {
                        Ok(answer) => crate::task_preview::TaskDetailState::Accepted {
                            answer,
                            stale: None,
                        },
                        Err(reason) => crate::task_preview::TaskDetailState::Failed(reason),
                    };
                    cx.notify();
                }
            });
        }));
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

    fn activate_new_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new_task_entry.activate();
        self.new_task_focus_handle.focus(window);
        cx.notify();
    }

    fn new_task_key(&mut self, event: &gpui::KeyDownEvent, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.new_task_entry.cancel(),
            "backspace" => self.new_task_entry.backspace(),
            "enter" => self.submit_new_task(cx),
            _ => {
                if let Some(text) = &event.keystroke.key_char {
                    for ch in text.chars() {
                        self.new_task_entry.insert_char(ch);
                    }
                }
            }
        }
        cx.notify();
    }

    fn submit_new_task(&mut self, cx: &mut Context<Self>) {
        let Some((repo_key, _)) = &self.board_repo else {
            return;
        };
        self.create_task_generation += 1;
        let Some(request) = self.new_task_entry.submit(
            repo_key.clone(),
            self.create_task_generation,
            self.board_scope_generation,
        ) else {
            return;
        };
        let generation = request.generation;
        let board_scope_generation = request.board_scope_generation;
        let request_repo = request.repo_key.clone();
        self.create_task_task = Some(cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    let (sender, receiver) = async_channel::bounded(1);
                    std::thread::spawn(move || {
                        let _ = sender.send_blocking(board::dispatch_create(request));
                    });
                    receiver
                        .recv()
                        .await
                        .expect("create worker must return its outcome")
                })
                .await;
            let _ = this.update(cx, |shell, cx| {
                if shell.board_repo.as_ref().map(|(key, _)| key) == Some(&request_repo)
                    && shell.board_scope_generation == board_scope_generation
                    && shell.new_task_entry.settle(
                        &request_repo,
                        generation,
                        board_scope_generation,
                        outcome,
                    )
                {
                    cx.notify();
                }
            });
        }));
    }

    #[doc(hidden)]
    pub fn set_board_for_test(
        &mut self,
        repo_key: String,
        repo_path: String,
        answer: ctx_traits_io::center::BoardWireResult,
        cx: &mut Context<Self>,
    ) {
        // Keep test-driven repository changes subject to the same pending
        // create teardown as `load_board`'s production switch path.
        self.new_task_entry.deactivated();
        self.board_scope_generation += 1;
        self.screen = Screen::Tasks;
        self.board_repo = Some((repo_key, repo_path));
        self.board = self.accepted_board(answer);
        cx.notify();
    }

    #[doc(hidden)]
    pub fn apply_link_update_for_test(&mut self, update: LinkUpdate, cx: &mut Context<Self>) {
        self.apply_link_update(update, SystemTime::now(), cx);
    }

    #[doc(hidden)]
    pub fn new_task_action_for_test(&self) -> crate::bottom_bar::BarAction {
        self.new_task_entry.action()
    }

    #[doc(hidden)]
    pub fn new_task_awaits_result_for_test(&self) -> bool {
        self.new_task_entry.awaits_result()
    }

    #[doc(hidden)]
    pub fn board_answer_for_test(&self) -> Option<ctx_traits_io::center::BoardWireResult> {
        match &self.board {
            BoardState::Accepted { answer, .. } => Some(answer.clone()),
            BoardState::Loading | BoardState::Failed(_) => None,
        }
    }

    #[doc(hidden)]
    pub fn board_has_task_for_test(&self, title: &str) -> bool {
        matches!(
            &self.board,
            BoardState::Accepted { answer, .. }
                if answer.resolution.rows.iter().any(|row| row.summary.title == title)
        )
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
        // Taken once per render, per 0265.14 §3.4: one derived `frame N of
        // M` value handed to both the header and the bottom bar, so the
        // "one derivation, two renders" property is directly observable in
        // a diff read rather than two independent format calls drifting.
        let preview_state = self.detail.preview_state();
        let frame_counter = preview_state.as_ref().and_then(|state| match state {
            crate::detail::PreviewState::Accepted { baseline, .. } => {
                crate::preview::frame_counter_text(&baseline.progress)
            }
            crate::detail::PreviewState::Loading | crate::detail::PreviewState::Failed(_) => None,
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
            .child({
                let screen_header = crate::screen_header::sessions_header(
                    preview_state.as_ref(),
                    frame_counter.as_deref(),
                );
                crate::screen_header_view::screen_header_element(
                    &screen_header.title,
                    &screen_header.summary,
                )
            })
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
            let bar = bottom_bar::sessions_bar(
                row,
                self.row_controls.status(&row.ledger_path),
                frame_counter.as_deref(),
            );
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
            column = column.child(bottom_bar_view::bar_element(&bar, on_pause, None));
        }
        let rail = rail_view::rail_element(&self.face.rail(self.detail.repo_key()));
        // The preview reads exclusively from `RunDetail::preview_state` —
        // never from a `RunRow` fetched separately out of `CenterFace` — so
        // the trait/run/task rows and the footer can never disagree about
        // which run, or which generation of that run, they describe
        // (review-verdict-1 blocker `selected-preview-not-atomic`).
        let preview = self.detail.preview_state().map(|state| {
            let mut body = vec![preview_view::named_block_element(
                "run",
                &crate::preview::sessions_run_block(Some(&state)),
            )];
            if let Some(now) = crate::preview::sessions_now_item(Some(&state)) {
                body.push(preview_view::now_item_element(&now));
            }
            if let Some(verdict) = crate::preview::sessions_verdict_block(Some(&state)) {
                body.push(preview_view::verdict_block_element(&verdict));
            }
            if let Some(slots) = crate::preview::sessions_slots_block(Some(&state)) {
                body.push(preview_view::named_block_element("slots", &slots));
            }
            if let Some(landing) = crate::preview::sessions_landing_block(Some(&state)) {
                body.push(preview_view::landing_block_element(&landing));
            }
            let footer = preview_view::preview_footer_element(&crate::preview::sessions_footer(
                Some(&state),
            ));
            preview_view::preview_column_element(body, Some(footer))
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
        if self.screen == Screen::Tasks {
            let (title, summary) = match (&self.board, &self.board_repo) {
                (BoardState::Accepted { answer, .. }, Some((repo_key, repo_path))) => {
                    let header = board::tasks_header(answer, repo_key, repo_path);
                    (header.title, header.summary)
                }
                (BoardState::Loading, _) => ("tasks".to_string(), "loading board".to_string()),
                (BoardState::Failed(reason), _) => {
                    ("tasks unavailable".to_string(), reason.clone())
                }
                (BoardState::Accepted { .. }, None) => (
                    "tasks unavailable".to_string(),
                    "repository scope is unavailable".to_string(),
                ),
            };
            let mut tasks = div()
                .debug_selector(|| "tasks-screen".to_string())
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .p(tokens::MAIN_PANE_PAD_TOP)
                .gap(tokens::MAIN_PANE_GAP)
                .child(crate::screen_header_view::screen_header_element(
                    &title, &summary,
                ));
            if let BoardState::Accepted { answer, .. } = &self.board {
                for (section, label) in [
                    (
                        ctx_traits_core::task::provider::BoardSection::InProgress,
                        "In progress",
                    ),
                    (
                        ctx_traits_core::task::provider::BoardSection::Ready,
                        "Ready",
                    ),
                    (
                        ctx_traits_core::task::provider::BoardSection::Draft,
                        "Draft",
                    ),
                ] {
                    let rows: Vec<_> = answer
                        .resolution
                        .rows
                        .iter()
                        .filter(|row| {
                            answer.sections.get(&row.summary.key).copied().flatten()
                                == Some(section)
                        })
                        .collect();
                    let group = div()
                        .debug_selector(|| format!("tasks-section-{section:?}"))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .debug_selector(|| format!("tasks-section-heading-{section:?}"))
                                .debug_selector({
                                    let heading = format!("{label} {}", rows.len());
                                    move || format!("tasks-section-heading-value-{heading}")
                                })
                                .font_family(tokens::FONT_MONO)
                                .text_size(tokens::SIZE_11)
                                .text_color(rgb(tokens::TEXT_MUTED))
                                .child(format!("{label} {}", rows.len())),
                        );
                    let mut rows_element = div().flex().flex_col().gap(tokens::LIST_ROWS_GAP_MIN);
                    for row in rows {
                        let task_key = row.summary.key.clone();
                        let click_task_key = task_key.clone();
                        let joined = answer
                            .joined_runs
                            .get(&row.summary.key)
                            .map(Vec::as_slice)
                            .unwrap_or(&[]);
                        let stack =
                            board::task_row_stack(&row.summary, joined, &row.unmet_dependencies);
                        let mut stack_element = div()
                            .flex()
                            .flex_col()
                            .flex_none()
                            .items_end()
                            .gap(tokens::LIST_ROWS_GAP_MIN)
                            .font_family(tokens::FONT_MONO)
                            .text_size(tokens::SIZE_10_5)
                            .child(
                                div()
                                    .debug_selector({
                                        let word = stack.word.clone();
                                        move || format!("task-status-{word}")
                                    })
                                    .text_color(rgb(crate::run_row::task_stack_word_color(
                                        stack.word_role,
                                    )))
                                    .child(stack.word),
                            );
                        if let Some(meta) = stack.meta {
                            stack_element = stack_element
                                .child(div().text_color(rgb(tokens::TEXT_MUTED)).child(meta));
                        }
                        rows_element = rows_element.child(
                            div()
                                .id(SharedString::from(format!("task-{task_key}")))
                                .debug_selector({
                                    let task_key = task_key.clone();
                                    move || format!("tasks-section-{section:?}-row-{task_key}")
                                })
                                .on_click(cx.listener(
                                    move |shell, _event: &gpui::ClickEvent, _window, cx| {
                                        shell.select_task(click_task_key.clone(), cx);
                                    },
                                ))
                                .flex()
                                .flex_row()
                                .justify_between()
                                .items_start()
                                .py(tokens::LIST_ROW_PAD_Y_OPEN)
                                .px(tokens::LIST_ROW_PAD_X_MAX)
                                .gap(tokens::ROW_DOT_TEXT_GAP_MAX)
                                .child(
                                    div().pt(tokens::FRAME_DOT_WRAP_PAD_TOP).child(
                                        div()
                                            .w(tokens::LIST_ROW_DOT_SIZE)
                                            .h(tokens::LIST_ROW_DOT_SIZE)
                                            .rounded_full()
                                            .bg(rgb(stack.dot_color)),
                                    ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_1()
                                        .flex_col()
                                        .gap(tokens::FRAME_ROW_TEXT_GAP)
                                        .child(
                                            div()
                                                .debug_selector({
                                                    let task_key = task_key.clone();
                                                    move || format!("task-title-{task_key}")
                                                })
                                                .debug_selector({
                                                    let title = row.summary.title.clone();
                                                    move || format!("task-title-value-{title}")
                                                })
                                                .font_family(tokens::FONT_SANS)
                                                .text_size(tokens::SIZE_12_5)
                                                .text_color(rgb(tokens::TEXT))
                                                .child(row.summary.title.clone()),
                                        )
                                        .child(
                                            div()
                                                .debug_selector({
                                                    let task_key = task_key.clone();
                                                    move || format!("task-description-{task_key}")
                                                })
                                                .debug_selector({
                                                    let description = row.short_description.clone();
                                                    move || {
                                                        format!(
                                                            "task-description-value-{description}"
                                                        )
                                                    }
                                                })
                                                .font_family(tokens::FONT_SANS)
                                                .text_size(tokens::SIZE_11)
                                                .text_color(rgb(tokens::TEXT_SECONDARY))
                                                .child(row.short_description.clone()),
                                        ),
                                )
                                .child(stack_element),
                        );
                    }
                    tasks = tasks.child(group.gap(tokens::LIST_SECTION_GAP).child(rows_element));
                }
            }
            let on_new_task: bottom_bar_view::BarActionHandler =
                Box::new(cx.listener(|shell, _event: &gpui::ClickEvent, window, cx| {
                    shell.activate_new_task(window, cx);
                }));
            tasks = tasks.child(
                div()
                    .id("new-task-entry")
                    .track_focus(&self.new_task_focus_handle)
                    .on_key_down(
                        cx.listener(|shell, event: &gpui::KeyDownEvent, _window, cx| {
                            shell.new_task_key(event, cx);
                        }),
                    )
                    .child(bottom_bar_view::bar_element(
                        &board::tasks_bar(&self.board, &self.new_task_entry),
                        None,
                        Some(on_new_task),
                    )),
            );
            body = div()
                .debug_selector(|| "tasks-screen".to_string())
                .flex()
                .flex_1()
                .min_h_0()
                .child(rail_view::rail_element(
                    &self.face.rail(self.detail.repo_key()),
                ))
                .child(tasks);
            if self.selected_task.is_some() {
                let lede = crate::task_preview::tasks_lede(&self.task_detail);
                let details = crate::task_preview::tasks_details_block(&self.task_detail);
                let facts = crate::task_preview::tasks_facts_block(&self.task_detail);
                let checks = crate::task_preview::tasks_checks_block(&self.task_detail);
                let landing = crate::task_preview::tasks_landing_block(&self.task_detail);
                body = body.child(preview_view::preview_column_element(
                    vec![
                        preview_view::lede_element(&lede),
                        preview_view::named_block_element("details", &details),
                        preview_view::named_block_element("facts", &facts),
                        preview_view::checks_block_element(&checks),
                        preview_view::landing_block_element(&landing),
                    ],
                    Some(preview_view::preview_footer_element(&board::tasks_footer(
                        &self.board,
                    ))),
                ));
            }
        }
        if self.screen == Screen::Merges {
            let sections = crate::merges::merge_sections();
            let merge_scope = connected_merge_scope(&self.face, self.detail.repo_key());
            let header = crate::merges::merges_header(merge_scope);
            let merge_block = crate::merges::merges_merge_block();
            let gates_block = crate::merges::merges_gates_block();
            let signoffs_block = crate::merges::merges_signoffs_block();
            let landing_block = crate::merges::merges_landing_block();
            let preview_body = vec![
                preview_view::lede_block_element(
                    "merge",
                    &merge_block,
                    Some(crate::placeholders::MERGES_MERGE.prose),
                ),
                preview_view::named_block_element("gates", &gates_block),
                preview_view::signoff_block_element(&signoffs_block),
                preview_view::landing_block_element(&landing_block),
            ];
            let merges = div()
                .debug_selector(|| "merges-screen".to_string())
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .pt(tokens::MAIN_PANE_PAD_TOP)
                .pr(tokens::MAIN_PANE_PAD_RIGHT)
                .pb(tokens::MAIN_PANE_PAD_BOTTOM)
                .pl(tokens::MAIN_PANE_PAD_LEFT)
                .gap(tokens::MAIN_PANE_GAP)
                .child(crate::screen_header_view::screen_header_element(
                    &header.title,
                    &header.summary,
                ))
                .child(crate::merges_view::merges_pane_element(
                    &sections,
                    self.selected_merge,
                    |index| {
                        Some(Box::new(cx.listener(
                            move |shell, _event: &gpui::ClickEvent, _window, cx| {
                                shell.select_merge(index, cx);
                            },
                        ))
                            as crate::merges_view::SelectHandler)
                    },
                ))
                .child(bottom_bar_view::bar_element(
                    &crate::merges::merges_bar(),
                    None,
                    None,
                ));
            body = div()
                .flex()
                .flex_1()
                .min_h_0()
                .child(rail_view::rail_element(
                    &self.face.rail(self.detail.repo_key()),
                ))
                .child(merges)
                .child(preview_view::overflow_preview_column_element(
                    preview_body,
                    preview_view::preview_footer_element(crate::placeholders::MERGES_FOOTER),
                ));
        }
        if matches!(self.screen, Screen::Traits | Screen::Config) {
            let header = trait_library::traits_header(&self.library);
            let authored_count = match &self.library {
                LibraryState::Accepted { answer, .. } => {
                    trait_library::authored_row_count(&answer.resolution)
                }
                LibraryState::Loading | LibraryState::Failed(_) => 0,
            };
            let mut traits = div()
                .debug_selector(|| "traits-screen".to_string())
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .p(tokens::MAIN_PANE_PAD_TOP)
                .gap(tokens::MAIN_PANE_GAP)
                .child(crate::screen_header_view::screen_header_element(
                    &header.title,
                    &header.summary,
                ))
                .child(
                    div()
                        .font_family(tokens::FONT_MONO)
                        .text_size(tokens::SIZE_11)
                        .text_color(rgb(tokens::TEXT_MUTED))
                        .child(format!("Authored — {authored_count}")),
                );
            if let LibraryState::Accepted { answer, .. } = &self.library {
                for row in &answer.resolution.rows {
                    let (label, selector, presentation) = match row {
                        LibraryRow::Resolved(member) => (
                            member
                                .family_key
                                .clone()
                                .unwrap_or_else(|| member.id.clone()),
                            LibraryDetailSelector {
                                trait_id: member.id.clone(),
                                canonical_digest: Some(member.canonical_digest.clone()),
                                member: member.family_key.clone(),
                            },
                            member
                                .trust_state
                                .presentation(LibraryPresentationFace::Desktop),
                        ),
                        LibraryRow::SourceOnly { id, .. } => (
                            id.clone(),
                            LibraryDetailSelector {
                                trait_id: id.clone(),
                                canonical_digest: None,
                                member: None,
                            },
                            ctx_traits_io::library::LibraryTrustState::SourceOnly
                                .presentation(LibraryPresentationFace::Desktop),
                        ),
                        LibraryRow::Unreadable { id, .. } => (
                            id.clone(),
                            LibraryDetailSelector {
                                trait_id: id.clone(),
                                canonical_digest: None,
                                member: None,
                            },
                            ctx_traits_io::library::LibraryTrustState::Unreadable
                                .presentation(LibraryPresentationFace::Desktop),
                        ),
                    };
                    let selected = self.trait_selection.as_ref() == Some(&selector);
                    traits = traits.child(
                        div()
                            .id(SharedString::from(format!("trait-row-{label}")))
                            .w_full()
                            .flex()
                            .flex_row()
                            .justify_between()
                            .items_center()
                            .px(tokens::RAIL_ROW_PAD_X)
                            .py(tokens::RAIL_ROW_PAD_Y)
                            .bg(rgb(if selected {
                                tokens::SURFACE_RAISED
                            } else {
                                tokens::ROW_OPEN
                            }))
                            .child(
                                div()
                                    .font_family(tokens::FONT_SANS)
                                    .text_size(tokens::SIZE_12)
                                    .text_color(rgb(if selected {
                                        tokens::TEXT_BRIGHT
                                    } else {
                                        tokens::TEXT
                                    }))
                                    .child(label),
                            )
                            .child(
                                div()
                                    .font_family(tokens::FONT_MONO)
                                    .text_size(tokens::SIZE_10_5)
                                    .text_color(rgb(crate::run_row::role_color(match presentation
                                        .role
                                    {
                                        ctx_traits_io::library::LibraryTrustRole::SettledGood => {
                                            crate::run_row::StateRole::Ok
                                        }
                                        ctx_traits_io::library::LibraryTrustRole::Danger => {
                                            crate::run_row::StateRole::Danger
                                        }
                                        ctx_traits_io::library::LibraryTrustRole::Warn => {
                                            crate::run_row::StateRole::Warn
                                        }
                                        ctx_traits_io::library::LibraryTrustRole::Neutral => {
                                            crate::run_row::StateRole::Neutral
                                        }
                                    })))
                                    .child(presentation.word),
                            )
                            .on_click(cx.listener(move |shell, _event, _window, cx| {
                                shell.select_trait(selector.clone(), cx)
                            })),
                    );
                }
            }
            if self.screen == Screen::Config {
                let header = config_screen::config_header(&self.config);
                let mut config = div()
                    .debug_selector(|| "config-screen".to_string())
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .pl(tokens::MAIN_PANE_PAD_LEFT)
                    .pr(tokens::MAIN_PANE_PAD_RIGHT)
                    .py(tokens::MAIN_PANE_PAD_TOP)
                    .gap(tokens::MAIN_PANE_GAP)
                    .child(crate::screen_header_view::screen_header_element(
                        &header.title,
                        &header.summary,
                    ));
                match &self.config {
                    ConfigState::Loading => {
                        config = config.child(
                            div()
                                .font_family(tokens::FONT_SANS)
                                .text_size(tokens::SIZE_12)
                                .text_color(rgb(tokens::TEXT_SECONDARY))
                                .child("loading config"),
                        );
                    }
                    ConfigState::Failed(reason) => {
                        config = config.child(
                            div()
                                .font_family(tokens::FONT_SANS)
                                .text_size(tokens::SIZE_12)
                                .text_color(rgb(tokens::TEXT_SECONDARY))
                                .child(reason.clone()),
                        );
                    }
                    ConfigState::Accepted { answer, stale } => match &answer.resolution {
                        ctx_traits_io::config_view::ConfigResolution::Resolved(view) => {
                            for (heading, rows) in [
                                (
                                    "Agents",
                                    view.seats
                                        .iter()
                                        .map(|seat| {
                                            (
                                                seat.role.clone(),
                                                seat.model
                                                    .clone()
                                                    .unwrap_or_else(|| "unconfigured".to_string()),
                                                seat.reasoning_effort.clone().unwrap_or_default(),
                                                Some(SeatIdentity {
                                                    role: seat.role.clone(),
                                                    seat_index: seat.seat_index,
                                                }),
                                            )
                                        })
                                        .collect::<Vec<_>>(),
                                ),
                                (
                                    "Runtime",
                                    view.runtime
                                        .iter()
                                        .map(|row| {
                                            (
                                                row.name.clone(),
                                                row.value.clone(),
                                                row.qualifier.clone(),
                                                None,
                                            )
                                        })
                                        .collect::<Vec<_>>(),
                                ),
                                (
                                    "Trust",
                                    vec![(
                                        "approved traits".to_string(),
                                        view.trust.approved_digests.to_string(),
                                        view.trust.approved_members.join(", "),
                                        None,
                                    )],
                                ),
                            ] {
                                config = config.child(
                                    div()
                                        .font_family(tokens::FONT_MONO)
                                        .text_size(tokens::SIZE_11)
                                        .text_color(rgb(tokens::TEXT_MUTED))
                                        .child(heading),
                                );
                                for (name, value, qualifier, seat) in rows {
                                    let selected =
                                        self.config_seat_selection.as_ref() == seat.as_ref();
                                    let mut row = div()
                                        .id(SharedString::from(format!("config-row-{name}")))
                                        .flex()
                                        .flex_row()
                                        .justify_between()
                                        .px(tokens::RAIL_ROW_PAD_X)
                                        .py(tokens::RAIL_ROW_PAD_Y)
                                        .bg(rgb(if selected {
                                            tokens::SURFACE_RAISED
                                        } else {
                                            tokens::ROW_OPEN
                                        }))
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .font_family(tokens::FONT_SANS)
                                                        .text_size(tokens::SIZE_12)
                                                        .text_color(rgb(if selected {
                                                            tokens::TEXT_BRIGHT
                                                        } else {
                                                            tokens::TEXT
                                                        }))
                                                        .child(name),
                                                )
                                                .child(
                                                    div()
                                                        .font_family(tokens::FONT_SANS)
                                                        .text_size(tokens::SIZE_11)
                                                        .text_color(rgb(tokens::TEXT_SECONDARY))
                                                        .child(qualifier),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .font_family(tokens::FONT_MONO)
                                                .text_size(tokens::SIZE_10_5)
                                                .text_color(rgb(tokens::TEXT_SECONDARY))
                                                .child(value),
                                        );
                                    if let Some(seat) = seat {
                                        row = row.on_click(cx.listener(
                                            move |shell, _event, _window, cx| {
                                                shell.select_config_seat(seat.clone(), cx)
                                            },
                                        ));
                                    }
                                    config = config.child(row);
                                }
                            }
                            let _ = stale;
                        }
                        ctx_traits_io::config_view::ConfigResolution::Refused { reason }
                        | ctx_traits_io::config_view::ConfigResolution::Failed { reason } => {
                            config = config.child(
                                div()
                                    .font_family(tokens::FONT_SANS)
                                    .text_size(tokens::SIZE_12)
                                    .text_color(rgb(tokens::TEXT_SECONDARY))
                                    .child(reason.clone()),
                            );
                        }
                    },
                }
                config = config
                    .child(div().flex_1())
                    .child(bottom_bar_view::bar_element(
                        &config_screen::config_bar(&self.config),
                        None,
                        None,
                    ));
                body = div()
                    .debug_selector(|| "config-screen".to_string())
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(rail_view::rail_element(
                        &self.face.rail(self.detail.repo_key()),
                    ))
                    .child(config)
                    .when_some(
                        config_preview::project(&self.config, self.config_seat_selection.as_ref()),
                        |body, preview| {
                            body.child(preview_view::preview_column_element(
                                vec![
                                    preview_view::lede_block_element(
                                        "seat",
                                        &preview.seat_block,
                                        preview.prose,
                                    ),
                                    preview_view::named_block_element(
                                        "facts",
                                        &preview.facts_block,
                                    ),
                                ],
                                Some(preview_view::preview_footer_element(&preview.footer)),
                            ))
                        },
                    );
            }
            if self.screen == Screen::Traits {
                let traits = traits
                    .child(div().flex_1())
                    .child(bottom_bar_view::bar_element(
                        &trait_library::traits_bar(&self.library),
                        None,
                        None,
                    ));
                body = div()
                    .debug_selector(|| "traits-screen".to_string())
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(rail_view::rail_element(
                        &self.face.rail(self.detail.repo_key()),
                    ))
                    .child(traits)
                    .child({
                        let preview = trait_preview::project(self.trait_detail.as_ref());
                        preview_view::preview_column_element(
                            vec![
                                preview_view::lede_block_element(
                                    "trait",
                                    &preview.trait_block,
                                    preview.lede.as_deref(),
                                ),
                                preview_view::named_block_element("facts", &preview.facts_block),
                                preview_view::named_block_element(
                                    "variants",
                                    &preview.variants_block,
                                ),
                                preview_view::named_block_element("ports", &preview.ports_block),
                            ],
                            Some(preview_view::preview_footer_element(
                                &trait_library::traits_footer_text(&self.library),
                            )),
                        )
                    });
            }
        }
        div()
            .debug_selector(|| "window-frame".to_string())
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(tokens::CANVAS))
            .rounded(tokens::WINDOW_CORNER_RADIUS)
            .overflow_hidden()
            .child(title_bar_view::title_bar_element(
                self.screen.title(),
                (self.screen != Screen::Sessions).then(|| {
                    Box::new(
                        cx.listener(|shell, _event: &gpui::ClickEvent, _window, cx| {
                            shell.switch_screen(Screen::Sessions, cx);
                        }),
                    ) as title_bar_view::MenuHandler
                }),
                (self.screen != Screen::Tasks).then(|| {
                    Box::new(
                        cx.listener(|shell, _event: &gpui::ClickEvent, _window, cx| {
                            shell.switch_screen(Screen::Tasks, cx);
                        }),
                    ) as title_bar_view::MenuHandler
                }),
                (self.screen != Screen::Traits).then(|| {
                    Box::new(
                        cx.listener(|shell, _event: &gpui::ClickEvent, _window, cx| {
                            shell.switch_screen(Screen::Traits, cx);
                        }),
                    ) as title_bar_view::MenuHandler
                }),
                (self.screen != Screen::Merges).then(|| {
                    Box::new(
                        cx.listener(|shell, _event: &gpui::ClickEvent, _window, cx| {
                            shell.switch_screen(Screen::Merges, cx);
                        }),
                    ) as title_bar_view::MenuHandler
                }),
                (self.screen != Screen::Config).then(|| {
                    Box::new(
                        cx.listener(|shell, _event: &gpui::ClickEvent, _window, cx| {
                            shell.switch_screen(Screen::Config, cx);
                        }),
                    ) as title_bar_view::MenuHandler
                }),
            ))
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::row_control::RowOutcome;
    use ctx_traits_core::task::TaskStatus;
    use ctx_traits_core::task::graph::DerivedStatus;
    use ctx_traits_core::task::provider::{BoardSection, TaskSummary};
    use ctx_traits_io::center::{BoardWireResult, CenterDelta, CenterPublicRow};
    use ctx_traits_io::run_summary::RunSummary;
    use ctx_traits_io::task_files::{BoardPresence, BoardResolution, BoardRow};
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

    fn empty_board() -> BoardWireResult {
        BoardWireResult {
            resolution: BoardResolution {
                presence: BoardPresence::Empty,
                digest: Some("sha256:fixture".to_string()),
                rows: Vec::new(),
                sync_report: ctx_traits_core::task::provider::SyncReport::default(),
                resolved_at: 0,
            },
            joined_runs: Default::default(),
            sections: Default::default(),
        }
    }

    fn board_with_task(key: &str, title: &str) -> BoardWireResult {
        let mut board = empty_board();
        board.resolution.presence = BoardPresence::Loaded;
        board.resolution.rows.push(BoardRow {
            summary: TaskSummary {
                key: key.to_string(),
                title: title.to_string(),
                stored_status: Some(TaskStatus::Draft),
                derived_status: DerivedStatus::Draft,
                archived: false,
            },
            relations: Default::default(),
            unmet_dependencies: Vec::new(),
            digest: "sha256:task".to_string(),
            short_description: String::new(),
        });
        board
            .sections
            .insert(key.to_string(), Some(BoardSection::Draft));
        board
    }

    #[gpui::test]
    fn board_selection_follows_open_sections_across_resection_and_closes(
        _cx: &mut gpui::TestAppContext,
    ) {
        let shell = _cx.update(|cx| cx.new(Shell::new));
        shell.update(_cx, |shell, _cx| {
            shell.board_repo = Some(("repo".to_string(), "/repo".to_string()));
            let mut initial = board_with_task("selected", "Selected");
            initial
                .sections
                .insert("selected".to_string(), Some(BoardSection::Ready));
            shell.board = shell.accepted_board(initial);
            shell.select_task("selected".to_string(), _cx);
            assert_eq!(shell.selected_task.as_deref(), Some("selected"));

            let mut resectioned = board_with_task("other", "Other");
            resectioned.resolution.rows.push(
                board_with_task("selected", "Selected")
                    .resolution
                    .rows
                    .remove(0),
            );
            resectioned
                .sections
                .insert("selected".to_string(), Some(BoardSection::InProgress));
            shell.board = shell.accepted_board(resectioned);
            assert_eq!(
                shell.selected_task.as_deref(),
                Some("selected"),
                "a retained task remains selected when its open section changes"
            );

            let mut closed = board_with_task("selected", "Selected");
            closed.sections.insert("selected".to_string(), None);
            shell.board = shell.accepted_board(closed);
            assert!(
                shell.selected_task.is_none(),
                "rows retained for board history cannot retain an open-pane selection"
            );
        });
    }

    #[gpui::test]
    fn tasks_navigation_returns_to_the_preserved_sessions_state(cx: &mut gpui::TestAppContext) {
        let window = cx
            .update(|cx| {
                let bounds = Bounds::new(gpui::point(px(0.), px(0.)), DEFAULT_WINDOW_SIZE);
                cx.open_window(window_options(bounds), |_, cx| cx.new(Shell::new))
            })
            .unwrap();
        window
            .update(cx, |shell, _window, cx| {
                shell
                    .detail
                    .select(&live_run_row("repo", "/repo/session.json", "session-1"));
                assert!(matches!(shell.screen, Screen::Sessions));
                assert_eq!(shell.detail.selected_key(), Some("/repo/session.json"));
                shell.switch_screen(Screen::Tasks, cx);
            })
            .unwrap();
        cx.run_until_parked();
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
        assert!(vcx.debug_bounds("title-bar-menu-current-Tasks").is_some());
        for selector in [
            "title-bar-menu-current-Sessions",
            "title-bar-menu-current-Traits",
            "title-bar-menu-current-Merges",
            "title-bar-menu-current-Config",
        ] {
            assert!(vcx.debug_bounds(selector).is_none());
        }
        drop(vcx);
        window
            .update(cx, |shell, _window, cx| {
                shell.switch_screen(Screen::Sessions, cx)
            })
            .unwrap();
        cx.run_until_parked();
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
        assert!(
            vcx.debug_bounds("title-bar-menu-current-Sessions")
                .is_some()
        );
        for selector in [
            "title-bar-menu-current-Tasks",
            "title-bar-menu-current-Traits",
            "title-bar-menu-current-Merges",
            "title-bar-menu-current-Config",
        ] {
            assert!(vcx.debug_bounds(selector).is_none());
        }
        drop(vcx);
        window
            .update(cx, |shell, _window, _cx| {
                assert!(matches!(shell.screen, Screen::Sessions));
                assert_eq!(shell.detail.selected_key(), Some("/repo/session.json"));
            })
            .unwrap();
    }

    #[gpui::test]
    fn tasks_screen_renders_answer_content_and_partitions_open_rows(cx: &mut gpui::TestAppContext) {
        let window = cx
            .update(|cx| {
                let bounds = Bounds::new(gpui::point(px(0.), px(0.)), DEFAULT_WINDOW_SIZE);
                cx.open_window(window_options(bounds), |_, cx| cx.new(Shell::new))
            })
            .unwrap();
        window
            .update(cx, |shell, _window, cx| {
                let mut answer = board_with_task("ready-a", "Answer ready title");
                answer.resolution.rows[0].short_description =
                    "Answer ready description".to_string();
                answer
                    .sections
                    .insert("ready-a".to_string(), Some(BoardSection::Ready));
                for (key, title, description, section) in [
                    (
                        "ready-b",
                        "Second ready title",
                        "Second ready description",
                        BoardSection::Ready,
                    ),
                    (
                        "progress",
                        "Answer progress title",
                        "Answer progress description",
                        BoardSection::InProgress,
                    ),
                    (
                        "draft",
                        "Answer draft title",
                        "Answer draft description",
                        BoardSection::Draft,
                    ),
                ] {
                    let mut row = board_with_task(key, title).resolution.rows.remove(0);
                    row.short_description = description.to_string();
                    answer.resolution.rows.push(row);
                    answer.sections.insert(key.to_string(), Some(section));
                }
                let mut closed = board_with_task("closed", "Closed must not render")
                    .resolution
                    .rows
                    .remove(0);
                closed.summary.stored_status = Some(TaskStatus::Done);
                closed.summary.derived_status = DerivedStatus::Done;
                answer.resolution.rows.push(closed);
                answer.sections.insert("closed".to_string(), None);
                let mut archived = board_with_task("archived", "Archived must not render")
                    .resolution
                    .rows
                    .remove(0);
                archived.summary.archived = true;
                answer.resolution.rows.push(archived);
                answer.sections.insert("archived".to_string(), None);
                shell.set_board_for_test("repo".to_string(), "/repo".to_string(), answer, cx);
            })
            .unwrap();
        cx.run_until_parked();
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
        for selector in [
            "tasks-section-InProgress",
            "tasks-section-Ready",
            "tasks-section-Draft",
            "tasks-section-heading-value-In progress 1",
            "tasks-section-heading-value-Ready 2",
            "tasks-section-heading-value-Draft 1",
            "tasks-section-Ready-row-ready-a",
            "task-title-value-Answer ready title",
            "task-description-value-Answer ready description",
            "tasks-section-Ready-row-ready-b",
            "task-title-value-Second ready title",
            "task-description-value-Second ready description",
            "tasks-section-InProgress-row-progress",
            "task-title-value-Answer progress title",
            "task-description-value-Answer progress description",
            "tasks-section-Draft-row-draft",
            "task-title-value-Answer draft title",
            "task-description-value-Answer draft description",
        ] {
            assert!(
                vcx.debug_bounds(selector).is_some(),
                "{selector} paints from the answer"
            );
        }
        // Titles are section-independent selectors, so these exclusions cover
        // every section rather than only the sections listed below.
        assert!(vcx.debug_bounds("task-title-closed").is_none());
        assert!(vcx.debug_bounds("task-title-archived").is_none());
        for selector in [
            "tasks-section-InProgress-row-ready-a",
            "tasks-section-Draft-row-ready-a",
            "tasks-section-InProgress-row-ready-b",
            "tasks-section-Draft-row-ready-b",
            "tasks-section-Ready-row-progress",
            "tasks-section-Draft-row-progress",
            "tasks-section-InProgress-row-draft",
            "tasks-section-Ready-row-draft",
            "tasks-section-Ready-row-closed",
            "tasks-section-Draft-row-archived",
        ] {
            assert!(
                vcx.debug_bounds(selector).is_none(),
                "{selector} must not paint outside its served section"
            );
        }
        let heading = vcx
            .debug_bounds("tasks-section-heading-value-Ready 2")
            .unwrap();
        let first = vcx
            .debug_bounds("tasks-section-Ready-row-ready-a")
            .unwrap();
        let second = vcx
            .debug_bounds("tasks-section-Ready-row-ready-b")
            .unwrap();
        assert_eq!(
            first.origin.y - (heading.origin.y + heading.size.height),
            tokens::LIST_SECTION_GAP + px(0.5),
            "the painted text box rounds the 12px section token by half a pixel"
        );
        assert_eq!(
            second.origin.y - (first.origin.y + first.size.height),
            tokens::LIST_ROWS_GAP_MIN
        );
        assert_eq!(first.origin.x, second.origin.x);
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
            session_title: None,
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

    #[gpui::test]
    fn new_task_entry_keeps_keyboard_focus_and_escape_restores_the_action(
        cx: &mut gpui::TestAppContext,
    ) {
        let shell = cx.update(|cx| cx.new(Shell::new));
        let window = cx
            .update(|cx| {
                let shell = shell.clone();
                cx.open_window(Default::default(), move |_, _| shell)
            })
            .unwrap();
        shell.update(cx, |shell, cx| {
            shell.screen = Screen::Tasks;
            cx.notify();
        });
        cx.run_until_parked();
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        let before = visual
            .debug_bounds("bottom-bar")
            .expect("the Tasks bar paints before entry activation");
        assert!(
            visual.debug_bounds("bottom-bar-action-NewTask").is_some(),
            "the rendered Tasks bar contains the NewTask action slot"
        );
        let action_bounds = visual
            .debug_bounds("bottom-bar-action-NewTask")
            .expect("the NewTask action paints before activation");
        visual.simulate_click(
            gpui::point(
                action_bounds.origin.x + action_bounds.size.width / 2.,
                action_bounds.origin.y + action_bounds.size.height / 2.,
            ),
            gpui::Modifiers::default(),
        );
        visual.run_until_parked();

        let active = visual
            .debug_bounds("bottom-bar")
            .expect("the Tasks bar remains painted while the entry is active");
        assert_eq!(
            before, active,
            "opening the in-bar entry changes no bar geometry"
        );

        for key in ["a", "b", "backspace"] {
            cx.dispatch_keystroke(*window, gpui::Keystroke::parse(key).unwrap());
            cx.run_until_parked();
        }
        let editing_label = window
            .update(cx, |shell, _window, _cx| {
                shell.new_task_entry.action().label
            })
            .unwrap();
        assert_eq!(editing_label, "new task: a");

        cx.dispatch_keystroke(*window, gpui::Keystroke::parse("escape").unwrap());
        cx.run_until_parked();
        let action = window
            .update(cx, |shell, _window, _cx| shell.new_task_entry.action())
            .unwrap();
        assert_eq!(action.label, "new task");
        assert_eq!(action.tone, crate::bottom_bar::ActionTone::Primary);
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        let after = visual
            .debug_bounds("bottom-bar")
            .expect("the Tasks bar remains painted after cancellation");
        assert_eq!(before, after, "entry activation changes no bar geometry");
    }

    #[gpui::test]
    fn terminal_board_events_free_the_new_task_guard_without_accepting_late_results(
        cx: &mut gpui::TestAppContext,
    ) {
        let shell = cx.update(|cx| cx.new(Shell::new));
        shell.update(cx, |shell, cx| {
            shell.board_repo = Some(("repo-a".to_string(), "/repo-a".to_string()));
            shell.board = shell.accepted_board(empty_board());

            shell.new_task_entry.activate();
            shell.new_task_entry.insert_char('a');
            let first = shell
                .new_task_entry
                .submit("repo-a".to_string(), 1, shell.board_scope_generation)
                .expect("first request");
            shell.apply_link_update(
                LinkUpdate::Board {
                    repo_key: "repo-a".to_string(),
                    board: Box::new(empty_board()),
                },
                SystemTime::UNIX_EPOCH,
                cx,
            );
            shell.new_task_entry.activate();
            shell.new_task_entry.insert_char('b');
            let second = shell
                .new_task_entry
                .submit("repo-a".to_string(), 2, shell.board_scope_generation)
                .expect("board acceptance must free the guard");
            assert!(!shell.new_task_entry.settle(
                &first.repo_key,
                first.generation,
                first.board_scope_generation,
                board::CreateOutcome::Failed("late first result".to_string()),
            ));
            assert!(shell.new_task_entry.settle(
                &second.repo_key,
                second.generation,
                second.board_scope_generation,
                board::CreateOutcome::Failed("second result".to_string()),
            ));

            shell.new_task_entry.activate();
            shell.new_task_entry.insert_char('c');
            let third = shell
                .new_task_entry
                .submit("repo-a".to_string(), 3, shell.board_scope_generation)
                .expect("third request");
            shell.apply_link_update(
                LinkUpdate::Down("connection closed".to_string()),
                SystemTime::UNIX_EPOCH,
                cx,
            );
            shell.new_task_entry.activate();
            shell.new_task_entry.insert_char('d');
            let fourth = shell
                .new_task_entry
                .submit("repo-a".to_string(), 4, shell.board_scope_generation)
                .expect("subscription loss must free the guard");
            assert!(!shell.new_task_entry.settle(
                &third.repo_key,
                third.generation,
                third.board_scope_generation,
                board::CreateOutcome::Failed("late third result".to_string()),
            ));
            assert!(shell.new_task_entry.settle(
                &fourth.repo_key,
                fourth.generation,
                fourth.board_scope_generation,
                board::CreateOutcome::Failed("fourth result".to_string()),
            ));
        });
    }

    #[gpui::test]
    fn new_task_reply_does_not_change_the_board_but_its_board_changed_does(
        cx: &mut gpui::TestAppContext,
    ) {
        let shell = cx.update(|cx| cx.new(Shell::new));
        shell.update(cx, |shell, cx| {
            shell.board_repo = Some(("repo-a".to_string(), "/repo-a".to_string()));
            shell.board = shell.accepted_board(empty_board());
            shell.new_task_entry.activate();
            for ch in "owner title".chars() {
                shell.new_task_entry.insert_char(ch);
            }
            let request = shell
                .new_task_entry
                .submit("repo-a".to_string(), 1, shell.board_scope_generation)
                .expect("the owner title creates one request");
            let created = TaskSummary {
                key: "draft-key".to_string(),
                title: "owner title".to_string(),
                stored_status: None,
                derived_status: DerivedStatus::Ready,
                archived: false,
            };
            assert!(shell.new_task_entry.settle(
                &request.repo_key,
                request.generation,
                request.board_scope_generation,
                board::CreateOutcome::Result(ctx_traits_io::center::CreateTaskWireResult::Created(
                    created,
                )),
            ));
            assert!(matches!(
                &shell.board,
                BoardState::Accepted { answer, .. } if answer.resolution.rows.is_empty()
            ));

            shell.apply_link_update(
                LinkUpdate::Board {
                    repo_key: "repo-a".to_string(),
                    board: Box::new(board_with_task("draft-key", "owner title")),
                },
                SystemTime::UNIX_EPOCH,
                cx,
            );
            assert!(matches!(
                &shell.board,
                BoardState::Accepted { answer, .. }
                    if answer.resolution.rows.len() == 1
                        && answer.resolution.rows[0].summary.title == "owner title"
                        && answer.sections.get("draft-key") == Some(&Some(BoardSection::Draft))
            ));
        });
    }

    #[gpui::test]
    fn board_subscription_loss_stays_visible_until_a_matching_board_update(
        cx: &mut gpui::TestAppContext,
    ) {
        let shell = cx.update(|cx| cx.new(Shell::new));
        shell.update(cx, |shell, cx| {
            shell.board_repo = Some(("repo-a".to_string(), "/repo-a".to_string()));
            shell.board = shell.accepted_board(empty_board());
            shell.apply_link_update(
                LinkUpdate::Down("connection closed".to_string()),
                SystemTime::UNIX_EPOCH,
                cx,
            );
            assert_eq!(
                board::tasks_bar(&shell.board, &shell.new_task_entry)
                    .state
                    .word,
                "unavailable"
            );
            assert_eq!(
                board::tasks_footer(&shell.board),
                "stale: connection closed"
            );

            shell.apply_link_update(
                LinkUpdate::Board {
                    repo_key: "repo-b".to_string(),
                    board: Box::new(empty_board()),
                },
                SystemTime::UNIX_EPOCH,
                cx,
            );
            assert_eq!(
                board::tasks_bar(&shell.board, &shell.new_task_entry)
                    .state
                    .word,
                "unavailable",
                "an unrelated board answer must not make repo-a current"
            );

            shell.apply_link_update(
                LinkUpdate::Board {
                    repo_key: "repo-a".to_string(),
                    board: Box::new(empty_board()),
                },
                SystemTime::UNIX_EPOCH,
                cx,
            );
            assert_eq!(
                board::tasks_bar(&shell.board, &shell.new_task_entry)
                    .state
                    .word,
                "synced"
            );
            assert!(board::tasks_footer(&shell.board).contains("synced"));
        });
    }

    #[gpui::test]
    fn subscription_loss_does_not_invalidate_an_in_flight_board_load(
        cx: &mut gpui::TestAppContext,
    ) {
        let shell = cx.update(|cx| cx.new(Shell::new));
        shell.update(cx, |shell, cx| {
            shell.board_repo = Some(("repo-a".to_string(), "/repo-a".to_string()));
            shell.board_generation = 7;
            shell.board = BoardState::Loading;

            shell.apply_link_update(
                LinkUpdate::Down("connection closed".to_string()),
                SystemTime::UNIX_EPOCH,
                cx,
            );
            assert_eq!(
                shell.board_generation, 7,
                "a subscription outage must not reject an already-issued board request"
            );
            assert!(shell.settle_board_load(7, Ok(empty_board())));
            assert!(matches!(
                &shell.board,
                BoardState::Accepted {
                    stale: Some(reason),
                    ..
                } if reason == "connection closed"
            ));
        });
    }

    #[gpui::test]
    fn recovery_snapshot_does_not_replace_the_stale_active_board_or_lose_late_refusal_correlation(
        cx: &mut gpui::TestAppContext,
    ) {
        let shell = cx.update(|cx| cx.new(Shell::new));
        shell.update(cx, |shell, cx| {
            shell.board_repo = Some(("repo-a".to_string(), "/repo-a".to_string()));
            shell.board = shell.accepted_board(empty_board());
            shell.apply_link_update(
                LinkUpdate::Snapshot(vec![wire_row("repo-a", "run-a")]),
                SystemTime::UNIX_EPOCH,
                cx,
            );
            shell.new_task_entry.activate();
            shell.new_task_entry.insert_char('a');
            let request = shell
                .new_task_entry
                .submit("repo-a".to_string(), 1, shell.board_scope_generation)
                .expect("request is correlated before the disconnect");

            shell.apply_link_update(
                LinkUpdate::Down("connection closed".to_string()),
                SystemTime::UNIX_EPOCH,
                cx,
            );
            assert_eq!(
                board::tasks_bar(&shell.board, &shell.new_task_entry)
                    .state
                    .word,
                "unavailable"
            );
            shell.apply_link_update(LinkUpdate::Snapshot(vec![]), SystemTime::UNIX_EPOCH, cx);
            assert_eq!(
                board::tasks_bar(&shell.board, &shell.new_task_entry)
                    .state
                    .word,
                "unavailable",
                "a recovering snapshot cannot replace the subscribed board"
            );
            assert!(shell.new_task_entry.settle(
                &request.repo_key,
                request.generation,
                request.board_scope_generation,
                board::CreateOutcome::Failed("late refusal".to_string()),
            ));
            assert!(
                shell.new_task_entry.action().label.contains("late refusal"),
                "disconnect recovery must not discard the originating request correlation"
            );
        });
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

    /// The menu has exactly one current label and more content than that
    /// current label alone.
    #[gpui::test]
    fn the_title_bar_renders_entries_with_one_current(cx: &mut gpui::TestAppContext) {
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
        assert!(
            menu.size.width > label.size.width + tokens::TITLE_BAR_MENU_GAP,
            "the other menu entries and declared gaps expand the menu beyond its singular current label"
        );
    }

    #[gpui::test]
    fn merges_menu_is_ordered_current_and_switches_to_content(cx: &mut gpui::TestAppContext) {
        let window = cx
            .update(|cx| {
                let bounds = Bounds::new(gpui::point(px(0.), px(0.)), DEFAULT_WINDOW_SIZE);
                cx.open_window(window_options(bounds), |_, cx| cx.new(Shell::new))
            })
            .unwrap();
        cx.run_until_parked();

        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
        let tasks = vcx
            .debug_bounds("title-bar-menu-label-Tasks")
            .expect("Tasks paints");
        let merges = vcx
            .debug_bounds("title-bar-menu-label-Merges")
            .expect("Merges paints");
        let config = vcx
            .debug_bounds("title-bar-menu-label-Config")
            .expect("Config paints");
        assert!(tasks.origin.x < merges.origin.x && merges.origin.x < config.origin.x);
        drop(vcx);
        window
            .update(cx, |shell, _window, cx| {
                shell.switch_screen(Screen::Merges, cx)
            })
            .unwrap();
        cx.run_until_parked();
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
        assert!(vcx.debug_bounds("merges-pane").is_some());
        assert!(vcx.debug_bounds("title-bar-menu-current-Merges").is_some());
        for selector in [
            "title-bar-menu-current-Sessions",
            "title-bar-menu-current-Tasks",
            "title-bar-menu-current-Traits",
            "title-bar-menu-current-Config",
        ] {
            assert!(
                vcx.debug_bounds(selector).is_none(),
                "{selector} is not current"
            );
        }
    }

    #[gpui::test]
    fn merges_chrome_paints_ordered_placeholder_blocks_and_noop_actions(
        cx: &mut gpui::TestAppContext,
    ) {
        let window = cx
            .update(|cx| {
                let bounds = Bounds::new(gpui::point(px(0.), px(0.)), DEFAULT_WINDOW_SIZE);
                cx.open_window(window_options(bounds), |_, cx| cx.new(Shell::new))
            })
            .unwrap();
        window
            .update(cx, |shell, _window, cx| {
                shell.switch_screen(Screen::Merges, cx);
            })
            .unwrap();
        cx.run_until_parked();

        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
        for selector in [
            "screen-header-title",
            "screen-header-summary",
            "screen-header-glyph",
            "bottom-bar",
            "bottom-bar-action-watch",
            "bottom-bar-action-hold",
            "preview-block-merge",
            "preview-lede-merge",
            "preview-block-gates",
            "preview-block-sign-offs",
            "preview-block-landing",
            "preview-footer",
        ] {
            assert!(vcx.debug_bounds(selector).is_some(), "{selector} paints");
        }
        let merge = vcx.debug_bounds("preview-block-merge").unwrap();
        let gates = vcx.debug_bounds("preview-block-gates").unwrap();
        let signoffs = vcx.debug_bounds("preview-block-sign-offs").unwrap();
        let landing = vcx.debug_bounds("preview-block-landing").unwrap();
        let footer = vcx.debug_bounds("preview-footer").unwrap();
        assert!(
            merge.origin.y < gates.origin.y
                && gates.origin.y < signoffs.origin.y
                && signoffs.origin.y < landing.origin.y
                && landing.origin.y < footer.origin.y
        );
        let selector = "breadcrumb";
        assert!(vcx.debug_bounds(selector).is_none(), "{selector} is absent");
        for selected in [1, 2] {
            window
                .update(cx, |shell, _window, cx| shell.select_merge(selected, cx))
                .unwrap();
            cx.run_until_parked();
            assert_eq!(vcx.debug_bounds("preview-block-gates"), Some(gates));
            assert_eq!(vcx.debug_bounds("preview-block-sign-offs"), Some(signoffs));
        }
        let selected_before = window
            .read_with(cx, |shell, _| shell.selected_merge)
            .unwrap();
        let preview_before = vcx
            .debug_bounds("preview-block-merge")
            .expect("merge preview paints before no-op actions");
        for selector in ["bottom-bar-action-watch", "bottom-bar-action-hold"] {
            let bounds = vcx.debug_bounds(selector).expect("action paints");
            vcx.simulate_click(
                gpui::point(
                    bounds.origin.x + bounds.size.width / 2.,
                    bounds.origin.y + bounds.size.height / 2.,
                ),
                gpui::Modifiers::default(),
            );
        }
        cx.run_until_parked();
        assert_eq!(
            window
                .read_with(cx, |shell, _| shell.selected_merge)
                .unwrap(),
            selected_before,
            "unbound Watch and Hold cannot change selection"
        );
        assert_eq!(
            vcx.debug_bounds("preview-block-merge"),
            Some(preview_before),
            "unbound Watch and Hold leave the rendered preview unchanged"
        );
    }

    #[test]
    fn merges_geometry_uses_declared_insets_and_prose_line_height() {
        assert_eq!(tokens::MAIN_PANE_PAD_TOP, px(18.));
        assert_eq!(tokens::MAIN_PANE_PAD_RIGHT, px(40.));
        assert_eq!(tokens::MAIN_PANE_PAD_BOTTOM, px(20.));
        assert_eq!(tokens::MAIN_PANE_PAD_LEFT, px(40.));
        assert_eq!(tokens::PREVIEW_PROSE_LINE_HEIGHT, px(17.));
    }

    #[gpui::test]
    fn merges_preview_overflow_is_strict_for_the_composed_column(cx: &mut gpui::TestAppContext) {
        let open_merges = |height, cx: &mut gpui::TestAppContext| {
            let window = cx
                .update(|cx| {
                    let bounds = Bounds::new(
                        gpui::point(px(0.), px(0.)),
                        gpui::size(px(1280.), px(height)),
                    );
                    cx.open_window(window_options(bounds), |_, cx| cx.new(Shell::new))
                })
                .unwrap();
            window
                .update(cx, |shell, _window, cx| {
                    shell.switch_screen(Screen::Merges, cx);
                })
                .unwrap();
            cx.run_until_parked();
            window
        };
        let fit_window = open_merges(900., cx);
        let mut vcx = gpui::VisualTestContext::from_window(fit_window.into(), cx);
        assert!(
            vcx.debug_bounds("merges-preview-overflow-fade-overlay")
                .is_none(),
            "the tall composed preview fits"
        );
        drop(vcx);
        let (viewport_height, content_height) = crate::overflow_fade::take_last_measurement()
            .expect("the preview overflow component measured the composed body");
        let exact_height = 900. - viewport_height + content_height;

        for (height, expected_overlay) in [(exact_height, false), (exact_height - 1., true)] {
            let window = open_merges(height, cx);
            let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
            assert_eq!(
                vcx.debug_bounds("merges-preview-overflow-fade-overlay")
                    .is_some(),
                expected_overlay,
                "height {height} has the expected strict overflow result"
            );
        }
    }

    #[gpui::test]
    fn merges_selection_starts_at_landing_and_stays_singular_across_moves(
        cx: &mut gpui::TestAppContext,
    ) {
        for selected in [0, 2, 3] {
            let window = cx
                .update(|cx| {
                    let bounds = Bounds::new(gpui::point(px(0.), px(0.)), DEFAULT_WINDOW_SIZE);
                    cx.open_window(window_options(bounds), |_, cx| cx.new(Shell::new))
                })
                .unwrap();
            window
                .update(cx, |shell, _window, cx| {
                    shell.switch_screen(Screen::Merges, cx);
                    shell.select_merge(selected, cx);
                })
                .unwrap();
            cx.run_until_parked();
            let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
            for (index, selector) in [
                "merge-row-selected-0",
                "merge-row-selected-1",
                "merge-row-selected-2",
                "merge-row-selected-3",
                "merge-row-selected-4",
            ]
            .into_iter()
            .enumerate()
            {
                assert_eq!(
                    vcx.debug_bounds(selector).is_some(),
                    index == selected,
                    "{selector} is selected only at index {selected}"
                );
            }
        }
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
    fn merges_header_drops_stale_scope_until_fresh_snapshot() {
        let mut face = CenterFace::new(RepoScope::All);
        let now = SystemTime::now();
        let mut first = wire_row("repo-a", "run-1");
        first.repo_path = "/work/acme/widgets".to_string();
        face.apply(LinkUpdate::Snapshot(vec![first]), now);
        assert_eq!(
            crate::merges::merges_header(connected_merge_scope(&face, None)).title,
            "merges — acme/widgets"
        );

        face.apply(LinkUpdate::Down("lost connection".to_string()), now);
        assert_eq!(
            crate::merges::merges_header(connected_merge_scope(&face, None)).title,
            "merges"
        );

        let mut replacement = wire_row("repo-b", "run-2");
        replacement.repo_path = "/srv/other/api".to_string();
        face.apply(LinkUpdate::Snapshot(vec![replacement]), now);
        assert_eq!(
            crate::merges::merges_header(connected_merge_scope(&face, None)).title,
            "merges — other/api"
        );
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
