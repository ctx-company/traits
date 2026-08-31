//! Native gpui element tree for a [`Rail`]. A free function, not a `Shell`
//! method — the `bottom_bar_view.rs`/`detail_view.rs`/`frame_list_view.rs`
//! convention: no `Context`, no `cx.listener`, directly callable from a
//! test with no `App`. Every geometric and colour value comes from
//! `tokens::`; the only literals here are rule 10's own interface words
//! ("Spaces", "·", "local"), the same class `frame_list_view.rs`'s state
//! words already are.

use gpui::prelude::*;
use gpui::{AnyElement, div, rgb};

use crate::frame_list_view::dot_element;
use crate::rail::Rail;
use crate::tokens;

pub fn rail_element(rail: &Rail) -> AnyElement {
    let mut column = div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .h_full()
        .w(tokens::RAIL_WIDTH)
        .py(tokens::RAIL_PAD_Y)
        .px(tokens::RAIL_PAD_X);

    let heading = div()
        .pt(tokens::RAIL_HEADING_PAD_TOP)
        .pr(tokens::RAIL_HEADING_PAD_RIGHT)
        .pb(tokens::RAIL_HEADING_PAD_BOTTOM)
        .pl(tokens::RAIL_HEADING_PAD_LEFT)
        .font_family(tokens::FONT_MONO)
        .text_size(tokens::SIZE_11)
        .text_color(rgb(tokens::TEXT_MUTED))
        .child("Spaces");
    column = column.child(heading);

    let mut rows = div()
        .flex()
        .flex_col()
        .w_full()
        .py(tokens::RAIL_REPO_ROW_PAD_Y)
        .px(tokens::RAIL_REPO_ROW_PAD_X)
        .gap(tokens::RAIL_REPO_ROW_GAP);
    for (index, repo) in rail.repos().iter().enumerate() {
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .py(tokens::RAIL_ROW_PAD_Y)
            .px(tokens::RAIL_ROW_PAD_X)
            .gap(tokens::ROW_DOT_TEXT_GAP_MIN)
            .child(dot_element(rail.dot(index)))
            .child(
                div()
                    .font_family(tokens::FONT_SANS)
                    .text_size(tokens::SIZE_13)
                    .text_color(rgb(rail.name_color(index)))
                    .child(repo.name.clone()),
            );
        if rail.is_active(index) {
            row = row.w_full().bg(rgb(tokens::SURFACE_RAISED));
        }
        rows = rows.child(row);
    }
    if rail.is_stale() {
        rows = rows.opacity(0.6);
    }
    column = column.child(rows);

    column = column.child(div().flex_1().w_full());

    let divider = div()
        .flex()
        .flex_row()
        .justify_center()
        .w_full()
        .pt(tokens::RAIL_DIVIDER_PAD_Y_MIN)
        .px(tokens::RAIL_DIVIDER_PAD_X_MIN)
        .child(
            div()
                .w(tokens::RAIL_DIVIDER_WIDTH)
                .h(tokens::RAIL_DIVIDER_HEIGHT)
                .bg(rgb(tokens::BORDER_STRONG)),
        );
    column = column.child(divider);

    let mut footer = div()
        .flex()
        .flex_col()
        .pt(tokens::RAIL_FOOTER_PAD_TOP)
        .pr(tokens::RAIL_FOOTER_PAD_RIGHT)
        .pb(tokens::RAIL_FOOTER_PAD_BOTTOM)
        .pl(tokens::RAIL_FOOTER_PAD_LEFT)
        .gap(tokens::RAIL_FOOTER_GAP)
        .child(
            div()
                .font_family(tokens::FONT_MONO)
                .text_size(tokens::SIZE_10_5)
                .text_color(rgb(tokens::TEXT_SECONDARY))
                .child(rail.owner()),
        );
    if let Some(space) = rail.space() {
        footer = footer.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::RAIL_FOOTER_SEGMENT_GAP)
                .child(
                    div()
                        .font_family(tokens::FONT_MONO)
                        .text_size(tokens::SIZE_10_5)
                        .text_color(rgb(tokens::TEXT_SESSION))
                        .child(space.to_string()),
                )
                .child(
                    div()
                        .font_family(tokens::FONT_MONO)
                        .text_size(tokens::SIZE_10_5)
                        .text_color(rgb(tokens::TEXT_FAINT))
                        .child("·"),
                )
                .child(
                    div()
                        .font_family(tokens::FONT_MONO)
                        .text_size(tokens::SIZE_10_5)
                        .text_color(rgb(tokens::TEXT_FAINT))
                        .child("local"),
                ),
        );
    }
    column = column.child(footer);

    column.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rail::project;
    use ctx_traits_io::center::CenterPublicRow;
    use ctx_traits_io::run_summary::RunSummary;

    fn row(repo_key: &str, repo_path: &str, live: bool) -> CenterPublicRow {
        CenterPublicRow {
            summary: RunSummary::unreadable(repo_key.to_string(), "unused".to_string()),
            repo_key: repo_key.to_string(),
            repo_path: repo_path.to_string(),
            ledger_path: format!("{repo_key}/x"),
            live,
            modified_epoch_secs: 0,
        }
    }

    #[test]
    fn builds_populated_and_active() {
        let rows = [row("repo-a", "/x/aaa", true)];
        let rail = project(rows.iter(), Some("repo-a"), false);
        let _ = rail_element(&rail);
    }

    #[test]
    fn builds_populated_and_no_active() {
        let rows = [row("repo-a", "/x/aaa", true)];
        let rail = project(rows.iter(), None, false);
        let _ = rail_element(&rail);
    }

    #[test]
    fn builds_empty() {
        let rail = project(std::iter::empty(), None, false);
        let _ = rail_element(&rail);
    }

    #[test]
    fn builds_stale() {
        let rows = [row("repo-a", "/x/aaa", true)];
        let rail = project(rows.iter(), Some("repo-a"), true);
        let _ = rail_element(&rail);
    }
}
