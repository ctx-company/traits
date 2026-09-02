//! Native gpui element tree for a [`BottomBar`]. A free function, not a
//! `Shell` method: no `Context`, no `cx.listener` — this component binds no
//! behaviour of its own (`0265.4` wires `pause`) — so the whole render path
//! is directly callable from a test with no gpui `App`, mirroring
//! `detail_view.rs`'s convention.

use gpui::prelude::*;
use gpui::{AnyElement, SharedString, div, rgb};

use crate::bottom_bar::{ActionTone, BarAction, BarActionId, BottomBar};
use crate::run_row::role_color;
use crate::tokens;

/// A click handler for a bound bar action. Boxed rather than generic so
/// `bar_element` keeps one signature for both the bound and unbound cases,
/// and so a test can pass `None` without a turbofish.
pub type BarActionHandler =
    Box<dyn Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static>;

fn mono_11(color: u32) -> gpui::Div {
    div()
        .font_family(tokens::FONT_MONO)
        .text_size(tokens::SIZE_11)
        .text_color(rgb(color))
}

fn action_element(action: &BarAction, handler: Option<BarActionHandler>) -> AnyElement {
    let color = match action.tone {
        ActionTone::Primary => tokens::TEXT_BRIGHT,
        ActionTone::Secondary => tokens::TEXT_SECONDARY,
        ActionTone::Muted => tokens::TEXT_MUTED,
        ActionTone::Danger => tokens::DANGER,
    };
    let selector = match action.id {
        BarActionId::Watch => "bottom-bar-action-watch",
        BarActionId::Hold => "bottom-bar-action-hold",
        BarActionId::NewTask => "bottom-bar-action-NewTask",
        _ => "bottom-bar-action",
    };
    let element = mono_11(color)
        .debug_selector(move || selector.to_string())
        .child(action.label.clone());
    match handler {
        // gpui requires a stateful element (`.id(...)`) for `.on_click` to
        // attach; invisible in paint — no geometry, no tone.
        Some(handler) => element
            .id(SharedString::from(format!("bottom-bar-{:?}", action.id)))
            .on_click(handler)
            .into_any_element(),
        None => element.into_any_element(),
    }
}

/// Build the native element tree for a [`BottomBar`]. Pure: no `Context`, no
/// `App` required to call it. `on_pause` is attached to the `Pause` action
/// only when supplied — the caller (`Shell::render`) decides eligibility;
/// this component still binds no behaviour of its own beyond wiring the one
/// handler it is given.
pub fn bar_element(
    bar: &BottomBar,
    on_pause: Option<BarActionHandler>,
    on_new_task: Option<BarActionHandler>,
) -> AnyElement {
    let mut left = div()
        .id("bottom-bar-state")
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::BOTTOM_BAR_GAP)
        .child(mono_11(role_color(bar.state.role)).child(bar.state.word));
    if let Some(detail) = bar.detail_text() {
        left = left.child(mono_11(tokens::TEXT_SECONDARY).child(detail));
    }

    let mut right = div()
        .id("bottom-bar-actions")
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::BOTTOM_BAR_GAP);
    let mut on_pause = on_pause;
    let mut on_new_task = on_new_task;
    for (index, action) in bar.actions.iter().enumerate() {
        if index > 0 {
            right = right.child(mono_11(tokens::TEXT_FAINT).child("·"));
        }
        let handler = match action.id {
            BarActionId::Pause => on_pause.take(),
            BarActionId::NewTask => on_new_task.take(),
            BarActionId::WatchRaw
            | BarActionId::AuthorTrait
            | BarActionId::EditRuntimeToml
            | BarActionId::Watch
            | BarActionId::Hold => None,
        };
        right = right.child(action_element(action, handler));
    }

    div()
        .id("bottom-bar")
        // `debug_selector` is a gpui-provided no-op outside test/test-support
        // builds; it lets a composition proof read this element's actual
        // painted bounds via `VisualTestContext::debug_bounds` instead of a
        // caller-recomputed proxy.
        .debug_selector(|| "bottom-bar".to_string())
        .w_full()
        .flex()
        .flex_row()
        .justify_between()
        .items_center()
        .border_1()
        .border_color(rgb(tokens::BORDER_SOFT))
        .px(tokens::BOTTOM_BAR_PAD_X)
        .py(tokens::BOTTOM_BAR_PAD_Y)
        .child(left)
        .child(right)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bottom_bar::sessions_bar;
    use crate::run_row::{RowState, RunRow, StatePresentation, StateRole};
    use gpui::px;

    #[test]
    fn bottom_bar_border_matches_its_token() {
        assert_eq!(tokens::BOTTOM_BAR_BORDER, px(1.));
    }

    #[test]
    fn bar_element_builds_from_a_projected_bar_with_no_app() {
        let row = RunRow {
            ledger_path: "/repo/session.json".to_string(),
            session_id: "session".to_string(),
            run_id: "run".to_string(),
            repo_key: "repo".to_string(),
            repo_path: "/repo".to_string(),
            repo_label: "repo".to_string(),
            title: "title".to_string(),
            session_title: None,
            trait_id: "fixture-trait".to_string(),
            state: RowState::Live,
            state_text: "running".to_string(),
            detail_text: String::new(),
            elapsed_text: "00:00:00".to_string(),
            tokens_text: "-".to_string(),
            live: true,
            modified_epoch_secs: 0,
            verdict_rounds: Some(2),
            elapsed_seconds: 0,
            started_at_epoch: None,
        };
        let bar = sessions_bar(&row, None, None);
        // Constructing the element must not panic and must reach an
        // `AnyElement` — proof this render path needs no `App`/`Context`.
        let _element: AnyElement = bar_element(&bar, None, None);
    }

    #[test]
    fn bar_element_builds_with_no_detail_segments() {
        let bar = BottomBar {
            state: StatePresentation {
                word: "unreadable",
                role: StateRole::Danger,
            },
            detail: Vec::new(),
            actions: Vec::new(),
        };
        let _element: AnyElement = bar_element(&bar, None, None);
    }

    #[test]
    fn bar_element_builds_with_a_bound_pause_handler() {
        let row = RunRow {
            ledger_path: "/repo/session.json".to_string(),
            session_id: "session".to_string(),
            run_id: "run".to_string(),
            repo_key: "repo".to_string(),
            repo_path: "/repo".to_string(),
            repo_label: "repo".to_string(),
            title: "title".to_string(),
            session_title: None,
            trait_id: "fixture-trait".to_string(),
            state: RowState::Live,
            state_text: "running".to_string(),
            detail_text: String::new(),
            elapsed_text: "00:00:00".to_string(),
            tokens_text: "-".to_string(),
            live: true,
            modified_epoch_secs: 0,
            verdict_rounds: None,
            elapsed_seconds: 0,
            started_at_epoch: None,
        };
        let bar = sessions_bar(&row, None, None);
        let handler: BarActionHandler = Box::new(|_, _, _| {});
        let _element: AnyElement = bar_element(&bar, Some(handler), None);
    }
}
