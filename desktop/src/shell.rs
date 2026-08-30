use std::time::SystemTime;

use gpui::prelude::*;
use gpui::{
    Bounds, Context, Pixels, Render, SharedString, Size, TitlebarOptions, Window, WindowOptions,
    div, px, size,
};

use crate::center_link::{self, LinkUpdate};
use crate::dashboard::Dashboard;
use crate::detail::{self, LoadRequest, RunDetail};
use crate::detail_view;
use crate::run_row::{RepoScope, RunRow};

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
                .child(row.tokens_text.clone());
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
        let mut column = div()
            .flex()
            .flex_col()
            .size_full()
            .child(header)
            .child(list);
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
