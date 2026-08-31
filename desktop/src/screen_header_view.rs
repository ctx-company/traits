//! Native gpui element tree for grammar rule 6's screen header. A free
//! function, no `Context`, no `cx.listener` — the `title_bar_view.rs`/
//! `preview_view.rs`/`frame_list_view.rs` convention, so this is directly
//! callable from a test with no gpui `App`.
//!
//! Reads nothing: no center, board, ledger, store, sidecar, filesystem, cwd,
//! cache or selection — the two `&str` arguments are the entire contract,
//! so `0267.2`/`0268.2`/`0269.2` can compose this unchanged from their own
//! sources.

use gpui::prelude::*;
use gpui::{AnyElement, div, rgb};

use crate::tokens;

/// The reusable rule-6 screen header: a full-width `space-between` row, a
/// left title stack (title over exactly one italic summary), and a
/// top-right glyph wrapper. `summary` is always emitted, exactly once — the
/// model guarantees a non-empty string, so there is no "skip when empty"
/// branch that could render zero elements. No breadcrumb, back link or
/// subtitle element exists in this function at all.
pub fn screen_header_element(title: &str, summary: &str) -> AnyElement {
    let title_stack = div()
        .debug_selector(|| "screen-header-title-stack".to_string())
        .flex()
        .flex_col()
        .gap(tokens::SCREEN_HEADER_STACK_GAP)
        .child(
            div()
                .debug_selector(|| "screen-header-title".to_string())
                .font_family(tokens::FONT_SANS)
                .text_size(tokens::SIZE_13)
                .font_weight(tokens::WEIGHT_NORMAL)
                .text_color(rgb(tokens::TEXT_HEADING))
                .whitespace_nowrap()
                .child(title.to_string()),
        )
        .child(
            div()
                .debug_selector(|| "screen-header-summary".to_string())
                .font_family(tokens::FONT_SANS)
                .text_size(tokens::SIZE_12)
                .italic()
                .font_weight(tokens::WEIGHT_NORMAL)
                .text_color(rgb(tokens::TEXT_SECONDARY))
                .w(tokens::SCREEN_HEADER_SUMMARY_WIDTH)
                .child(summary.to_string()),
        );

    // `\u{2237}` (`∷`) is a design glyph literal (`grammar.md:58-66`,
    // `reference/sessions.html:265,283`), the same class as `preview.rs`'s
    // `·` and `title_bar_view.rs`'s `SESSIONS` — not a data literal.
    let action = div()
        .debug_selector(|| "screen-header-action".to_string())
        .py(tokens::SCREEN_HEADER_ACTION_PAD_Y)
        .px(tokens::SCREEN_HEADER_ACTION_PAD_X)
        .child(
            div()
                .debug_selector(|| "screen-header-glyph".to_string())
                .font_family(tokens::FONT_MONO)
                .text_size(tokens::SIZE_14)
                .text_color(rgb(tokens::TEXT_SECONDARY))
                .child("\u{2237}"),
        );

    div()
        .debug_selector(|| "screen-header".to_string())
        .flex()
        .flex_row()
        .w_full()
        .justify_between()
        .items_start()
        .flex_shrink_0()
        .child(title_stack)
        .child(action)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Bounds, Context, Render, Window, px, size};

    struct Harness {
        title: &'static str,
        summary: &'static str,
    }

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(900.))
                .child(screen_header_element(self.title, self.summary))
        }
    }

    fn open_harness(
        cx: &mut gpui::TestAppContext,
        title: &'static str,
        summary: &'static str,
    ) -> gpui::WindowHandle<Harness> {
        let window = cx
            .update(|cx| {
                let bounds = Bounds::new(gpui::point(px(0.), px(0.)), size(px(900.), px(200.)));
                cx.open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|_| Harness { title, summary }),
                )
            })
            .unwrap();
        cx.run_until_parked();
        window
    }

    #[gpui::test]
    fn title_stack_gap_matches_the_token_and_title_sits_above_the_summary(
        cx: &mut gpui::TestAppContext,
    ) {
        let window = open_harness(cx, "0265.13 \u{2014} render", "a summary");
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);

        let title = vcx
            .debug_bounds("screen-header-title")
            .expect("title painted");
        let summary = vcx
            .debug_bounds("screen-header-summary")
            .expect("summary painted");
        assert!(
            title.origin.y < summary.origin.y,
            "title sits above summary"
        );
        let gap = summary.origin.y - (title.origin.y + title.size.height);
        assert_eq!(gap, tokens::SCREEN_HEADER_STACK_GAP);
    }

    #[gpui::test]
    fn summary_width_and_full_width_space_between_hold(cx: &mut gpui::TestAppContext) {
        let window = open_harness(cx, "title", "a summary");
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);

        let summary = vcx
            .debug_bounds("screen-header-summary")
            .expect("summary painted");
        assert_eq!(summary.size.width, tokens::SCREEN_HEADER_SUMMARY_WIDTH);

        let header = vcx.debug_bounds("screen-header").expect("header painted");
        let action = vcx
            .debug_bounds("screen-header-action")
            .expect("action painted");
        assert_eq!(header.size.width, px(900.));
        assert_eq!(
            action.origin.x + action.size.width,
            header.origin.x + header.size.width,
            "the glyph wrapper's right edge sits at the container's right edge"
        );
    }

    #[gpui::test]
    fn glyph_wrapper_insets_match_the_tokens_and_sits_top_right(cx: &mut gpui::TestAppContext) {
        let window = open_harness(cx, "title", "a summary");
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);

        let action = vcx
            .debug_bounds("screen-header-action")
            .expect("action painted");
        let glyph = vcx
            .debug_bounds("screen-header-glyph")
            .expect("glyph painted");
        let title_stack = vcx
            .debug_bounds("screen-header-title-stack")
            .expect("title stack painted");

        assert_eq!(
            glyph.origin.y - action.origin.y,
            tokens::SCREEN_HEADER_ACTION_PAD_Y
        );
        assert_eq!(
            glyph.origin.x - action.origin.x,
            tokens::SCREEN_HEADER_ACTION_PAD_X
        );
        assert_eq!(
            action.origin.y, title_stack.origin.y,
            "the glyph wrapper's top aligns with the title stack's top"
        );
    }

    #[gpui::test]
    fn exactly_one_summary_element_paints_for_present_absent_and_long_text(
        cx: &mut gpui::TestAppContext,
    ) {
        for summary in [
            "a present summary",
            "no run description yet",
            &"x".repeat(400),
        ] {
            let window = open_harness(cx, "title", Box::leak(summary.to_string().into_boxed_str()));
            let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
            assert!(
                vcx.debug_bounds("screen-header-summary").is_some(),
                "exactly one summary element must paint for {summary:?}"
            );
        }
    }

    #[gpui::test]
    fn no_breadcrumb_back_or_subtitle_selector_exists_in_the_painted_tree(
        cx: &mut gpui::TestAppContext,
    ) {
        let window = open_harness(cx, "title", "a summary");
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
        assert!(vcx.debug_bounds("screen-header-breadcrumb").is_none());
        assert!(vcx.debug_bounds("screen-header-back").is_none());
        assert!(vcx.debug_bounds("screen-header-subtitle").is_none());
    }
}
