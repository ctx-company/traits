use gpui::prelude::*;
use gpui::{
    Bounds, Context, Pixels, Render, Size, TitlebarOptions, Window, WindowOptions, div, px, size,
};

use crate::center_link::{self, LinkUpdate};
use crate::dashboard::Dashboard;
use crate::run_row::RepoScope;

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

pub enum CenterState {
    Connecting,
    Connected { dashboard: Dashboard },
    Unavailable { message: String },
}

pub struct Shell {
    center: CenterState,
    scope: RepoScope,
}

impl Shell {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let updates = center_link::start(None);
        cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                if this
                    .update(cx, |shell, cx| {
                        shell.apply(update);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            center: CenterState::Connecting,
            scope: RepoScope::All,
        }
    }

    /// Exposed so the scoped view is exercisable and testable ahead of any UI
    /// control for it — a control is out of scope for this task.
    pub fn set_scope(&mut self, scope: RepoScope) {
        self.scope = scope.clone();
        if let CenterState::Connected { dashboard } = &mut self.center {
            dashboard.set_scope(scope);
        }
    }

    fn apply(&mut self, update: LinkUpdate) {
        match update {
            LinkUpdate::Snapshot(rows) => {
                // A snapshot always installs a coherent `Connected` state,
                // replacing whatever came before it (including a prior
                // `Connected` dashboard — 0256.5's recovery case).
                self.center = CenterState::Connected {
                    dashboard: Dashboard::from_snapshot(rows, self.scope.clone()),
                };
            }
            LinkUpdate::Delta(delta) => {
                // A delta before any snapshot cannot occur in a conformant
                // ordered stream; drop it rather than paint incomplete state
                // as coherent.
                if let CenterState::Connected { dashboard } = &mut self.center {
                    dashboard.apply(delta);
                }
            }
            LinkUpdate::Unavailable(message) => {
                self.center = CenterState::Unavailable { message };
            }
        }
    }
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        match &self.center {
            CenterState::Connecting => div().child("connecting to center…"),
            CenterState::Unavailable { message } => {
                div().child(format!("center unavailable: {message}"))
            }
            CenterState::Connected { dashboard } => {
                let projected = dashboard.rows();
                let header = match &self.scope {
                    RepoScope::All => format!("{} runs", projected.len()),
                    RepoScope::Repo(repo_key) => {
                        format!("{} runs in {repo_key}", projected.len())
                    }
                };
                let mut list = div()
                    .id("run-list")
                    .flex()
                    .flex_col()
                    .size_full()
                    .overflow_y_scroll();
                for row in projected {
                    list = list.child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child(row.repo_label.clone())
                            .child(row.title.clone())
                            .child(format!("{} / {}", row.run_id, row.trait_id))
                            .child(row.state_text.clone())
                            .child(row.detail_text.clone())
                            .child(row.elapsed_text.clone())
                            .child(row.tokens_text.clone()),
                    );
                }
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .child(header)
                    .child(list)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
