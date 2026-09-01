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
//! The first two owned screens share this deliberately small menu. A caller
//! supplies the navigation handlers; this view owns only the stable geometry.

use gpui::prelude::*;
use gpui::{AnyElement, SharedString, div, rgb};

use crate::tokens;

/// The menu's only entry — the current screen has no navigation seam yet,
/// so this is a plain interface word, not a registry lookup.
pub const SESSIONS: &str = "Sessions";
pub const TASKS: &str = "Tasks";
pub const TRAITS: &str = "Traits";

pub type MenuHandler = Box<dyn Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static>;

fn menu_entry(
    label: &'static str,
    current_screen: &str,
    handler: Option<MenuHandler>,
) -> AnyElement {
    let current = current_screen == label;
    let element = div()
        .id(SharedString::from(format!("title-bar-menu-{label}")))
        .debug_selector(move || {
            if current {
                "title-bar-menu-current".to_string()
            } else {
                "title-bar-menu-entry".to_string()
            }
        })
        .font_family(tokens::FONT_SANS)
        .text_size(tokens::TITLE_BAR_MENU_SIZE)
        .font_weight(tokens::WEIGHT_NORMAL)
        .text_color(rgb(if current {
            tokens::TEXT_BRIGHT
        } else {
            tokens::TEXT_SECONDARY
        }))
        .whitespace_nowrap()
        .child(label);
    match handler {
        Some(handler) => element.on_click(handler).into_any_element(),
        None => element.into_any_element(),
    }
}

pub fn title_bar_element(
    current_screen: &str,
    on_sessions: Option<MenuHandler>,
    on_tasks: Option<MenuHandler>,
    on_traits: Option<MenuHandler>,
) -> AnyElement {
    let menu = div()
        .debug_selector(|| "title-bar-menu".to_string())
        .flex()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(tokens::TITLE_BAR_MENU_GAP)
        .child(menu_entry(SESSIONS, current_screen, on_sessions))
        .child(menu_entry(TASKS, current_screen, on_tasks))
        .child(menu_entry(TRAITS, current_screen, on_traits));

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
