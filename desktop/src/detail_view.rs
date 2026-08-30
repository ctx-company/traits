//! Native gpui element tree for a projected [`DetailTree`]. Free functions,
//! not `Shell` methods: no `Context`, no `cx.listener` — this slice has no
//! controls (see `detail_tree.rs`'s scope) — so the whole render path is
//! directly callable from a test with no gpui `App`. Depth, hierarchy and
//! state live in the element structure, not in a pre-formatted string.

use gpui::prelude::*;
use gpui::{AnyElement, SharedString, div, px};

use crate::detail_tree::{DetailNode, DetailTree, FrameState};

fn state_word(state: FrameState) -> &'static str {
    match state {
        FrameState::Pending => "pending",
        FrameState::Ready => "ready",
        FrameState::Done => "done",
        FrameState::Rejected => "rejected",
        FrameState::Blocked => "blocked",
        FrameState::Skipped => "skipped",
        FrameState::Routed => "routed",
        FrameState::Structural => "—",
    }
}

fn node_element(node: &DetailNode, depth: usize, path: &str) -> AnyElement {
    let mut label = node.title.clone();
    if let Some(ordinal) = node.ordinal
        && !label.contains(char::is_numeric)
    {
        label = format!("{label} {ordinal}");
    }
    let mut row = div()
        .id(SharedString::from(format!(
            "detail-node-{path}-{}-{}",
            node.kind,
            node.id.clone().unwrap_or_default()
        )))
        .flex()
        .flex_row()
        .gap_2()
        .pl(px(depth as f32 * 16.))
        .child(label)
        .child(if node.current {
            "current".to_string()
        } else {
            String::new()
        })
        .child(
            node.session_state
                .map(|state| format!("{state:?}"))
                .unwrap_or_else(|| state_word(node.state).to_string()),
        );
    if !node.reason.is_empty() {
        row = row.child(node.reason.clone());
    }
    if let Some(activity) = &node.activity {
        row = row.child(format!(
            "{:?}{}",
            activity.kind,
            activity
                .text
                .as_ref()
                .map(|text| format!(": {text}"))
                .unwrap_or_default()
        ));
    }
    if let Some(narration) = &node.narration {
        row = row.child(narration.clone());
    }

    let mut column = div().flex().flex_col().child(row);
    for (index, child) in node.children.iter().enumerate() {
        column = column.child(node_element(child, depth + 1, &format!("{path}.{index}")));
    }
    column.into_any_element()
}

fn header_element(tree: &DetailTree) -> AnyElement {
    let header = &tree.header;
    let mut row = div()
        .id("detail-header")
        .flex()
        .flex_row()
        .gap_2()
        .child(header.title.clone())
        .child(format!("{:?}", header.run_state))
        .child(header.elapsed_text.clone())
        .child(header.tokens_text.clone());
    if let Some(task_value) = &header.task_value {
        row = row.child(task_value.clone());
    }
    if let Some(current) = &header.current_sequence_title {
        row = row.child(current.clone());
    }
    if let Some(landing) = &header.landing {
        row = row.child(landing.clone());
    }
    if let Some(kind) = &header.next_frame_kind {
        row = row.child(kind.clone());
    }
    if let Some(stop_reason) = &header.stop_reason_text {
        row = row.child(stop_reason.clone());
    }
    row.into_any_element()
}

/// Build the native element tree for a loaded [`DetailTree`]. Pure: no
/// `Context`, no `App` required to call it.
pub fn detail_element(tree: &DetailTree) -> AnyElement {
    let mut column = div()
        .id("detail-pane")
        .flex()
        .flex_col()
        .gap_1()
        .child(header_element(tree));
    for (index, root) in tree.roots.iter().enumerate() {
        column = column.child(node_element(root, 0, &index.to_string()));
    }
    column.into_any_element()
}

/// Native "loading" state, painted while a selection's background read is
/// in flight.
pub fn loading_element() -> AnyElement {
    div()
        .id("detail-loading")
        .child("loading…")
        .into_any_element()
}

/// Native failure state, carrying the load's failure reason verbatim.
pub fn failed_element(reason: &str) -> AnyElement {
    div()
        .id("detail-failed")
        .child(format!("unreadable: {reason}"))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detail_tree;
    use ctx_traits_core::procedure::session::Session;

    fn session() -> Session {
        serde_json::from_value(serde_json::json!({
            "schema-version": "0.1.0",
            "session-id": "session-fixture",
            "run-id": "run-fixture",
            "trait-id": "fixture-trait",
            "current-run-index": 0,
            "status": "completed",
            "provenance": {
                "started-by": {"surface": "test", "caller": "detail-view-fixture"},
                "state-source": "test",
            },
            "ledger": {
                "run-id": "run-fixture",
                "trait-id": "fixture-trait",
                "current-run-index": 0,
                "final-state": "completed",
                "sequence-statuses": [{
                    "sequence-index": 0,
                    "run-index": 0,
                    "item-id": "solo-item",
                    "title": "Solo item",
                    "status": "accepted",
                    "reason": "",
                    "position-path": [],
                }],
            },
            "state-digest": "sha256:fixture",
        }))
        .expect("fixture session")
    }

    #[test]
    fn detail_element_builds_from_a_projected_tree_with_no_app() {
        let session = session();
        let tree = detail_tree::project(&session, &detail_tree::ActivityOverlay::default(), false);
        // Constructing the element must not panic and must reach an
        // `AnyElement` — proof this render path needs no `App`/`Context`.
        let _element: AnyElement = detail_element(&tree);
    }

    #[test]
    fn loading_and_failed_states_build_without_a_tree() {
        let _loading: AnyElement = loading_element();
        let _failed: AnyElement = failed_element("bad json");
    }
}
