use gpui::{
    Bounds, Context, IntoElement, ParentElement, Pixels, Render, Size, TitlebarOptions, Window,
    WindowOptions, div, px, size,
};

use crate::center_link::{self, LinkUpdate};
use ctx_traits_io::center::CenterPublicRow;

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
    Connected { rows: Vec<CenterPublicRow> },
    Unavailable { message: String },
}

pub struct Shell {
    center: CenterState,
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
        }
    }

    fn apply(&mut self, update: LinkUpdate) {
        self.center = match update {
            LinkUpdate::Snapshot(rows) => CenterState::Connected { rows },
            LinkUpdate::Unavailable(message) => CenterState::Unavailable { message },
        };
    }
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        match &self.center {
            CenterState::Connecting => div().child("connecting to center…"),
            CenterState::Unavailable { message } => {
                div().child(format!("center unavailable: {message}"))
            }
            CenterState::Connected { rows } => {
                let mut container = div().child(format!("{} runs", rows.len()));
                for row in rows {
                    container = container.child(div().child(row.summary.run_id.clone()));
                }
                container
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
