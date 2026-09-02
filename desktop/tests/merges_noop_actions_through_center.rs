//! Merges interactions are deliberately local: neither unbound bottom-bar
//! action nor selection movement may send a request through the center link.

mod support;

use std::time::Duration;

use ctx_traits_desktop::shell::{Shell, Screen, window_options};
use gpui::{AppContext, Bounds, VisualTestContext, px, size};

fn click(vcx: &mut VisualTestContext, selector: &'static str) {
    let bounds = vcx.debug_bounds(selector).expect("interactive element paints");
    vcx.simulate_click(
        gpui::point(
            bounds.origin.x + bounds.size.width / 2.,
            bounds.origin.y + bounds.size.height / 2.,
        ),
        gpui::Modifiers::default(),
    );
}

#[gpui::test]
fn merges_actions_and_cross_section_selection_do_not_write_to_center(
    cx: &mut gpui::TestAppContext,
) {
    let guard = support::scratch("merges-noop-actions-through-center");
    // SAFETY: this integration target owns its process environment.
    let socket = unsafe { support::install_center_env(&guard.0) };
    let peer = support::FakePeer::bind(&socket);

    let window = cx
        .update(|cx| {
            let bounds = Bounds::new(gpui::point(px(0.), px(0.)), size(px(1280.), px(900.)));
            cx.open_window(window_options(bounds), |_, cx| cx.new(Shell::new))
        })
        .expect("open Merges window");
    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let rows = [
        support::wire_row("repo-a", "/fixtures/repo-a/session.json", "run-a"),
        support::wire_row("repo-b", "/fixtures/repo-b/session.json", "run-b"),
    ];
    connection.serve_snapshot(&subscribe_id, &rows);
    window
        .update(cx, |shell, _window, cx| {
            shell.switch_screen(Screen::Merges, cx);
        })
        .expect("switch to Merges");
    cx.run_until_parked();

    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    for selector in ["preview-block-merge", "preview-block-landing"] {
        assert!(vcx.debug_bounds(selector).is_some(), "{selector} paints before interaction");
    }
    click(&mut vcx, "bottom-bar-action-watch");
    click(&mut vcx, "bottom-bar-action-hold");
    click(&mut vcx, "merge-row-2");
    cx.run_until_parked();

    for selector in ["merge-row-selected-2", "preview-block-merge", "preview-block-landing"] {
        assert!(vcx.debug_bounds(selector).is_some(), "{selector} remains rendered");
    }
    connection.assert_no_request_bytes(Duration::from_millis(250));
}
