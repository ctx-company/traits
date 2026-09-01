//! Native gpui element tree for the Sessions preview column: the frame, the
//! `run` identity block, and the footer. Free functions, no `Context`, no
//! `cx.listener` — mirrors `bottom_bar_view.rs`'s/`detail_view.rs`'s
//! convention so the whole render path is directly callable from a test
//! with no gpui `App`.

use gpui::prelude::*;
use gpui::{AnyElement, div, rgb};

use crate::preview::{KeyValueRow, LandingBlock, NamedBlock, NowItem, ValueSegment, VerdictBlock};
use crate::run_row::role_color;
use crate::task_preview::ChecksBlock;
use crate::tokens;

pub fn lede_element(lede: &crate::task_preview::Lede) -> AnyElement {
    let mut element = div()
        .debug_selector(|| "tasks-lede".to_string())
        .flex()
        .flex_col()
        .gap(tokens::LEDE_GAP)
        .pb(tokens::LEDE_BOTTOM_PADDING)
        .w_full()
        .child(
            div()
                .debug_selector(|| "tasks-lede-title".to_string())
                .font_family(tokens::FONT_SANS)
                .text_size(tokens::SIZE_13)
                .text_color(rgb(tokens::TEXT_BRIGHT))
                .child(lede.title.clone()),
        );
    if !lede.content.is_empty() {
        element = element.child(
            div()
                .debug_selector(|| "tasks-lede-content".to_string())
                .font_family(tokens::FONT_SANS)
                .text_size(tokens::SIZE_11_5)
                .line_height(tokens::LEDE_LINE_HEIGHT)
                .text_color(rgb(tokens::TEXT_SECONDARY))
                .child(lede.content.clone()),
        );
    }
    element.into_any_element()
}

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

/// The one shell every preview body block renders through: block gap, bottom
/// padding, full width, and the heading typography, with `slug` distinguishing
/// this block's debug selectors from any other block mounted in the same tree
/// (`preview-block-{slug}` / `preview-heading-{slug}`) — with three blocks in
/// one preview column, a hardcoded selector would make `debug_bounds`
/// ambiguous. `named_block_element`, `now_item_element` and
/// `verdict_block_element` all compose their own body children and delegate
/// the container and heading to this — one place owns the block geometry.
fn named_block_shell(slug: &str, heading: &str, children: Vec<AnyElement>) -> AnyElement {
    let slug_owned = slug.to_string();
    let heading_slug = slug.to_string();
    let mut element = div()
        .debug_selector(move || format!("preview-block-{slug_owned}"))
        .flex()
        .flex_col()
        .gap(tokens::BLOCK_GAP)
        .pb(tokens::BLOCK_BOTTOM_PADDING)
        .w_full()
        .child(
            div()
                .debug_selector(move || format!("preview-heading-{heading_slug}"))
                .font_family(tokens::FONT_MONO)
                .text_size(tokens::SIZE_11)
                .text_color(rgb(tokens::TEXT_MUTED))
                .child(heading.to_string()),
        );
    for child in children {
        element = element.child(child);
    }
    element.into_any_element()
}

/// `slug` distinguishes this block's debug selectors from any other block
/// mounted in the same tree (`preview-block-{slug}` /
/// `preview-heading-{slug}`) — with three blocks in one preview column, a
/// hardcoded selector would make `debug_bounds` ambiguous.
pub fn named_block_element(slug: &str, block: &NamedBlock) -> AnyElement {
    let rows = block.rows.iter().map(kv_row_element).collect();
    named_block_shell(slug, &block.heading, rows)
}

/// Checks are a heading followed by plain lines, not empty-key fact rows.
pub fn checks_block_element(block: &ChecksBlock) -> AnyElement {
    let lines = block
        .lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            div()
                .debug_selector(move || format!("preview-check-{index}"))
                .w_full()
                .font_family(tokens::FONT_SANS)
                .text_size(tokens::SIZE_11_5)
                .text_color(rgb(tokens::TEXT_SECONDARY))
                .child(line.clone())
                .into_any_element()
        })
        .collect();
    named_block_shell("checks", &block.heading, lines)
}

