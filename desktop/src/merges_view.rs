//! Native gpui elements for the static Merges pane.

use gpui::prelude::*;
use gpui::{AnyElement, SharedString, div, rgb};

use crate::merges::{MergeRowData, MergeSectionData};
use crate::overflow_fade;
use crate::run_row::role_color;
use crate::tokens;

pub type SelectHandler =
    Box<dyn Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static>;

fn merge_row_element(
    row: &MergeRowData,
    selected: bool,
    index: usize,
    handler: Option<SelectHandler>,
) -> AnyElement {
    let state = div()
        .flex()
        .flex_col()
        .flex_none()
        .items_end()
        .gap(tokens::LIST_ROWS_GAP_MIN)
        .font_family(tokens::FONT_MONO)
        .text_size(tokens::SIZE_10_5)
        .child(
            div()
                .debug_selector(move || format!("merge-row-state-{index}"))
                .text_color(rgb(role_color(row.state_role)))
                .child(match row.content.state_detail {
                    Some(detail) => format!("{} · {detail}", row.state_word),
                    None => row.state_word.to_string(),
                }),
        )
        .child(
            div()
                .debug_selector(move || format!("merge-row-meta-{index}"))
                .text_color(rgb(tokens::TEXT_MUTED))
                .child(row.content.meta),
        );
    let row_element = div()
        .id(SharedString::from(format!("merge-row-{index}")))
        .debug_selector(move || format!("merge-row-{index}"))
        .when(selected, |element| {
            element
                .bg(rgb(tokens::SURFACE_RAISED))
                .debug_selector(move || format!("merge-row-selected-{index}"))
        })
        .flex()
        .flex_row()
        .justify_between()
        .items_start()
        .py(tokens::LIST_ROW_PAD_Y_OPEN)
        .px(tokens::LIST_ROW_PAD_X_MAX)
        .gap(tokens::ROW_DOT_TEXT_GAP_MAX)
        .child(
            div().pt(tokens::FRAME_DOT_WRAP_PAD_TOP).child(
                div()
                    .debug_selector(move || format!("merge-row-dot-{index}"))
                    .w(tokens::LIST_ROW_DOT_SIZE)
                    .h(tokens::LIST_ROW_DOT_SIZE)
                    .rounded_full()
                    .bg(rgb(role_color(row.state_role))),
            ),
        )
        .child(
            div()
                .flex()
                .flex_1()
                .flex_col()
                .gap(tokens::LIST_ROWS_GAP_MIN)
                .child(
                    div()
                        .debug_selector(move || format!("merge-row-title-{index}"))
                        .font_family(tokens::FONT_SANS)
                        .text_size(tokens::SIZE_12_5)
                        .text_color(rgb(if selected {
                            tokens::TEXT_BRIGHT
                        } else {
                            tokens::TEXT
                        }))
                        .child(row.content.title),
                )
                .child(
                    div()
                        .debug_selector(move || format!("merge-row-description-{index}"))
                        .font_family(tokens::FONT_SANS)
                        .text_size(tokens::SIZE_11)
                        .text_color(rgb(tokens::TEXT_SECONDARY))
                        .child(row.content.description),
                ),
        )
        .child(state);
    let row_element = match handler {
        Some(handler) => row_element.on_click(handler).into_any_element(),
        None => row_element.into_any_element(),
    };
    if selected {
        div()
            .debug_selector(|| "merge-row-selected".to_string())
            .child(row_element)
            .into_any_element()
    } else {
        row_element
    }
}

