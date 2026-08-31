//! Native gpui element tree for a projected [`DetailTree`]. Free functions,
//! not `Shell` methods: no `Context`, no `cx.listener` — this slice has no
//! controls (see `detail_tree.rs`'s scope) — so the whole render path is
//! directly callable from a test with no gpui `App`. Depth, hierarchy and
//! state live in the element structure, not in a pre-formatted string.

use gpui::prelude::*;
use gpui::{AnyElement, div};

use crate::detail::FollowState;
use crate::detail_tree::DetailTree;
use crate::frame_list::FrameList;
use crate::frame_list_view::frame_list_element;
use crate::overflow_fade::clipped_with_overflow_fade;

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
/// `Context`, no `App` required to call it. The frame rows themselves are
/// `frame_list_view::frame_list_element`'s — this function only supplies the
/// header above them.
pub fn detail_element(tree: &DetailTree) -> AnyElement {
    let list = FrameList::from_tree(tree);
    div()
        .id("detail-pane")
        .flex()
        .flex_col()
        .size_full()
        .gap_1()
        .child(header_element(tree))
        .child(clipped_with_overflow_fade(frame_list_element(&list)))
        .into_any_element()
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

/// The live-follow banner: `None` while `Following` (nothing to say), a
/// labelled strip while `Stale` — worded to mirror `CenterFace::header`'s
/// existing staleness shape ("detail unreachable — showing state as of …;
/// retrying") so the two faces never word the same condition two ways.
pub fn follow_element(follow: &FollowState) -> Option<AnyElement> {
    match follow {
        FollowState::Following => None,
        FollowState::Stale { reason } => Some(
            div()
                .id("detail-follow-stale")
                .child(format!(
                    "detail unreachable — showing last known state ({reason}); retrying"
                ))
                .into_any_element(),
        ),
    }
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

    #[test]
    fn follow_element_is_absent_while_following_and_present_while_stale() {
        assert!(follow_element(&FollowState::Following).is_none());
        let _stale: AnyElement = follow_element(&FollowState::Stale {
            reason: "subscription closed".to_string(),
        })
        .expect("a stale follow state renders a banner");
    }
}
