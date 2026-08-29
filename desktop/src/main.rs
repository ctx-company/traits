use ctx_traits_desktop::shell::{DEFAULT_WINDOW_SIZE, Shell, window_options};
use gpui::{App, AppContext, Application, Bounds};

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, DEFAULT_WINDOW_SIZE, cx);
        cx.open_window(window_options(bounds), |_, cx| cx.new(Shell::new))
            .expect("open desktop shell window");
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
