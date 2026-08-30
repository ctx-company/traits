//! `screens/sessions.md`/`grammar.md`'s Sessions frame list as a gpui-free
//! model, mirroring `bottom_bar.rs`'s split: the rows, forms, tones and
//! right-side joining are all assertable in plain `#[test]`s here, with no
//! gpui `App` needed. `frame_list_view.rs` paints exactly this model.
//!
//! # The brightness invariant
//!
//! [`FrameRow`] carries no `bright`/`selected` field — brightness is a
//! function of [`FrameList::selected`], never of a row's own state. A
//! caller holding one `FrameRow` cannot make it bright; only the list as a
//! whole knows which index (if any) is selected, and `is_bright` is the one
//! place that answers the question. `from_tree` selects the current node
//! only when the run is live (`tree.header.run_state ==
//! SessionState::Running`, exact per `SessionState::derive`'s own liveness
//! rule) — a settled or failed run promotes nothing, so its title stays
//! `text`, never `text-bright`.

use std::time::Duration;

use ctx_traits_core::procedure::activity::SessionState;

use crate::detail_tree::{DetailNode, DetailTree, FrameState};
use crate::placeholders;
use crate::run_row::{StatePresentation, StateRole, session_state_presentation};

/// Rule 4's dot vocabulary plus `danger` (a failed frame must stay
/// distinguishable from every other state — the design does not name this
/// variant, `0265.5` adds it, see the work summary's owner correction).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DotTone {
    Ok,
    Accent,
    Idle,
    Dim,
    Warn,
    Danger,
}

fn dot_tone_for_role(role: StateRole) -> DotTone {
    match role {
        StateRole::Accent => DotTone::Accent,
        StateRole::Ok => DotTone::Ok,
        StateRole::Warn => DotTone::Warn,
        StateRole::Danger => DotTone::Danger,
        StateRole::Neutral => DotTone::Dim,
    }
}

/// A row's right-hand content, typed rather than a pre-joined string — the
/// one place `Live`'s `·`-joined segments are assembled is
/// `frame_list_view.rs`, mirroring `bottom_bar::detail_text`'s "no dangling
/// separator" shape.
#[derive(Debug, Clone, PartialEq)]
pub enum RightSide {
    None,
    Elapsed(Option<Duration>),
    Word(StatePresentation),
    Live {
        state: StatePresentation,
        round: Option<u64>,
        role: Option<String>,
    },
}

/// Which of the design's row geometries a row paints in. `Group` covers the
/// synthesized structural nodes the design does not name (see the work
/// summary's owner correction 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowForm {
    Done,
    Pending,
    Current,
    Group,
}

/// One flattened row, ready for native rendering. No `bright`/`selected`
/// field — see the module doc comment.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameRow {
    pub depth: usize,
    pub title: String,
    pub form: RowForm,
    /// `None` only for [`RowForm::Group`] — rule 4 admits dots for status,
    /// and a synthesized group has none.
    pub dot: Option<DotTone>,
    pub right: RightSide,
    /// Only [`RowForm::Current`] carries a description.
    pub description: Option<String>,
}

/// Exhaustive over the seven non-structural [`FrameState`] variants, no
/// catch-all arm; a new `FrameState` is a compile error here. `Rejected`
/// reuses the shared table's `"failed"`/`Danger` entry
/// (`session_state_presentation(SessionState::Failed)`) rather than
/// inventing a second word for the same meaning — the owner correction this
/// task reports. `Skipped`/`Routed` have no equivalent in that table, so
/// they get their own neutral words here, in this one place.
fn frame_state_presentation(state: FrameState) -> StatePresentation {
    match state {
        FrameState::Done => StatePresentation {
            word: "done",
            role: StateRole::Ok,
        },
        FrameState::Rejected => session_state_presentation(SessionState::Failed),
        FrameState::Blocked => session_state_presentation(SessionState::Blocked),
        FrameState::Skipped => StatePresentation {
            word: "skipped",
            role: StateRole::Neutral,
        },
        FrameState::Routed => StatePresentation {
            word: "routed",
            role: StateRole::Neutral,
        },
        FrameState::Pending => StatePresentation {
            word: "pending",
            role: StateRole::Neutral,
        },
        FrameState::Ready => StatePresentation {
            word: "ready",
            role: StateRole::Neutral,
        },
        FrameState::Structural => {
            unreachable!("a structural node is routed to RowForm::Group before this is called")
        }
    }
}

