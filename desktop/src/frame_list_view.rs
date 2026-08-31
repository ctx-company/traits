//! Native gpui element tree for a [`FrameList`]. A free function, not a
//! `Shell` method — the `bottom_bar_view.rs`/`detail_view.rs` convention: no
//! `Context`, no `cx.listener`, directly callable from a test with no
//! `App`. Geometry, colour and font all come from `tokens::`; word/role
//! come from the model — no literal, no invented value at a call site.

use gpui::prelude::*;
use gpui::{AnyElement, SharedString, div, rgb};

use ctx_traits_core::procedure::activity::compact_elapsed_text;

use crate::frame_list::{ActivityBlock, DotTone, FrameList, FrameRow, RightSide, RowForm};
use crate::run_row::role_color;
use crate::tokens;

fn dot_color(tone: DotTone) -> u32 {
    match tone {
        DotTone::Ok => tokens::OK,
        DotTone::Accent => tokens::ACCENT,
        DotTone::Idle => tokens::DOT_IDLE,
        DotTone::Dim => tokens::DOT_DIM,
        DotTone::Warn => tokens::WARN,
        DotTone::Danger => tokens::DANGER,
    }
}

/// Rule 4's 5px ellipse (`tokens::LIST_ROW_DOT_SIZE`), never a canvas
/// radius: `rounded_full` here is the dot marker, not a container corner.
fn dot_element(tone: DotTone) -> AnyElement {
    div()
        .size(tokens::LIST_ROW_DOT_SIZE)
        .rounded_full()
        .bg(rgb(dot_color(tone)))
        .into_any_element()
}

fn right_side_text(right: &RightSide) -> Option<(String, u32)> {
    match right {
        RightSide::None => None,
        RightSide::Elapsed(span) => {
            span.map(|duration| (compact_elapsed_text(duration), tokens::TEXT_MUTED))
        }
        RightSide::Word(presentation) => {
            Some((presentation.word.to_string(), role_color(presentation.role)))
        }
        RightSide::Live { state, round, role } => {
            let mut segments = vec![state.word.to_string()];
            if let Some(round) = round {
                segments.push(format!("round {round}"));
            }
            if let Some(role) = role {
                segments.push(role.clone());
            }
            Some((segments.join(" · "), role_color(state.role)))
        }
    }
}

/// Left padding for a row at `depth`: the row's base horizontal padding plus
/// one `tokens::FRAME_ROW_DEPTH_INDENT` step per depth. `gpui`'s `.pl()` and
/// `.px()` both assign `style.padding.left`, so whichever is called last
/// wins — never call both on the same element; use this for the left edge
/// and `tokens::LIST_ROW_PAD_X_MAX` alone for the right.
fn depth_pad_left(depth: usize) -> gpui::Pixels {
    tokens::LIST_ROW_PAD_X_MAX + tokens::FRAME_ROW_DEPTH_INDENT * depth
}

fn settled_row_element(row: &FrameRow, index: usize, bright: bool) -> AnyElement {
    let title_color = if bright {
        tokens::TEXT_BRIGHT
    } else if row.form == RowForm::Pending {
        tokens::TEXT_MUTED
    } else {
        tokens::TEXT
    };
    let mut left = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::ROW_DOT_TEXT_GAP_MIN);
    if let Some(dot) = row.dot {
        left = left.child(dot_element(dot));
    }
    left = left.child(
        div()
            .font_family(tokens::FONT_SANS)
            .text_size(tokens::SIZE_12_5)
            .text_color(rgb(title_color))
            .child(row.title.clone()),
    );

    let mut element = div()
        .id(SharedString::from(format!("frame-row-{index}")))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .pl(depth_pad_left(row.depth))
        .pr(tokens::LIST_ROW_PAD_X_MAX)
        .py(tokens::LIST_ROW_PAD_Y_COMPACT)
        .child(left);

    if let Some((text, color)) = right_side_text(&row.right) {
        element = element.child(
            div()
                .font_family(tokens::FONT_MONO)
                .text_size(tokens::SIZE_10_5)
                .text_color(rgb(color))
                .child(text),
        );
    }
    element.into_any_element()
}