/// A named block with a full-width Sans lede below its key/value rows.
pub fn lede_block_element(slug: &str, block: &NamedBlock, lede: Option<&str>) -> AnyElement {
    let mut children: Vec<_> = block.rows.iter().map(kv_row_element).collect();
    if let Some(lede) = lede {
        children.push(
            div()
                .debug_selector(move || format!("preview-lede-{slug}"))
                .w_full()
                .font_family(tokens::FONT_SANS)
                .text_size(tokens::SIZE_11_5)
                .text_color(rgb(tokens::TEXT_SECONDARY))
                .child(lede.to_string())
                .into_any_element(),
        );
    }
    named_block_shell(slug, &block.heading, children)
}

/// The `in progress` block: the reusable named-block heading, then one
/// bordered "now" item — a `space-between` title/state-word row, then a
/// full-width non-italic narration line.
/// The now item's title color: `TEXT_MUTED` for the absence literal,
/// `TEXT` for a served title. Shared by the renderer and its color-
/// resolution test so the two can never diverge.
fn now_item_title_color(item: &NowItem) -> u32 {
    if item.title_muted {
        tokens::TEXT_MUTED
    } else {
        tokens::TEXT
    }
}

pub fn now_item_element(item: &NowItem) -> AnyElement {
    let title_color = now_item_title_color(item);
    let inner = div()
        .debug_selector(|| "preview-now-item".to_string())
        .flex()
        .flex_col()
        .w_full()
        .gap(tokens::NOW_ITEM_GAP)
        .bg(rgb(tokens::ROW_OPEN))
        .border(tokens::NOW_ITEM_BORDER)
        .border_color(rgb(tokens::BORDER_SOFT))
        .py(tokens::BORDERED_BOX_PAD_Y)
        .px(tokens::BORDERED_BOX_PAD_X_MIN)
        .child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .justify_between()
                .items_center()
                .child(
                    div()
                        .debug_selector(|| "preview-now-title".to_string())
                        .font_family(tokens::FONT_SANS)
                        .text_size(tokens::SIZE_12)
                        .text_color(rgb(title_color))
                        .child(item.title.clone()),
                )
                .child(
                    div()
                        .debug_selector(|| "preview-now-marker".to_string())
                        .font_family(tokens::FONT_MONO)
                        .text_size(tokens::SIZE_10_5)
                        .text_color(rgb(role_color(item.state_role)))
                        .child(item.state_word.clone()),
                ),
        )
        .child(
            div()
                .debug_selector(|| "preview-now-narration".to_string())
                .w_full()
                .font_family(tokens::FONT_MONO)
                .text_size(tokens::SIZE_10_5)
                .text_color(rgb(tokens::TEXT_MUTED))
                .line_height(tokens::NOW_NARRATION_LINE_HEIGHT)
                .child(item.narration.clone()),
        );

    named_block_shell("now", "in progress", vec![inner.into_any_element()])
}

/// The `verdict` block: one heading form (mirroring [`named_block_element`]),
/// the one `status` row, then zero or more full-width blocker lines — all as
/// siblings in a single container, the same "one heading form, one row form"
/// invariant `named_block_element` establishes.
pub fn verdict_block_element(verdict: &VerdictBlock) -> AnyElement {
    let mut children = vec![kv_row_element(&verdict.status_row)];
    for (index, line) in verdict.blocker_lines.iter().enumerate() {
        children.push(
            div()
                .debug_selector(move || format!("preview-blocker-{index}"))
                .w_full()
                .font_family(tokens::FONT_MONO)
                .text_size(tokens::SIZE_10_5)
                .text_color(rgb(tokens::TEXT_SECONDARY))
                .child(line.clone())
                .into_any_element(),
        );
    }
    named_block_shell("verdict", &verdict.heading, children)
}

