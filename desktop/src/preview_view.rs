//! Native gpui element tree for the Sessions preview column: the frame, the
//! `run` identity block, and the footer. Free functions, no `Context`, no
//! `cx.listener` — mirrors `bottom_bar_view.rs`'s/`detail_view.rs`'s
//! convention so the whole render path is directly callable from a test
//! with no gpui `App`.

use gpui::prelude::*;
use gpui::{AnyElement, div, rgb};

use crate::preview::{KeyValueRow, NamedBlock, ValueSegment};
use crate::run_row::role_color;
use crate::tokens;

fn segment_color(segment: &ValueSegment) -> u32 {
    match segment.role {
        Some(role) => role_color(role),
        None => tokens::TEXT,
    }
}

pub fn kv_row_element(row: &KeyValueRow) -> AnyElement {
    let mut value = div()
        .debug_selector(|| format!("preview-row-{}-value", row.key))
        .flex()
        .flex_row()
        .font_family(tokens::FONT_MONO)
        .text_size(tokens::SIZE_10_5);
    for segment in &row.value {
        value = value.child(
            div()
                .text_color(rgb(segment_color(segment)))
                .child(segment.text.clone()),
        );
    }
    div()
        .debug_selector(|| format!("preview-row-{}", row.key))
        .w_full()
        .flex()
        .flex_row()
        .justify_between()
        .items_center()
        .child(
            div()
                .debug_selector(|| format!("preview-row-{}-key", row.key))
                .font_family(tokens::FONT_SANS)
                .text_size(tokens::SIZE_11_5)
                .text_color(rgb(tokens::TEXT_SECONDARY))
                .child(row.key.clone()),
        )
        .child(value)
        .into_any_element()
}

pub fn named_block_element(block: &NamedBlock) -> AnyElement {
    let mut element = div()
        .debug_selector(|| "preview-run-block".to_string())
        .flex()
        .flex_col()
        .gap(tokens::BLOCK_GAP)
        .pb(tokens::BLOCK_BOTTOM_PADDING)
        .w_full()
        .child(
            div()
                .debug_selector(|| "preview-run-heading".to_string())
                .font_family(tokens::FONT_MONO)
                .text_size(tokens::SIZE_11)
                .text_color(rgb(tokens::TEXT_MUTED))
                .child(block.heading.clone()),
        );
    for row in &block.rows {
        element = element.child(kv_row_element(row));
    }
    element.into_any_element()
}

pub fn preview_footer_element(text: &str) -> AnyElement {
    div()
        .debug_selector(|| "preview-footer".to_string())
        .w_full()
        .flex()
        .flex_row()
        .justify_end()
        .font_family(tokens::FONT_MONO)
        .text_size(tokens::SIZE_10)
        .text_color(rgb(tokens::TEXT_FAINT))
        .child(text.to_string())
        .into_any_element()
}