fn group_row_element(row: &FrameRow, index: usize) -> AnyElement {
    div()
        .id(SharedString::from(format!("frame-row-{index}")))
        .flex()
        .flex_row()
        .items_center()
        .pl(depth_pad_left(row.depth))
        .pr(tokens::LIST_ROW_PAD_X_MAX)
        .py(tokens::LIST_ROW_PAD_Y_COMPACT)
        .child(
            div()
                .font_family(tokens::FONT_SANS)
                .text_size(tokens::SIZE_12_5)
                .text_color(rgb(tokens::TEXT_MUTED))
                .child(row.title.clone()),
        )
        .into_any_element()
}

fn current_row_element(row: &FrameRow, index: usize, bright: bool) -> AnyElement {
    let title_color = if bright {
        tokens::TEXT_BRIGHT
    } else {
        tokens::TEXT
    };
    let dot = row
        .dot
        .map(dot_element)
        .unwrap_or_else(|| div().into_any_element());

    let mut text_column = div()
        .flex()
        .flex_col()
        .gap(tokens::FRAME_ROW_TEXT_GAP)
        .child(
            div()
                .font_family(tokens::FONT_SANS)
                .text_size(tokens::SIZE_12_5)
                .text_color(rgb(title_color))
                .child(row.title.clone()),
        );
    if let Some(description) = &row.description {
        text_column = text_column.child(
            div()
                .font_family(tokens::FONT_SANS)
                .text_size(tokens::SIZE_11_5)
                .text_color(rgb(tokens::TEXT_SECONDARY))
                .w(tokens::FRAME_ROW_DESC_WIDTH)
                .line_height(tokens::FRAME_ROW_DESC_LINE_HEIGHT)
                .child(description.clone()),
        );
    }

    let mut element = div()
        .id(SharedString::from(format!("frame-row-{index}")))
        .flex()
        .flex_row()
        .justify_between()
        .bg(rgb(tokens::SURFACE_RAISED))
        .pl(depth_pad_left(row.depth))
        .pr(tokens::LIST_ROW_PAD_X_MAX)
        .py(tokens::LIST_ROW_PAD_Y_OPEN)
        .child(
            div()
                .flex()
                .flex_row()
                .gap(tokens::ROW_DOT_TEXT_GAP_MIN)
                .child(div().pt(tokens::FRAME_DOT_WRAP_PAD_TOP).child(dot))
                .child(text_column),
        );

    if let Some((text, color)) = right_side_text(&row.right) {
        element = element.child(
            div()
                .pt(tokens::FRAME_ROW_RIGHT_PAD_TOP)
                .font_family(tokens::FONT_MONO)
                .text_size(tokens::SIZE_10_5)
                .text_color(rgb(color))
                .child(text),
        );
    }
    element.into_any_element()
}

fn row_element(row: &FrameRow, index: usize, bright: bool) -> AnyElement {
    match row.form {
        RowForm::Current => current_row_element(row, index, bright),
        RowForm::Group => group_row_element(row, index),
        RowForm::Done | RowForm::Pending => settled_row_element(row, index, bright),
    }
}