/// One `landing` line: sans `SIZE_12`, `TEXT_SECONDARY` when `role` is
/// `None` (the block's own default, distinct from [`segment_color`]'s
/// identity-value `None`), `role_color` otherwise.
fn landing_line_element(index: usize, line: &crate::preview::LandingLine) -> AnyElement {
    let color = match line.role {
        Some(role) => role_color(role),
        None => tokens::TEXT_SECONDARY,
    };
    div()
        .debug_selector(move || format!("preview-landing-line-{index}"))
        .w_full()
        .font_family(tokens::FONT_SANS)
        .text_size(tokens::SIZE_12)
        .text_color(rgb(color))
        .child(line.text.clone())
        .into_any_element()
}

/// The `landing` block: the reusable named-block heading, then exactly three
/// full-width lines — composed through [`named_block_shell`] unchanged, so
/// heading form, gap and bottom padding are inherited, not re-declared.
pub fn landing_block_element(block: &LandingBlock) -> AnyElement {
    let lines = block
        .lines
        .iter()
        .enumerate()
        .map(|(index, line)| landing_line_element(index, line))
        .collect();
    named_block_shell("landing", &block.heading, lines)
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
        let block_element = named_block_element("run", &block);
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

    fn fixture_now_item() -> NowItem {
        NowItem {
            title: "review \u{b7} 2".to_string(),
            title_muted: false,
            state_word: "running".to_string(),
            state_role: crate::run_row::StateRole::Accent,
            narration: "working on this frame now".to_string(),
        }
    }

    fn fixture_landing_block() -> LandingBlock {
        LandingBlock {
            heading: "landing".to_string(),
            lines: vec![
                crate::preview::LandingLine {
                    text: "\u{2192} Runs in worktree wt-ab12ef".to_string(),
                    role: None,
                },
                crate::preview::LandingLine {
                    text: "\u{2192} Merged at deadbeef".to_string(),
                    role: Some(crate::run_row::StateRole::Ok),
                },
                crate::preview::LandingLine {
                    text: "\u{2192} Task 0243.4 close policy unresolved".to_string(),
                    role: Some(crate::run_row::StateRole::Danger),
                },
            ],
        }
    }

    fn fixture_verdict_block() -> VerdictBlock {
        VerdictBlock {
            heading: "verdict \u{b7} round 1".to_string(),
            status_row: KeyValueRow {
                key: "status".to_string(),
                value: vec![
                    ValueSegment {
                        text: "revise".to_string(),
                        role: Some(crate::run_row::StateRole::Neutral),
                    },
                    ValueSegment {
                        text: " \u{b7} ".to_string(),
                        role: None,
                    },
                    ValueSegment {
                        text: "1 finding".to_string(),
                        role: None,
                    },
                ],
            },
            blocker_lines: vec!["b1 \u{b7} the defect".to_string()],
        }
    }

    /// A minimal `Render` root — mirrors `Shell`'s pattern but paints only
    /// the preview column, with an extra probe body element so the frame's
    /// "ordered slot" contract (an inserted block sits between the identity
    /// block and the spacer, footer stays last) is provable without a full
    /// `Shell`/center harness.
    struct PreviewHarness {
        extra: bool,
        now_and_verdict: bool,
        landing: bool,
    }

    impl Render for PreviewHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let mut body = vec![named_block_element("run", &fixture_block())];
            if self.now_and_verdict {
                body.push(now_item_element(&fixture_now_item()));
                body.push(verdict_block_element(&fixture_verdict_block()));
            }
            if self.extra {
                body.push(
                    div()
                        .debug_selector(|| "preview-probe".to_string())
                        .w_full()
                        .h(px(10.))
                        .into_any_element(),
                );
            }
            if self.landing {
                body.push(landing_block_element(&fixture_landing_block()));
            }
            let footer = preview_footer_element("run-1 \u{b7} started 13:44");
            preview_column_element(body, Some(footer))
        }
    }

    fn open_harness(
        cx: &mut gpui::TestAppContext,
        extra: bool,
    ) -> gpui::WindowHandle<PreviewHarness> {
        open_harness_full(cx, extra, false)
    }

    fn open_landing_harness(cx: &mut gpui::TestAppContext) -> gpui::WindowHandle<PreviewHarness> {
        let window = cx
            .update(|cx| {
                let bounds = Bounds::new(gpui::point(px(0.), px(0.)), size(px(900.), px(700.)));
                cx.open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                        ..Default::default()
                    },
                    |_, cx| {
                        cx.new(|_| PreviewHarness {
                            extra: true,
                            now_and_verdict: false,
                            landing: true,
                        })
                    },
                )
            })
            .unwrap();
        cx.run_until_parked();
        window
    }

    fn open_harness_full(
        cx: &mut gpui::TestAppContext,
        extra: bool,
        now_and_verdict: bool,
    ) -> gpui::WindowHandle<PreviewHarness> {
        let window = cx
            .update(|cx| {
                let bounds = Bounds::new(gpui::point(px(0.), px(0.)), size(px(900.), px(700.)));
                cx.open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                        ..Default::default()
                    },
                    |_, cx| {
                        cx.new(|_| PreviewHarness {
                            extra,
                            now_and_verdict,
                            landing: false,
                        })
                    },
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
            .debug_bounds("preview-block-run")
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
            .debug_bounds("preview-block-run")
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

    /// The `landing` block paints its heading and exactly three `→` lines,
    /// after the (probe, standing in for slots) block and before the spacer
    /// and footer — the same "one heading form" invariant every other block
    /// composes through `named_block_shell`.
    #[gpui::test]
    fn landing_block_paints_three_lines_after_the_probe_and_before_the_spacer(
        cx: &mut gpui::TestAppContext,
    ) {
        let window = open_landing_harness(cx);
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);

        let probe = vcx.debug_bounds("preview-probe").expect("probe painted");
        let heading = vcx
            .debug_bounds("preview-heading-landing")
            .expect("landing heading painted");
        let line0 = vcx
            .debug_bounds("preview-landing-line-0")
            .expect("landing line 0 painted");
        let line1 = vcx
            .debug_bounds("preview-landing-line-1")
            .expect("landing line 1 painted");
        let line2 = vcx
            .debug_bounds("preview-landing-line-2")
            .expect("landing line 2 painted");
        let spacer = vcx.debug_bounds("preview-spacer").expect("spacer painted");
        let footer = vcx.debug_bounds("preview-footer").expect("footer painted");

        assert!(
            heading.origin.y >= probe.origin.y + probe.size.height,
            "the landing block sits after the prior block"
        );
        assert!(line0.origin.y >= heading.origin.y + heading.size.height);
        assert!(line1.origin.y >= line0.origin.y + line0.size.height);
        assert!(line2.origin.y >= line1.origin.y + line1.size.height);
        assert!(
            spacer.origin.y >= line2.origin.y + line2.size.height,
            "the landing block sits before the spacer"
        );
        assert!(footer.origin.y >= spacer.origin.y + spacer.size.height);
    }

    #[gpui::test]
    fn an_extra_body_block_inserts_between_identity_block_and_spacer(
        cx: &mut gpui::TestAppContext,
    ) {
        let window = open_harness(cx, true);
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);

        let block = vcx
            .debug_bounds("preview-block-run")
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
            .debug_bounds("preview-heading-run")
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
            .debug_bounds("preview-block-run")
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
            .debug_bounds("preview-block-run")
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
            .debug_bounds("preview-block-run")
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

    #[gpui::test]
    fn now_item_and_verdict_block_insert_between_identity_block_and_spacer_footer_still_last(
        cx: &mut gpui::TestAppContext,
    ) {
        let window = open_harness_full(cx, false, true);
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);

        let block = vcx
            .debug_bounds("preview-block-run")
            .expect("run block painted");
        let now = vcx
            .debug_bounds("preview-block-now")
            .expect("now block painted");
        let verdict = vcx
            .debug_bounds("preview-block-verdict")
            .expect("verdict block painted");
        let spacer = vcx.debug_bounds("preview-spacer").expect("spacer painted");
        let footer = vcx.debug_bounds("preview-footer").expect("footer painted");

        assert!(now.origin.y >= block.origin.y + block.size.height);
        assert!(verdict.origin.y >= now.origin.y + now.size.height);
        assert!(spacer.origin.y >= verdict.origin.y + verdict.size.height);
        assert!(footer.origin.y >= spacer.origin.y + spacer.size.height);
    }

    #[gpui::test]
    fn now_item_is_bordered_row_open_filled_with_the_internal_gap_and_no_radius(
        cx: &mut gpui::TestAppContext,
    ) {
        let window = open_harness_full(cx, false, true);
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);

        let item = vcx
            .debug_bounds("preview-now-item")
            .expect("now item container painted");
        let title = vcx
            .debug_bounds("preview-now-title")
            .expect("now title painted");
        let marker = vcx
            .debug_bounds("preview-now-marker")
            .expect("now marker painted");
        let narration = vcx
            .debug_bounds("preview-now-narration")
            .expect("now narration painted");

        assert!(
            marker.origin.x > title.origin.x + title.size.width,
            "title and marker must not overlap"
        );

        let gap = narration.origin.y - (title.origin.y + title.size.height);
        assert!(
            (gap - tokens::NOW_ITEM_GAP).abs() <= px(1.),
            "the title row and narration are separated by the internal gap: got {gap:?}"
        );
        assert!(
            narration.size.height >= tokens::NOW_NARRATION_LINE_HEIGHT,
            "the narration paints at least its line height"
        );

        // Leading/trailing/top inner edges reach [8,10] padding, not a
        // full-width container proving nothing about the padded box.
        let leading_pad = title.origin.x - item.origin.x;
        assert!(
            (leading_pad - tokens::BORDERED_BOX_PAD_X_MIN).abs() <= px(1.),
            "the title's leading edge must sit BORDERED_BOX_PAD_X_MIN in from the item's own \
             leading edge: got {leading_pad:?}"
        );
        let trailing_pad =
            (item.origin.x + item.size.width) - (marker.origin.x + marker.size.width);
        assert!(
            (trailing_pad - tokens::BORDERED_BOX_PAD_X_MIN).abs() <= px(1.),
            "the marker's trailing edge must sit BORDERED_BOX_PAD_X_MIN in from the item's own \
             trailing edge: got {trailing_pad:?}"
        );
        let top_pad = title.origin.y - item.origin.y;
        assert!(
            (top_pad - tokens::BORDERED_BOX_PAD_Y).abs() <= px(1.),
            "the title's top edge must sit BORDERED_BOX_PAD_Y down from the item's own top \
             edge: got {top_pad:?}"
        );
        let expected_narration_width =
            item.size.width - (tokens::BORDERED_BOX_PAD_X_MIN + tokens::NOW_ITEM_BORDER) * 2.;
        assert!(
            (narration.size.width - expected_narration_width).abs() <= px(1.),
            "the narration reaches exactly the item's padded content width, not merely narrower \
             than the outer box: got {:?}, expected {expected_narration_width:?}",
            narration.size.width
        );
        let bottom_pad =
            (item.origin.y + item.size.height) - (narration.origin.y + narration.size.height);
        assert!(
            (bottom_pad - tokens::BORDERED_BOX_PAD_Y).abs() <= px(1.),
            "the narration's bottom edge must sit BORDERED_BOX_PAD_Y up from the item's own \
             bottom edge: got {bottom_pad:?}"
        );
    }

    /// The two new blocks carry the same block gap and bottom padding as the
    /// existing `run` block — one block-shell form, not a second geometry.
    #[gpui::test]
    fn now_and_verdict_blocks_carry_the_shared_block_gap_and_bottom_padding(
        cx: &mut gpui::TestAppContext,
    ) {
        let window = open_harness_full(cx, false, true);
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);

        let now_block = vcx
            .debug_bounds("preview-block-now")
            .expect("now block painted");
        let now_heading = vcx
            .debug_bounds("preview-heading-now")
            .expect("now heading painted");
        let now_item = vcx
            .debug_bounds("preview-now-item")
            .expect("now item painted");
        let verdict_block = vcx
            .debug_bounds("preview-block-verdict")
            .expect("verdict block painted");
        let verdict_heading = vcx
            .debug_bounds("preview-heading-verdict")
            .expect("verdict heading painted");
        let status_row = vcx
            .debug_bounds("preview-row-status")
            .expect("status row painted");

        assert!(
            (now_item.size.width - now_block.size.width).abs() <= px(1.),
            "the now item reaches the full width of its enclosing block, not a narrower \
             child: item {:?}, block {:?}",
            now_item.size.width,
            now_block.size.width
        );

        let now_gap = now_item.origin.y - (now_heading.origin.y + now_heading.size.height);
        assert!(
            (now_gap - tokens::BLOCK_GAP).abs() <= px(1.),
            "the now block's heading-to-item gap must be BLOCK_GAP: got {now_gap:?}"
        );
        let verdict_gap =
            status_row.origin.y - (verdict_heading.origin.y + verdict_heading.size.height);
        assert!(
            (verdict_gap - tokens::BLOCK_GAP).abs() <= px(1.),
            "the verdict block's heading-to-row gap must be BLOCK_GAP: got {verdict_gap:?}"
        );

        let now_bottom_gap = (now_block.origin.y + now_block.size.height)
            - (now_item.origin.y + now_item.size.height);
        assert!(
            (now_bottom_gap - tokens::BLOCK_BOTTOM_PADDING).abs() <= px(1.),
            "the now block's last-child-to-block-bottom gap must be exactly \
             BLOCK_BOTTOM_PADDING: got {now_bottom_gap:?}"
        );
        // The fixture verdict block carries one blocker line, so the block's
        // actual last child is `preview-blocker-0`, not the status row —
        // measuring from the status row alone let a dropped bottom-padding
        // regression pass (review-verdict-1 blocker
        // `required-rendered-preview-evidence-missing`).
        let last_blocker = vcx
            .debug_bounds("preview-blocker-0")
            .expect("blocker line painted");
        assert!(
            last_blocker.origin.y >= status_row.origin.y + status_row.size.height,
            "the blocker line must sit below the status row: blocker {last_blocker:?}, \
             status row {status_row:?}"
        );
        let verdict_bottom_gap = (verdict_block.origin.y + verdict_block.size.height)
            - (last_blocker.origin.y + last_blocker.size.height);
        assert!(
            (verdict_bottom_gap - tokens::BLOCK_BOTTOM_PADDING).abs() <= px(1.),
            "the verdict block's last-child-to-block-bottom gap must be exactly \
             BLOCK_BOTTOM_PADDING, measured from the blocker line: got {verdict_bottom_gap:?}"
        );
    }

    /// Direct colour-resolution proof (no window) that a live now item's
    /// accent marker resolves through `role_color(StateRole::Accent)` and a
    /// muted absence title resolves through `tokens::TEXT_MUTED` — the same
    /// class of assertion `segment_color_resolves_...` makes for the run
    /// block's rows.
    #[test]
    fn now_item_accent_marker_and_muted_title_resolve_through_the_shared_tables() {
        assert_eq!(
            role_color(crate::run_row::StateRole::Accent),
            tokens::ACCENT
        );

        let muted = NowItem {
            title: "no current frame".to_string(),
            title_muted: true,
            state_word: "running".to_string(),
            state_role: crate::run_row::StateRole::Accent,
            narration: "working on this frame now".to_string(),
        };
        assert_eq!(now_item_title_color(&muted), tokens::TEXT_MUTED);
        assert_ne!(now_item_title_color(&muted), tokens::TEXT);

        let served = NowItem {
            title_muted: false,
            ..muted
        };
        assert_eq!(now_item_title_color(&served), tokens::TEXT);
    }

    #[gpui::test]
    fn verdict_block_renders_heading_status_row_and_blocker_line_in_order(
        cx: &mut gpui::TestAppContext,
    ) {
        let window = open_harness_full(cx, false, true);
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);

        let heading = vcx
            .debug_bounds("preview-heading-verdict")
            .expect("verdict heading painted");
        let status_row = vcx
            .debug_bounds("preview-row-status")
            .expect("status row painted");
        let blocker = vcx
            .debug_bounds("preview-blocker-0")
            .expect("blocker line painted");

        assert!(heading.origin.y < status_row.origin.y);
        assert!(status_row.origin.y < blocker.origin.y);
    }
}
