//! Native gpui element tree for the window's title bar. A free function,
//! not a `Shell` method — the `rail_view.rs`/`bottom_bar_view.rs`/
//! `frame_list_view.rs` convention: no `Context`, no `cx.listener`,
//! directly callable from a test with no `App`.
//!
//! This bar renders no mark of any kind: the design set points every
//! screen's title bar at `ctx-logo-white.png`, but no logo asset is
//! tracked anywhere in the repository (`git ls-files` matching
//! `logo|icon|*.png` returns only reference/design PNGs and fonts). `0260`
//! (packaging, launcher, icon) needs to supply an owner-approved, tracked,
//! licensed title-bar mark before any screen can render one.
//!
//! The menu carries exactly one entry, `SESSIONS` — the same
//! rule-10-interface-word class `rail_view.rs`'s `"Spaces"` already is.
//! The export renders four entries (Sessions · Traits · Tasks · Config),
//! but the screen spec names only one; no `Screen` enum, no registry, no
//! navigation is built here.

use gpui::prelude::*;
use gpui::{AnyElement, div, rgb};

use crate::tokens;

/// The menu's only entry — the current screen has no navigation seam yet,
/// so this is a plain interface word, not a registry lookup.
pub const SESSIONS: &str = "Sessions";

pub fn title_bar_element(current_screen: &str) -> AnyElement {
    let menu = div()
        .debug_selector(|| "title-bar-menu".to_string())
        .flex()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(tokens::TITLE_BAR_MENU_GAP)
        .child(
            div()
                .debug_selector(|| "title-bar-menu-current".to_string())
                .font_family(tokens::FONT_SANS)
                .text_size(tokens::TITLE_BAR_MENU_SIZE)
                .font_weight(tokens::WEIGHT_NORMAL)
                .text_color(rgb(tokens::TEXT_BRIGHT))
                .whitespace_nowrap()
                .child(current_screen.to_string()),
        );

    div()
        .debug_selector(|| "title-bar".to_string())
        .flex()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .w_full()
        .h(tokens::TITLE_BAR_HEIGHT)
        .py(tokens::TITLE_BAR_PAD_Y)
        .px(tokens::TITLE_BAR_PAD_X)
        .child(div().flex_1())
        .child(menu)
        .into_any_element()
}