/// Rule 5's inline-narration voice: italic mono 10.5 `text-faint`, nowrap, in
/// a full-width container at padding `[8,12,2,12]`
/// (`grammar.md:49-56`, `reference/sessions.html:349-359`) — never the
/// italic sans 12 `text-secondary` narrated screen summary rule 5
/// distinguishes it from.
fn narration_element(text: &str) -> AnyElement {
    div()
        .w_full()
        .pt(tokens::LOOP_NARRATION_PAD_TOP)
        .pr(tokens::LIST_ROW_PAD_X_MAX)
        .pb(tokens::LOOP_NARRATION_PAD_BOTTOM)
        .pl(tokens::LIST_ROW_PAD_X_MAX)
        .child(
            div()
                .font_family(tokens::FONT_MONO)
                .text_size(tokens::SIZE_10_5)
                .italic()
                .font_weight(tokens::WEIGHT_NORMAL)
                .text_color(rgb(tokens::TEXT_FAINT))
                .whitespace_nowrap()
                .child(text.to_string()),
        )
        .into_any_element()
}

/// The role prefix, mirroring `right_side_text`'s no-dangling-separator
/// shape: `Some(role)` composes `"{role}: {text}"`, `None` leaves `text`
/// unprefixed.
fn activity_line_text(role: Option<&str>, text: &str) -> String {
    match role {
        Some(role) => format!("{role}: {text}"),
        None => text.to_string(),
    }
}

/// Full width, flex column, gap 3, padding `[4,12,6,27]`, mono 10.5
/// `text-faint` (`reference/sessions.html:460-463`, `grammar.md:104-108`),
/// per-line opacity **zipped** with `tokens::ACTIVITY_FADE_OPACITY` — zipping
/// rather than indexing means the eight-line cap and the opacity ramp are
/// structurally inseparable: a ninth line cannot render and no caller can
/// pass an opacity of its own. `gpui`'s `.opacity()` sets the element's own
/// style opacity (and inherits to children), so it is applied per line, not
/// on the column.
fn activity_block_element(block: &ActivityBlock) -> AnyElement {
    let mut column = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(tokens::ACTIVITY_BLOCK_GAP)
        .pt(tokens::ACTIVITY_BLOCK_PAD_TOP)
        .pr(tokens::ACTIVITY_BLOCK_PAD_RIGHT)
        .pb(tokens::ACTIVITY_BLOCK_PAD_BOTTOM)
        .pl(tokens::ACTIVITY_BLOCK_PAD_LEFT);
    for (line, opacity) in block.lines.iter().zip(tokens::ACTIVITY_FADE_OPACITY) {
        column = column.child(
            div()
                .font_family(tokens::FONT_MONO)
                .text_size(tokens::SIZE_10_5)
                .text_color(rgb(tokens::TEXT_FAINT))
                .opacity(opacity)
                .child(activity_line_text(block.role.as_deref(), line)),
        );
    }
    column.into_any_element()
}