pub fn merges_pane_element(
    sections: &[MergeSectionData],
    selected_index: usize,
    on_select: impl Fn(usize) -> Option<SelectHandler>,
) -> AnyElement {
    let mut flat_index = 0;
    let mut list = div().flex().flex_col().gap(tokens::LIST_SECTION_GAP);
    for (section_index, section) in sections.iter().enumerate() {
        let section_element = div()
            .id(SharedString::from(format!("merge-section-{section_index}")))
            .debug_selector(move || format!("merge-section-{section_index}"))
            .flex()
            .flex_col()
            .gap(tokens::LIST_SECTION_GAP)
            .child(
                div()
                    .debug_selector(move || format!("merge-section-heading-{section_index}"))
                    .font_family(tokens::FONT_MONO)
                    .text_size(tokens::SIZE_11)
                    .text_color(rgb(tokens::TEXT_MUTED))
                    .child(format!("{} — {}", section.heading, section.rows.len())),
            );
        let mut rows = div().flex().flex_col().gap(tokens::LIST_ROWS_GAP_MAX);
        for row in &section.rows {
            rows = rows.child(merge_row_element(
                row,
                flat_index == selected_index,
                flat_index,
                on_select(flat_index),
            ));
            flat_index += 1;
        }
        list = list.child(section_element.child(rows));
    }
    div()
        .debug_selector(|| "merges-pane".to_string())
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .pt(tokens::MAIN_PANE_PAD_TOP)
        .pr(tokens::MAIN_PANE_PAD_RIGHT)
        .pb(tokens::MAIN_PANE_PAD_BOTTOM)
        .pl(tokens::MAIN_PANE_PAD_LEFT)
        .gap(tokens::MAIN_PANE_GAP)
        .child(overflow_fade::clipped_with_overflow_fade(
            list.into_any_element(),
        ))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Bounds, Context, Render, Window, px, size};

    struct Harness {
        sections: Vec<MergeSectionData>,
    }

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .flex_col()
                .overflow_hidden()
                .child(merges_pane_element(&self.sections, 0, |_| None))
        }
    }

    fn open_harness(
        cx: &mut gpui::TestAppContext,
        height: gpui::Pixels,
    ) -> gpui::WindowHandle<Harness> {
        let sections = crate::merges::merge_sections();
        let window = cx
            .update(|cx| {
                let bounds = Bounds::new(gpui::point(px(0.), px(0.)), size(px(900.), height));
                cx.open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|_| Harness { sections }),
                )
            })
            .expect("open a real Merges window");
        cx.run_until_parked();
        window
    }

    #[gpui::test]
    fn merges_pane_paints_ordered_sections_and_all_row_forms(cx: &mut gpui::TestAppContext) {
        let window = open_harness(cx, px(600.));
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
        for selector in [
            "merge-section-0",
            "merge-section-1",
            "merge-section-2",
            "merge-section-heading-0",
            "merge-section-heading-1",
            "merge-section-heading-2",
            "merge-row-title-0",
            "merge-row-title-1",
            "merge-row-title-2",
            "merge-row-title-3",
            "merge-row-title-4",
            "merge-row-description-0",
            "merge-row-description-1",
            "merge-row-description-2",
            "merge-row-description-3",
            "merge-row-description-4",
            "merge-row-meta-0",
            "merge-row-meta-1",
            "merge-row-meta-2",
            "merge-row-meta-3",
            "merge-row-meta-4",
            "merge-row-dot-0",
            "merge-row-dot-1",
            "merge-row-dot-3",
            "merge-row-state-0",
            "merge-row-state-1",
            "merge-row-state-3",
        ] {
            assert!(vcx.debug_bounds(selector).is_some(), "{selector} paints");
        }
        assert!(vcx.debug_bounds("merge-row-selected").is_some());
        for selector in ["merge-row-1", "merge-row-2", "merge-row-3", "merge-row-4"] {
            assert!(vcx.debug_bounds(selector).is_some(), "{selector} paints");
        }
    }

    #[gpui::test]
    fn merges_pane_fade_fits_tall_and_paints_when_short(cx: &mut gpui::TestAppContext) {
        let tall = open_harness(cx, px(600.));
        let mut tall_vcx = gpui::VisualTestContext::from_window(tall.into(), cx);
        assert!(tall_vcx.debug_bounds("overflow-fade-overlay").is_none());

        let short = open_harness(cx, px(100.));
        let mut short_vcx = gpui::VisualTestContext::from_window(short.into(), cx);
        assert!(short_vcx.debug_bounds("overflow-fade-overlay").is_some());
    }
}