/// A flattened, form-decided view of a [`DetailTree`]'s frame list.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameList {
    rows: Vec<FrameRow>,
    selected: Option<usize>,
}

impl FrameList {
    /// Build the list from a projected [`DetailTree`]. Pure: no gpui types,
    /// no IO.
    pub fn from_tree(tree: &DetailTree) -> Self {
        let mut rows = Vec::new();
        let mut selected = None;
        let live = tree.header.run_state == SessionState::Running;
        for root in &tree.roots {
            push_node(root, 0, tree, live, &mut rows, &mut selected);
        }
        FrameList { rows, selected }
    }

    pub fn rows(&self) -> &[FrameRow] {
        &self.rows
    }

    /// The index of the single bright-titled row, if any.
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Whether `index` is the list's one bright-titled row.
    pub fn is_bright(&self, index: usize) -> bool {
        self.selected == Some(index)
    }
}

fn group_title(node: &DetailNode) -> String {
    let mut label = node.title.clone();
    if let Some(ordinal) = node.ordinal
        && !label.contains(char::is_numeric)
    {
        label = format!("{label} {ordinal}");
    }
    label
}

fn push_node(
    node: &DetailNode,
    depth: usize,
    tree: &DetailTree,
    live: bool,
    rows: &mut Vec<FrameRow>,
    selected: &mut Option<usize>,
) {
    let row = if node.state == FrameState::Structural {
        FrameRow {
            depth,
            title: group_title(node),
            form: RowForm::Group,
            dot: None,
            right: RightSide::None,
            description: None,
        }
    } else if node.current {
        if live {
            *selected = Some(rows.len());
            let state = session_state_presentation(tree.header.run_state);
            FrameRow {
                depth,
                title: node.title.clone(),
                form: RowForm::Current,
                dot: Some(DotTone::Accent),
                right: RightSide::Live {
                    state,
                    round: tree.header.verdict_rounds,
                    role: tree.header.current_agent_role.clone(),
                },
                description: Some(placeholders::CURRENT_FRAME_INTENT.label.to_string()),
            }
        } else {
            // A settled or failed run stopped at its current frame renders
            // the settled form its actual state presents as — never
            // `Current`, so a failed run never reads as live or done. A
            // reconstructed `FrameState::Rejected` takes precedence over the
            // session-level state: `Status::Rejected` derives
            // `SessionState::WaitingOnAgent`, which would otherwise present
            // this row as neutral "awaiting agent" and lose the danger role
            // rule 2 requires for a failed frame (review-verdict-1 blocker
            // `current-rejected-loses-danger`). Every other stopped-current
            // case keeps the shared session-state presentation.
            let presentation = if node.state == FrameState::Rejected {
                frame_state_presentation(FrameState::Rejected)
            } else {
                let state = node.session_state.unwrap_or(tree.header.run_state);
                session_state_presentation(state)
            };
            FrameRow {
                depth,
                title: node.title.clone(),
                form: RowForm::Done,
                dot: Some(dot_tone_for_role(presentation.role)),
                right: RightSide::Word(presentation),
                description: None,
            }
        }
    } else {
        match node.state {
            FrameState::Done => FrameRow {
                depth,
                title: node.title.clone(),
                form: RowForm::Done,
                dot: Some(DotTone::Ok),
                right: RightSide::Elapsed(node.span),
                description: None,
            },
            FrameState::Pending | FrameState::Ready => FrameRow {
                depth,
                title: node.title.clone(),
                form: RowForm::Pending,
                dot: Some(DotTone::Idle),
                right: RightSide::None,
                description: None,
            },
            FrameState::Rejected
            | FrameState::Blocked
            | FrameState::Skipped
            | FrameState::Routed => {
                let presentation = frame_state_presentation(node.state);
                FrameRow {
                    depth,
                    title: node.title.clone(),
                    form: RowForm::Done,
                    dot: Some(dot_tone_for_role(presentation.role)),
                    right: RightSide::Word(presentation),
                    description: None,
                }
            }
            FrameState::Structural => unreachable!("handled above"),
        }
    };
    rows.push(row);
    for child in &node.children {
        push_node(child, depth + 1, tree, live, rows, selected);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detail_tree::ActivityOverlay;
    use ctx_traits_core::procedure::session::Session;

    fn session(status: &str, final_state: &str, sequence_statuses: serde_json::Value) -> Session {
        serde_json::from_value(serde_json::json!({
            "schema-version": "0.1.0",
            "session-id": "session-fixture",
            "run-id": "run-fixture",
            "trait-id": "fixture-trait",
            "current-run-index": 0,
            "status": status,
            "provenance": {
                "started-by": {"surface": "test", "caller": "frame-list-fixture"},
                "state-source": "test",
            },
            "ledger": {
                "run-id": "run-fixture",
                "trait-id": "fixture-trait",
                "current-run-index": 0,
                "final-state": final_state,
                "sequence-statuses": sequence_statuses,
            },
            "state-digest": "sha256:fixture",
        }))
        .expect("fixture session")
    }

    fn status(item_id: &str, title: &str, status: &str) -> serde_json::Value {
        serde_json::json!({
            "sequence-index": 0,
            "run-index": 0,
            "item-id": item_id,
            "title": title,
            "status": status,
            "reason": "",
            "position-path": [],
        })
    }

    #[test]
    fn every_frame_state_maps_to_its_own_form() {
        let cases = [
            ("accepted", RowForm::Done),
            ("rejected", RowForm::Done),
            ("blocked", RowForm::Done),
            ("skipped", RowForm::Done),
            ("routed", RowForm::Done),
            ("pending", RowForm::Pending),
            ("ready", RowForm::Pending),
        ];
        for (status_wire, expected_form) in cases {
            let session = session(
                "completed",
                "completed",
                serde_json::Value::Array(vec![status("item", "Item", status_wire)]),
            );
            let tree = crate::detail_tree::project(&session, &ActivityOverlay::default(), false);
            let list = FrameList::from_tree(&tree);
            assert_eq!(list.rows()[0].form, expected_form, "{status_wire}");
        }
    }

    #[test]
    fn a_rejected_frame_and_a_stopped_at_current_failed_frame_both_render_failed_never_ok() {
        let rejected_session = session(
            "completed",
            "completed",
            serde_json::Value::Array(vec![status("item", "Item", "rejected")]),
        );
        let tree =
            crate::detail_tree::project(&rejected_session, &ActivityOverlay::default(), false);
        let list = FrameList::from_tree(&tree);
        assert_eq!(list.rows()[0].dot, Some(DotTone::Danger));
        assert!(matches!(
            &list.rows()[0].right,
            RightSide::Word(StatePresentation {
                word: "failed",
                role: StateRole::Danger,
            })
        ));

        let mut json = session(
            "failed",
            "failed",
            serde_json::Value::Array(vec![status("item", "Item", "ready")]),
        );
        json.active_path = vec![ctx_traits_core::procedure::runtime::PathSegment {
            kind: "procedure".to_string(),
            id: Some("item".to_string()),
            index: 0,
            iteration: None,
            item_index: None,
        }];
        let tree = crate::detail_tree::project(&json, &ActivityOverlay::default(), false);
        let list = FrameList::from_tree(&tree);
        assert_eq!(list.rows()[0].form, RowForm::Done);
        assert_eq!(list.rows()[0].dot, Some(DotTone::Danger));
        assert!(matches!(
            &list.rows()[0].right,
            RightSide::Word(StatePresentation {
                word: "failed",
                role: StateRole::Danger,
            })
        ));
        assert!(list.selected().is_none());
    }

    /// A settled current node whose reconstructed `FrameState` is `Rejected`
    /// must retain danger + `failed`, not the neutral "awaiting agent" the
    /// shared `SessionState` table would otherwise present (review-verdict-1
    /// blocker `current-rejected-loses-danger`).
    #[test]
    fn current_rejected_frame_renders_failed() {
        let mut json = session(
            "rejected",
            "failed",
            serde_json::Value::Array(vec![status("item", "Item", "rejected")]),
        );
        json.active_path = vec![ctx_traits_core::procedure::runtime::PathSegment {
            kind: "procedure".to_string(),
            id: Some("item".to_string()),
            index: 0,
            iteration: None,
            item_index: None,
        }];
        let tree = crate::detail_tree::project(&json, &ActivityOverlay::default(), false);
        let list = FrameList::from_tree(&tree);
        assert_eq!(list.rows()[0].form, RowForm::Done);
        assert_eq!(list.rows()[0].dot, Some(DotTone::Danger));
        assert!(matches!(
            &list.rows()[0].right,
            RightSide::Word(StatePresentation {
                word: "failed",
                role: StateRole::Danger,
            })
        ));
        assert!(
            list.selected().is_none(),
            "a non-live current row is never the selected/bright row"
        );
    }

    #[test]
    fn bright_title_count_is_exactly_one_for_a_live_run_with_a_selection() {
        let mut json = session(
            "awaiting-agent-output",
            "running",
            serde_json::Value::Array(vec![status("item", "Item", "ready")]),
        );
        json.active_path = vec![ctx_traits_core::procedure::runtime::PathSegment {
            kind: "procedure".to_string(),
            id: Some("item".to_string()),
            index: 0,
            iteration: None,
            item_index: None,
        }];
        let tree = crate::detail_tree::project(&json, &ActivityOverlay::default(), true);
        let list = FrameList::from_tree(&tree);
        let bright_count = (0..list.rows().len())
            .filter(|&index| list.is_bright(index))
            .count();
        assert_eq!(bright_count, 1);
        assert_eq!(list.rows()[0].form, RowForm::Current);
    }

    #[test]
    fn bright_title_count_is_zero_for_a_completed_run() {
        let session = session(
            "completed",
            "completed",
            serde_json::Value::Array(vec![status("item", "Item", "accepted")]),
        );
        let tree = crate::detail_tree::project(&session, &ActivityOverlay::default(), false);
        let list = FrameList::from_tree(&tree);
        assert!((0..list.rows().len()).all(|index| !list.is_bright(index)));
    }

    #[test]
    fn live_right_side_omits_a_none_round_and_an_absent_role_with_no_dangling_separator() {
        let mut json = session(
            "awaiting-agent-output",
            "running",
            serde_json::Value::Array(vec![status("item", "Item", "ready")]),
        );
        json.active_path = vec![ctx_traits_core::procedure::runtime::PathSegment {
            kind: "procedure".to_string(),
            id: Some("item".to_string()),
            index: 0,
            iteration: None,
            item_index: None,
        }];
        let tree = crate::detail_tree::project(&json, &ActivityOverlay::default(), true);
        let list = FrameList::from_tree(&tree);
        match &list.rows()[0].right {
            RightSide::Live { round, role, .. } => {
                assert_eq!(*round, None);
                assert_eq!(*role, None);
            }
            other => panic!("expected Live, got {other:?}"),
        }
    }

    #[test]
    fn a_some_round_and_role_carry_through_onto_the_live_right_side() {
        let mut json = session(
            "awaiting-agent-output",
            "running",
            serde_json::Value::Array(vec![status("item", "Item", "ready")]),
        );
        json.active_path = vec![ctx_traits_core::procedure::runtime::PathSegment {
            kind: "procedure".to_string(),
            id: Some("item".to_string()),
            index: 0,
            iteration: None,
            item_index: None,
        }];
        json.current_agent = Some(ctx_traits_core::procedure::runtime::AgentRole {
            role: "plan".to_string(),
            ref_text: String::new(),
            description: String::new(),
            summary: None,
            system: None,
            structural_seat: None,
        });
        // Two accepted revisions of the same `-verdict`-suffixed slot: the
        // exact shape `verdict_slot_rounds` (`stats.rs:113-122`) counts,
        // proving the round is a real, non-zero derived value rather than
        // an untested `None`/absent field.
        json.slot_revisions = vec![
            verdict_revision("review-verdict"),
            verdict_revision("review-verdict"),
        ];
        let tree = crate::detail_tree::project(&json, &ActivityOverlay::default(), true);
        let list = FrameList::from_tree(&tree);
        match &list.rows()[0].right {
            RightSide::Live { round, role, .. } => {
                assert_eq!(*round, Some(2), "two accepted verdict revisions = round 2");
                assert_eq!(role.as_deref(), Some("plan"));
            }
            other => panic!("expected Live, got {other:?}"),
        }
    }

    fn verdict_revision(slot_id: &str) -> ctx_traits_core::procedure::runtime::SlotRevision {
        use ctx_traits_core::procedure::runtime::SlotRevision;
        use ctx_traits_core::reference::{Kind, Reference};
        SlotRevision {
            slot_ref: Reference::local(Kind::Slot, slot_id).expect("valid slot ref"),
            value_digest: ctx_traits_core::digest::Digest::source(slot_id),
            acceptance_order: 0,
            operation: None,
            submitted_payload: None,
            prior_value_digest: None,
            prior_value: None,
            source: None,
            command_execution: None,
            runtime_binding: false,
            projection: None,
            position_path: Vec::new(),
            loop_id: None,
            iteration_index: None,
            for_each_id: None,
            item_index: None,
        }
    }

    #[test]
    fn description_resolves_through_the_placeholders_module() {
        let mut json = session(
            "awaiting-agent-output",
            "running",
            serde_json::Value::Array(vec![status("item", "Item", "ready")]),
        );
        json.active_path = vec![ctx_traits_core::procedure::runtime::PathSegment {
            kind: "procedure".to_string(),
            id: Some("item".to_string()),
            index: 0,
            iteration: None,
            item_index: None,
        }];
        let tree = crate::detail_tree::project(&json, &ActivityOverlay::default(), true);
        let list = FrameList::from_tree(&tree);
        assert_eq!(
            list.rows()[0].description.as_deref(),
            Some(placeholders::CURRENT_FRAME_INTENT.label)
        );
    }

    #[test]
    fn a_done_frame_carries_its_span_through_as_elapsed() {
        use ctx_traits_core::procedure::activity::{ActivityEvent, ActivityKind};
        use ctx_traits_io::activity_sidecar::ActivityRecord;

        let session = session(
            "completed",
            "completed",
            serde_json::Value::Array(vec![status("item", "Item", "accepted")]),
        );
        let overlay = ActivityOverlay::from_records(&[
            ActivityRecord::Activity {
                at_epoch_ms: 1_000,
                event: ActivityEvent {
                    sequence: 1,
                    frame_id: "item".to_string(),
                    kind: ActivityKind::Thinking,
                    text: None,
                    tool: None,
                    tokens: None,
                    rate_limit: None,
                },
            },
            ActivityRecord::Activity {
                at_epoch_ms: 3_000,
                event: ActivityEvent {
                    sequence: 2,
                    frame_id: "item".to_string(),
                    kind: ActivityKind::Thinking,
                    text: None,
                    tool: None,
                    tokens: None,
                    rate_limit: None,
                },
            },
        ]);
        let tree = crate::detail_tree::project(&session, &overlay, false);
        let list = FrameList::from_tree(&tree);
        assert_eq!(
            list.rows()[0].right,
            RightSide::Elapsed(Some(Duration::from_millis(2_000)))
        );
    }

    #[test]
    fn a_structural_group_row_has_no_dot_and_no_right_side() {
        let mut statuses = vec![status("the-loop", "The loop", "pending")];
        let loop_child = serde_json::json!({
            "sequence-index": 1,
            "run-index": 1,
            "item-id": "child",
            "title": "Child",
            "status": "accepted",
            "reason": "",
            "position-path": [
                {"kind": "procedure", "id": "the-loop", "index": 0},
                {"kind": "loop", "id": "the-loop-body", "index": 0, "iteration": 0},
                {"kind": "item", "id": "child", "index": 0, "iteration": 0},
            ],
        });
        statuses.push(loop_child);
        let session = session("completed", "completed", serde_json::Value::Array(statuses));
        let tree = crate::detail_tree::project(&session, &ActivityOverlay::default(), false);
        let list = FrameList::from_tree(&tree);
        let group_row = list
            .rows()
            .iter()
            .find(|row| row.form == RowForm::Group)
            .expect("an iteration group row exists");
        assert_eq!(group_row.dot, None);
        assert_eq!(group_row.right, RightSide::None);
    }
}