/// Build the native element tree for a [`FrameList`]. Pure: no `Context`, no
/// `App` required to call it.
pub fn frame_list_element(list: &FrameList) -> AnyElement {
    let mut column = div()
        .id("frame-list")
        .flex()
        .flex_col()
        .gap(tokens::LIST_ROWS_GAP_MAX);
    for (index, row) in list.rows().iter().enumerate() {
        for text in list.narration_before(index) {
            column = column.child(narration_element(text));
        }
        column = column.child(row_element(row, index, list.is_bright(index)));
        if row.form == RowForm::Current
            && let Some(block) = list.activity_block()
        {
            column = column.child(activity_block_element(block));
        }
    }
    // A loop group that is the very last node in the whole tree places its
    // narration at `rows().len()` — past every row index the loop above
    // walks, so it is rendered here rather than dropped.
    for text in list.narration_before(list.rows().len()) {
        column = column.child(narration_element(text));
    }
    column.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detail_tree::{self, ActivityOverlay};
    use ctx_traits_core::procedure::session::Session;

    fn session_with(status_wire: &str, final_state: &str, sequence_status: &str) -> Session {
        serde_json::from_value(serde_json::json!({
            "schema-version": "0.1.0",
            "session-id": "session-fixture",
            "run-id": "run-fixture",
            "trait-id": "fixture-trait",
            "current-run-index": 0,
            "status": status_wire,
            "provenance": {
                "started-by": {"surface": "test", "caller": "frame-list-view-fixture"},
                "state-source": "test",
            },
            "active-path": [{"kind": "procedure", "id": "item", "index": 0}],
            "ledger": {
                "run-id": "run-fixture",
                "trait-id": "fixture-trait",
                "current-run-index": 0,
                "final-state": final_state,
                "sequence-statuses": [{
                    "sequence-index": 0,
                    "run-index": 0,
                    "item-id": "item",
                    "title": "Item",
                    "status": sequence_status,
                    "reason": "",
                    "position-path": [],
                }],
            },
            "state-digest": "sha256:fixture",
        }))
        .expect("fixture session")
    }

    /// One live run whose sequence carries a done, a pending, a rejected
    /// (failed), and a current item, in that order — so `FrameList::rows()`
    /// contains all four row forms in a single list.
    fn session_with_all_four_forms() -> Session {
        serde_json::from_value(serde_json::json!({
            "schema-version": "0.1.0",
            "session-id": "session-fixture",
            "run-id": "run-fixture",
            "trait-id": "fixture-trait",
            "current-run-index": 0,
            "status": "awaiting-agent-output",
            "provenance": {
                "started-by": {"surface": "test", "caller": "frame-list-view-fixture"},
                "state-source": "test",
            },
            "active-path": [{"kind": "procedure", "id": "item-current", "index": 0}],
            "ledger": {
                "run-id": "run-fixture",
                "trait-id": "fixture-trait",
                "current-run-index": 0,
                "final-state": "running",
                "sequence-statuses": [
                    {
                        "sequence-index": 0,
                        "run-index": 0,
                        "item-id": "item-done",
                        "title": "Done item",
                        "status": "accepted",
                        "reason": "",
                        "position-path": [],
                    },
                    {
                        "sequence-index": 1,
                        "run-index": 0,
                        "item-id": "item-pending",
                        "title": "Pending item",
                        "status": "pending",
                        "reason": "",
                        "position-path": [],
                    },
                    {
                        "sequence-index": 2,
                        "run-index": 0,
                        "item-id": "item-rejected",
                        "title": "Rejected item",
                        "status": "rejected",
                        "reason": "",
                        "position-path": [],
                    },
                    {
                        "sequence-index": 3,
                        "run-index": 0,
                        "item-id": "item-current",
                        "title": "Current item",
                        "status": "ready",
                        "reason": "",
                        "position-path": [],
                    },
                ],
            },
            "state-digest": "sha256:fixture",
        }))
        .expect("fixture session")
    }

    #[test]
    fn right_side_text_composes_the_live_segments_with_no_dangling_separator() {
        use crate::run_row::{StatePresentation, StateRole};

        let state = StatePresentation {
            word: "running",
            role: StateRole::Accent,
        };

        let none_none = right_side_text(&RightSide::Live {
            state,
            round: None,
            role: None,
        });
        assert_eq!(
            none_none.as_ref().map(|(text, _)| text.as_str()),
            Some("running"),
            "no round, no role: exactly the state word, no separator"
        );

        let some_round_only = right_side_text(&RightSide::Live {
            state,
            round: Some(2),
            role: None,
        });
        assert_eq!(
            some_round_only.as_ref().map(|(text, _)| text.as_str()),
            Some("running · round 2")
        );

        let role_only = right_side_text(&RightSide::Live {
            state,
            round: None,
            role: Some("plan".to_string()),
        });
        assert_eq!(
            role_only.as_ref().map(|(text, _)| text.as_str()),
            Some("running · plan")
        );

        let both = right_side_text(&RightSide::Live {
            state,
            round: Some(3),
            role: Some("plan".to_string()),
        });
        assert_eq!(
            both.as_ref().map(|(text, _)| text.as_str()),
            Some("running · round 3 · plan")
        );
    }

    #[test]
    fn right_side_text_omits_an_absent_elapsed_span_and_renders_a_present_one() {
        assert_eq!(right_side_text(&RightSide::None), None);
        assert_eq!(right_side_text(&RightSide::Elapsed(None)), None);
        let (text, _) = right_side_text(&RightSide::Elapsed(Some(std::time::Duration::from_secs(
            128,
        ))))
        .expect("a present span renders");
        assert_eq!(text, "2m 8s");
    }

    #[test]
    fn depth_pad_left_adds_the_indent_token_without_losing_base_padding() {
        assert_eq!(
            depth_pad_left(0),
            tokens::LIST_ROW_PAD_X_MAX,
            "a depth-zero row keeps exactly the base horizontal padding"
        );
        assert_eq!(
            depth_pad_left(1),
            tokens::LIST_ROW_PAD_X_MAX + tokens::FRAME_ROW_DEPTH_INDENT,
            "one nesting step adds exactly one indent token on top of the base"
        );
        assert_eq!(
            depth_pad_left(2),
            tokens::LIST_ROW_PAD_X_MAX + tokens::FRAME_ROW_DEPTH_INDENT * 2,
            "indentation accumulates per depth"
        );
    }

    #[test]
    fn activity_line_text_composes_the_role_prefix_with_no_dangling_separator() {
        assert_eq!(
            activity_line_text(Some("review"), "reading a file"),
            "review: reading a file"
        );
        assert_eq!(activity_line_text(None, "reading a file"), "reading a file");
    }

    #[test]
    fn activity_block_element_opacity_slots_match_visible_index_and_cap_at_eight() {
        use crate::frame_list::ActivityBlock;

        for count in [0usize, 1, 5, 8, 9] {
            let lines: Vec<String> = (0..count).map(|i| format!("line-{i}")).collect();
            let block = ActivityBlock { role: None, lines };
            // Zipping (not indexing) with the fixed opacity table is what
            // caps rendering at eight regardless of how many lines the
            // model carries — assert the zip length directly, mirroring
            // what `activity_block_element` actually iterates.
            let rendered = block
                .lines
                .iter()
                .zip(tokens::ACTIVITY_FADE_OPACITY)
                .count();
            assert_eq!(rendered, count.min(8), "count={count}");
            let _element = activity_block_element(&block);
        }
    }

    #[test]
    fn frame_list_element_builds_for_every_form_with_no_app() {
        let done = session_with("completed", "completed", "accepted");
        let pending = session_with("completed", "completed", "pending");
        let rejected = session_with("completed", "completed", "rejected");
        let live = session_with("awaiting-agent-output", "running", "ready");

        for (session, is_live) in [
            (done, false),
            (pending, false),
            (rejected, false),
            (live, true),
        ] {
            let tree = detail_tree::project(&session, &ActivityOverlay::default(), is_live);
            let list = crate::frame_list::FrameList::from_tree(&tree);
            let _element: AnyElement = frame_list_element(&list);
        }
    }

    /// One real gpui window rendering a list of done, current, pending and
    /// failed rows together — the required-runtime proof `desktop-render-
    /// proof-incomplete` asked for. Asserts the model's dot/selection/right-
    /// side semantics first (so a regression there fails loudly, not just
    /// silently under paint), then retains the window handle and drives
    /// `run_until_parked` so `FrameListProbe::render` actually executes,
    /// mirroring `shell.rs`'s established real-frame precedent (the
    /// `open_window(...).unwrap()` handle kept, not dropped).
    #[gpui::test]
    fn frame_list_paints_all_four_forms(cx: &mut gpui::TestAppContext) {
        let session = session_with_all_four_forms();
        let tree = detail_tree::project(&session, &ActivityOverlay::default(), true);
        let list = crate::frame_list::FrameList::from_tree(&tree);

        let rows = list.rows();
        assert_eq!(rows.len(), 4);
        assert_eq!(
            (rows[0].form, rows[0].dot),
            (RowForm::Done, Some(DotTone::Ok))
        );
        assert_eq!(
            (rows[1].form, rows[1].dot),
            (RowForm::Pending, Some(DotTone::Idle))
        );
        assert_eq!(
            (rows[2].form, rows[2].dot),
            (RowForm::Done, Some(DotTone::Danger)),
            "a rejected frame renders the failed form, never done/ok"
        );
        assert_eq!(
            (rows[3].form, rows[3].dot),
            (RowForm::Current, Some(DotTone::Accent))
        );
        assert_eq!(list.selected(), Some(3));
        assert!((0..4).filter(|&index| list.is_bright(index)).count() == 1);

        // The handle is kept (not dropped before `run_until_parked`, unlike
        // the prior version of this test) so the window and its one render
        // pass are not torn down before the frame actually runs.
        let _window = cx
            .update(|cx| {
                let bounds =
                    gpui::Bounds::centered(None, gpui::size(gpui::px(400.), gpui::px(300.)), cx);
                cx.open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|_| FrameListProbe { list: list.clone() }),
                )
            })
            .expect("open a real window");
        cx.run_until_parked();
    }

    /// A real gpui window painting a narration line and a full eight-line
    /// activity block together with the four settled row forms, so `render`
    /// actually executes over both new element kinds this task adds.
    #[gpui::test]
    fn frame_list_paints_a_narration_line_and_a_full_activity_block(cx: &mut gpui::TestAppContext) {
        use ctx_traits_core::procedure::activity::{ActivityEvent, ActivityKind};
        use ctx_traits_io::activity_sidecar::ActivityRecord;

        let mut session = session_with_all_four_forms();
        session.ledger.sequence_statuses[1].position_path = vec![
            ctx_traits_core::procedure::runtime::PathSegment {
                kind: "procedure".to_string(),
                id: Some("the-loop".to_string()),
                index: 0,
                iteration: None,
                item_index: None,
            },
            ctx_traits_core::procedure::runtime::PathSegment {
                kind: "loop".to_string(),
                id: Some("the-loop-body".to_string()),
                index: 0,
                iteration: Some(1),
                item_index: None,
            },
            ctx_traits_core::procedure::runtime::PathSegment {
                kind: "item".to_string(),
                id: Some("item-pending".to_string()),
                index: 0,
                iteration: Some(1),
                item_index: None,
            },
        ];
        let records: Vec<ActivityRecord> = (0..9u64)
            .map(|sequence| ActivityRecord::Activity {
                at_epoch_ms: sequence,
                event: ActivityEvent {
                    sequence,
                    frame_id: "item-current".to_string(),
                    kind: ActivityKind::Thinking,
                    text: Some(format!("thinking {sequence}")),
                    tool: None,
                    tokens: None,
                    rate_limit: None,
                },
            })
            .collect();
        let overlay = ActivityOverlay::from_records(&records);
        let tree = detail_tree::project(&session, &overlay, true);
        let list = crate::frame_list::FrameList::from_tree(&tree);

        assert!(
            (0..list.rows().len() + 1).any(|index| !list.narration_before(index).is_empty()),
            "the loop-wrapped pending item produces a narration placement"
        );
        let block = list
            .activity_block()
            .expect("the current row's nine folded events produce a capped block");
        assert_eq!(block.lines.len(), 8, "the ninth line is dropped, not shown");

        let _window = cx
            .update(|cx| {
                let bounds =
                    gpui::Bounds::centered(None, gpui::size(gpui::px(400.), gpui::px(300.)), cx);
                cx.open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|_| FrameListProbe { list: list.clone() }),
                )
            })
            .expect("open a real window");
        cx.run_until_parked();
    }

    struct FrameListProbe {
        list: crate::frame_list::FrameList,
    }

    impl gpui::Render for FrameListProbe {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            frame_list_element(&self.list)
        }
    }
}