/// The reusable preview column frame: an ordered list of body blocks, then a
/// `flex: 1 1 0` spacer, then the footer as the last child. `body` is a
/// `Vec<AnyElement>` (not `Vec<NamedBlock>`) so `0265.11`/`.12`/`.15`'s
/// non-key/value blocks insert between the identity block and the spacer
/// without forking this frame.
pub fn preview_column_element(body: Vec<AnyElement>, footer: Option<AnyElement>) -> AnyElement {
    let mut column = div()
        .debug_selector(|| "preview-column".to_string())
        .w(tokens::PREVIEW_COLUMN_WIDTH)
        .h_full()
        .py(tokens::PREVIEW_COLUMN_PAD_Y)
        .px(tokens::PREVIEW_COLUMN_PAD_X)
        .flex()
        .flex_col();
    for child in body {
        column = column.child(child);
    }
    column = column.child(
        div()
            .debug_selector(|| "preview-spacer".to_string())
            .flex_1()
            .w_full(),
    );
    if let Some(footer) = footer {
        column = column.child(footer);
    }
    column.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::{KeyValueRow, NamedBlock, ValueSegment, sessions_run_block};
    use gpui::{Bounds, Context, Render, Window, px, size};

    #[test]
    fn preview_elements_build_from_a_projected_block_with_no_app() {
        let block = sessions_run_block(None);
        let block_element = named_block_element(&block);
        let footer_element = preview_footer_element("run-1");
        let _element: AnyElement =
            preview_column_element(vec![block_element], Some(footer_element));
    }

    fn fixture_block() -> NamedBlock {
        NamedBlock {
            heading: "run".to_string(),
            rows: vec![
                KeyValueRow {
                    key: "trait".to_string(),
                    value: vec![ValueSegment {
                        text: "implement-phase".to_string(),
                        role: None,
                    }],
                },
                KeyValueRow {
                    key: "run".to_string(),
                    value: vec![ValueSegment {
                        text: "run-1".to_string(),
                        role: None,
                    }],
                },
                KeyValueRow {
                    key: "task".to_string(),
                    value: vec![ValueSegment {
                        text: "0265.10".to_string(),
                        role: None,
                    }],
                },
            ],
        }
    }

    /// A minimal `Render` root — mirrors `Shell`'s pattern but paints only
    /// the preview column, with an extra probe body element so the frame's
    /// "ordered slot" contract (an inserted block sits between the identity
    /// block and the spacer, footer stays last) is provable without a full
    /// `Shell`/center harness.
    struct PreviewHarness {
        extra: bool,
    }

    impl Render for PreviewHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let mut body = vec![named_block_element(&fixture_block())];
            if self.extra {
                body.push(
                    div()
                        .debug_selector(|| "preview-probe".to_string())
                        .w_full()
                        .h(px(10.))
                        .into_any_element(),
                );
            }
            let footer = preview_footer_element("run-1 \u{b7} started 13:44");
            preview_column_element(body, Some(footer))
        }
    }

    fn open_harness(
        cx: &mut gpui::TestAppContext,
        extra: bool,
    ) -> gpui::WindowHandle<PreviewHarness> {
        let window = cx
            .update(|cx| {
                let bounds = Bounds::new(gpui::point(px(0.), px(0.)), size(px(900.), px(700.)));
                cx.open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|_| PreviewHarness { extra }),
                )
            })
            .unwrap();
        cx.run_until_parked();
        window
    }

    #[gpui::test]
    fn preview_column_paints_full_height_380_wide_with_inset_content(
        cx: &mut gpui::TestAppContext,
    ) {
        let window = open_harness(cx, false);
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);

        let column = vcx
            .debug_bounds("preview-column")
            .expect("the preview column actually painted this frame");
        assert_eq!(column.size.width, tokens::PREVIEW_COLUMN_WIDTH);
        assert_eq!(column.size.height, px(700.));

        let block = vcx
            .debug_bounds("preview-run-block")
            .expect("the run block actually painted this frame");
        assert_eq!(
            block.origin.x - column.origin.x,
            tokens::PREVIEW_COLUMN_PAD_X,
            "the block's leading edge sits at the column's inner left inset"
        );
        assert_eq!(
            block.origin.y - column.origin.y,
            tokens::PREVIEW_COLUMN_PAD_Y,
            "the block's top edge sits at the column's inner top inset"
        );
    }

    #[gpui::test]
    fn preview_frame_orders_identity_block_then_spacer_then_footer_last(
        cx: &mut gpui::TestAppContext,
    ) {
        let window = open_harness(cx, false);
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);

        let column = vcx.debug_bounds("preview-column").expect("column painted");
        let block = vcx
            .debug_bounds("preview-run-block")
            .expect("block painted");
        let spacer = vcx.debug_bounds("preview-spacer").expect("spacer painted");
        let footer = vcx.debug_bounds("preview-footer").expect("footer painted");

        assert!(
            spacer.origin.y >= block.origin.y + block.size.height,
            "the spacer follows the identity block"
        );
        assert!(
            footer.origin.y >= spacer.origin.y + spacer.size.height,
            "the footer follows the spacer"
        );
        let inner_bottom = column.origin.y + column.size.height - tokens::PREVIEW_COLUMN_PAD_Y;
        assert!(
            (footer.origin.y + footer.size.height - inner_bottom).abs() <= px(1.),
            "the footer's bottom edge sits at the column's inner bottom edge"
        );
    }

    #[gpui::test]
    fn an_extra_body_block_inserts_between_identity_block_and_spacer(
        cx: &mut gpui::TestAppContext,
    ) {
        let window = open_harness(cx, true);
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);

        let block = vcx
            .debug_bounds("preview-run-block")
            .expect("block painted");
        let probe = vcx.debug_bounds("preview-probe").expect("probe painted");
        let spacer = vcx.debug_bounds("preview-spacer").expect("spacer painted");
        let footer = vcx.debug_bounds("preview-footer").expect("footer painted");

        assert!(
            probe.origin.y >= block.origin.y + block.size.height,
            "the extra block sits after the identity block"
        );
        assert!(
            spacer.origin.y >= probe.origin.y + probe.size.height,
            "the extra block sits before the spacer"
        );
        assert!(
            footer.origin.y >= spacer.origin.y + spacer.size.height,
            "the footer is still last regardless of the extra body slot"
        );
    }

    #[gpui::test]
    fn identity_block_has_heading_above_rows_in_trait_run_task_order(
        cx: &mut gpui::TestAppContext,
    ) {
        let window = open_harness(cx, false);
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);

        let heading = vcx
            .debug_bounds("preview-run-heading")
            .expect("heading painted");
        let trait_row = vcx
            .debug_bounds("preview-row-trait")
            .expect("trait row painted");
        let run_row = vcx
            .debug_bounds("preview-row-run")
            .expect("run row painted");
        let task_row = vcx
            .debug_bounds("preview-row-task")
            .expect("task row painted");

        assert!(heading.origin.y < trait_row.origin.y);
        assert!(trait_row.origin.y < run_row.origin.y);
        assert!(run_row.origin.y < task_row.origin.y);

        let gap = run_row.origin.y - (trait_row.origin.y + trait_row.size.height);
        assert!(
            (gap - tokens::BLOCK_GAP).abs() <= px(1.),
            "rows are separated by the block gap: got {gap:?}"
        );

        let block = vcx
            .debug_bounds("preview-run-block")
            .expect("block painted");
        assert!(
            (block.size.width - trait_row.size.width).abs() <= px(1.),
            "each row reaches the block's full width"
        );

        let bottom_gap =
            block.origin.y + block.size.height - (task_row.origin.y + task_row.size.height);
        assert!(
            (bottom_gap - tokens::BLOCK_BOTTOM_PADDING).abs() <= px(1.),
            "the block's bottom padding sits below the last row: got {bottom_gap:?}"
        );
    }

    #[gpui::test]
    fn spacer_consumes_exactly_the_remainder_between_block_and_footer(
        cx: &mut gpui::TestAppContext,
    ) {
        let window = open_harness(cx, false);
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);

        let column = vcx.debug_bounds("preview-column").expect("column painted");
        let block = vcx
            .debug_bounds("preview-run-block")
            .expect("block painted");
        let spacer = vcx.debug_bounds("preview-spacer").expect("spacer painted");
        let footer = vcx.debug_bounds("preview-footer").expect("footer painted");

        let inner_bottom = column.origin.y + column.size.height - tokens::PREVIEW_COLUMN_PAD_Y;
        let expected_spacer_height = inner_bottom - footer.size.height - spacer.origin.y;
        assert!(
            (spacer.size.height - expected_spacer_height).abs() <= px(1.),
            "the spacer consumes exactly the remainder between the block and the footer: \
             got {:?}, expected {:?}",
            spacer.size.height,
            expected_spacer_height
        );
        assert!(
            (spacer.origin.y - (block.origin.y + block.size.height)).abs() <= px(1.),
            "the spacer starts immediately after the identity block"
        );
    }

    /// Measures the row's `key`/`value` *children* directly, not the
    /// full-width row container — a container reaching the block's edges
    /// proves nothing about whether the key and value are actually pushed
    /// to opposite ends of it. Deleting `.justify_between()` on
    /// `kv_row_element` must make this fail (review-verdict-1 blocker
    /// `rendered-preview-evidence-missing`).
    #[gpui::test]
    fn a_row_key_and_value_reach_the_block_opposite_inner_edges(cx: &mut gpui::TestAppContext) {
        let window = open_harness(cx, false);
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);

        let block = vcx
            .debug_bounds("preview-run-block")
            .expect("block painted");
        let key = vcx
            .debug_bounds("preview-row-trait-key")
            .expect("trait row key painted");
        let value = vcx
            .debug_bounds("preview-row-trait-value")
            .expect("trait row value painted");

        assert!(
            (key.origin.x - block.origin.x).abs() <= px(1.),
            "the row's key edge sits at the block's leading inner edge"
        );
        assert!(
            ((value.origin.x + value.size.width) - (block.origin.x + block.size.width)).abs()
                <= px(1.),
            "the row's value edge sits at the block's trailing inner edge"
        );
        assert!(
            value.origin.x > key.origin.x + key.size.width,
            "the key and value must not overlap or sit adjacent with no gap between them"
        );
    }

    /// A neutral identity/separator segment resolves through `tokens::TEXT`,
    /// never a semantic role color; a state segment resolves through its
    /// own `StateRole` — the mapping `kv_row_element` must never invert
    /// (rule 7, `grammar.md:68-72`).
    #[test]
    fn segment_color_resolves_neutral_to_text_token_and_state_to_its_role() {
        use crate::run_row::StateRole;

        let identity = ValueSegment {
            text: "run-1".to_string(),
            role: None,
        };
        assert_eq!(segment_color(&identity), tokens::TEXT);

        let dot = ValueSegment {
            text: " \u{b7} ".to_string(),
            role: None,
        };
        assert_eq!(segment_color(&dot), tokens::TEXT);

        let state = ValueSegment {
            text: "done".to_string(),
            role: Some(StateRole::Ok),
        };
        assert_eq!(segment_color(&state), role_color(StateRole::Ok));
        assert_ne!(segment_color(&state), tokens::TEXT);
    }
}
