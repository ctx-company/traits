//! P505 pane tree + chrome kit: a `PaneTree` resolved through nested ratatui
//! `Layout`s (no `Rect` arithmetic in this module), shared pane chrome
//! (`render_pane`), a focus ring over leaf ids, a tab-bar helper, and combined
//! content renderers (`render_list_pane`/`render_lines_pane`) that resolve a
//! pane's scroll window exactly once and feed the content from that same
//! value. P551: no pane in this kit draws a scrollbar — the bordered box is
//! enough; a future surface that wants one writes it explicitly. Sibling
//! of [`super::tui_kit`], which it extends additively —
//! [`super::tui_kit::ScrollList`]/[`super::tui_kit::ViewportScroll`] stay the
//! sole scroll-state types, [`super::tui_kit::MarkSet`] the sole multi-select
//! type. `tui-demo` is the proof surface; screen adoption (dashboard, live
//! run view) is P506/P507. Same two-tone palette doctrine as the rest of the
//! kit: named ANSI foreground plus `DIM`/`BOLD` only, no backgrounds. Pane
//! chrome is deliberately never green — green means "pass" in the house
//! grammar and must not be overloaded onto focus.
//! No fixture/snapshot tests here either, by the same 2026-07-24 owner ruling
//! `tui_kit.rs` documents: every test asserts state-machine behavior, never a
//! rendered frame.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as RLine, Span};
use ratatui::widgets::{Block, List, Paragraph, Tabs};

use super::tui_kit::{self, ScrollList, ViewportScroll};

/// The vertical regions every dashboard-style screen divides into: a 1-row
/// tab bar, the pane tree's own area, and a 2-row footer (P506 §2 — hoisted
/// out of `tui_demo`'s own private copy once the dashboard needed the
/// identical three-region shape, the one genuine second-consumer extraction
/// this phase creates). Shared by a screen's own draw pass and its
/// directional-move key handling, which needs the SAME pane-tree area a
/// frame would resolve against without a frame in hand.
pub(crate) fn screen_regions(area: Rect) -> [Rect; 3] {
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(area);
    [regions[0], regions[1], regions[2]]
}

/// Static per-screen pane identity. Panes are authored statically (each
/// screen builds its own fixed tree), so equality for the focus ring is
/// trivial and allocation-free.
pub(crate) type PaneId = &'static str;

/// `Leaf { id, title }` or `Split { dir, children }`, resolved to
/// `[(PaneId, Rect)]` through nested ratatui `Layout::split` calls — the
/// tree grows no second copy of `SplitBounds`' floor/cap policy; a caller
/// that needs a width floor expresses it via `Constraint::Min`/`Max` or
/// decides not to split before building the tree.
pub(crate) enum PaneTree {
    Leaf {
        id: PaneId,
        title: String,
    },
    Split {
        dir: Direction,
        children: Vec<(Constraint, PaneTree)>,
    },
}

impl PaneTree {
    /// Resolves this tree against `area`, recursively splitting through
    /// ratatui `Layout` at every `Split` node.
    pub(crate) fn resolve(&self, area: Rect) -> PaneLayoutResult {
        let mut rects = Vec::new();
        Self::resolve_into(self, area, &mut rects);
        PaneLayoutResult { rects }
    }

    fn resolve_into(node: &PaneTree, area: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match node {
            PaneTree::Leaf { id, .. } => out.push((*id, area)),
            PaneTree::Split { dir, children } => {
                let constraints: Vec<Constraint> = children.iter().map(|(c, _)| *c).collect();
                let split = ratatui::layout::Layout::default()
                    .direction(*dir)
                    .constraints(constraints)
                    .split(area);
                for ((_, child), rect) in children.iter().zip(split.iter()) {
                    Self::resolve_into(child, *rect, out);
                }
            }
        }
    }

    /// The leaf ids in this tree, depth-first — the order a [`FocusRing`]
    /// cycles through.
    pub(crate) fn leaf_ids(&self) -> Vec<PaneId> {
        let mut ids = Vec::new();
        Self::collect_ids(self, &mut ids);
        ids
    }

    fn collect_ids(node: &PaneTree, out: &mut Vec<PaneId>) {
        match node {
            PaneTree::Leaf { id, .. } => out.push(*id),
            PaneTree::Split { children, .. } => {
                for (_, child) in children {
                    Self::collect_ids(child, out);
                }
            }
        }
    }

    /// The title of the leaf `id`, if it exists in this tree — for a
    /// consumer's own `render_pane` call, so it never needs to track titles
    /// in a second map beside the tree it already built.
    pub(crate) fn title(&self, id: PaneId) -> Option<&str> {
        match self {
            PaneTree::Leaf { id: leaf_id, title } if *leaf_id == id => Some(title.as_str()),
            PaneTree::Leaf { .. } => None,
            PaneTree::Split { children, .. } => {
                children.iter().find_map(|(_, child)| child.title(id))
            }
        }
    }
}

/// The rectangles a [`PaneTree::resolve`] call produced. `Clone`+`Default` so
/// a consumer can cache the last-drawn layout (e.g. for directional focus
/// movement at key-handling time) instead of re-reading the terminal size
/// and re-resolving the tree outside a draw pass — cheaper, and immune to a
/// resize landing between a draw and the next key.
#[derive(Clone, Default)]
pub(crate) struct PaneLayoutResult {
    rects: Vec<(PaneId, Rect)>,
}

impl PaneLayoutResult {
    pub(crate) fn rect(&self, id: PaneId) -> Option<Rect> {
        self.rects
            .iter()
            .find(|(pane_id, _)| *pane_id == id)
            .map(|(_, rect)| *rect)
    }

    /// Overwrites (or inserts) `id`'s own rect — for a caller merging a
    /// sub-renderer's own resolved geometry (e.g. `render_pane_body`'s
    /// bounded-progress split) into a cached whole-screen layout that a
    /// different tree produced for every other pane, so cached rects for
    /// `id` never drift from what was actually drawn.
    pub(crate) fn set(&mut self, id: PaneId, rect: Rect) {
        if let Some(existing) = self.rects.iter_mut().find(|(pane_id, _)| *pane_id == id) {
            existing.1 = rect;
        } else {
            self.rects.push((id, rect));
        }
    }
}

/// Draws a bordered pane with `title` in the border's top-left, styled per
/// `focused` — primary (default-fg) border + title when focused, DIM border +
/// DIM title otherwise (owner spec 2026-08-04: pane focus shows in the chrome
/// alone). Content is deliberately never dimmed here — text stays primary in
/// every pane, so this block carries no whole-rect style and no BOLD.
/// Terminal-window focus is not an input either: when the window itself is
/// unfocused, `tui_ratatui`'s draw pass dims the whole frame buffer, taking
/// chrome, titles, and content down together. Returns the inner content rect,
/// so a consumer never computes `block.inner()` itself (the single call every
/// second-copy chrome bug in this kit has been about).
pub(crate) fn render_pane(
    frame: &mut ratatui::Frame<'_>,
    rect: Rect,
    title: &str,
    focused: bool,
) -> Rect {
    let chrome_style = if focused {
        Style::default()
    } else {
        Style::default().add_modifier(Modifier::DIM)
    };
    let block = Block::bordered()
        .border_style(chrome_style)
        .title_top(RLine::from(Span::styled(title.to_string(), chrome_style)));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    inner
}

/// Returns the content rectangle owned by [`render_pane`]'s one-cell border.
/// Consumers can resolve input against current pane geometry before painting
/// chrome, without duplicating border arithmetic.
pub(crate) fn pane_inner(rect: Rect) -> Rect {
    Block::bordered().inner(rect)
}

/// Directional pane movement, resolved from already-resolved rects — the
/// only geometry this phase adds beyond the tree's own `Layout::split`
/// calls.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MoveDir {
    Up,
    Down,
    Left,
    Right,
}

fn center(rect: Rect) -> (i64, i64) {
    (
        rect.x as i64 + rect.width as i64 / 2,
        rect.y as i64 + rect.height as i64 / 2,
    )
}

/// Focus over a fixed set of leaf ids: `next`/`prev` (wrapping) for
/// `Tab`/`Shift-Tab`-style cycling, plus [`Self::move_dir`] for directional
/// movement across already-resolved rects.
pub(crate) struct FocusRing {
    ids: Vec<PaneId>,
    current: usize,
}

impl FocusRing {
    pub(crate) fn new(ids: Vec<PaneId>) -> Self {
        Self { ids, current: 0 }
    }

    pub(crate) fn current(&self) -> Option<PaneId> {
        self.ids.get(self.current).copied()
    }

    /// Reconciles ring membership against `ids` — the leaves of the tree
    /// actually about to be drawn — so the focused pane can never be one
    /// that has no rect this frame (P506 review: `focus-ring-includes-
    /// undrawn-panes`). Keeps the current focus if it survives into `ids`;
    /// otherwise falls back to `fallback` (always the screen's list pane,
    /// which every tree — even the narrow-terminal single-leaf one —
    /// includes). Call this every draw, against the width-resolved tree,
    /// never a hypothetical maximum-width one.
    pub(crate) fn reconcile(&mut self, ids: Vec<PaneId>, fallback: PaneId) {
        let keep = self.current();
        self.current = ids
            .iter()
            .position(|&id| Some(id) == keep)
            .or_else(|| ids.iter().position(|&id| id == fallback))
            .unwrap_or(0);
        self.ids = ids;
    }

    pub(crate) fn is_focused(&self, id: PaneId) -> bool {
        self.current() == Some(id)
    }

    /// Wraps to the first id once past the last. A single-leaf (or empty)
    /// ring is a no-op.
    pub(crate) fn next(&mut self) {
        if self.ids.is_empty() {
            return;
        }
        self.current = (self.current + 1) % self.ids.len();
    }

    /// Wraps to the last id once before the first. A single-leaf (or empty)
    /// ring is a no-op.
    pub(crate) fn prev(&mut self) {
        if self.ids.is_empty() {
            return;
        }
        self.current = (self.current + self.ids.len() - 1) % self.ids.len();
    }

    /// Moves focus to the nearest leaf whose rect center lies in the
    /// half-plane `dir` points to, breaking ties by squared distance. A
    /// no-op if no candidate lies in that half-plane (e.g. at an edge, or a
    /// single-leaf tree).
    pub(crate) fn move_dir(&mut self, dir: MoveDir, layout: &PaneLayoutResult) {
        let Some(current_id) = self.current() else {
            return;
        };
        let Some(current_rect) = layout.rect(current_id) else {
            return;
        };
        let (cx, cy) = center(current_rect);
        let mut best: Option<(usize, i64)> = None;
        for (idx, id) in self.ids.iter().enumerate() {
            if *id == current_id {
                continue;
            }
            let Some(rect) = layout.rect(id) else {
                continue;
            };
            let (x, y) = center(rect);
            let in_half_plane = match dir {
                MoveDir::Up => y < cy,
                MoveDir::Down => y > cy,
                MoveDir::Left => x < cx,
                MoveDir::Right => x > cx,
            };
            if !in_half_plane {
                continue;
            }
            let dist = (x - cx).pow(2) + (y - cy).pow(2);
            if best.is_none_or(|(_, best_dist)| dist < best_dist) {
                best = Some((idx, dist));
            }
        }
        if let Some((idx, _)) = best {
            self.current = idx;
        }
    }
}

/// A `Tabs` cycle step, resolved from a key by [`tab_cycle_key`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TabStep {
    Next,
    Prev,
}

/// `Tab` cycles forward, `Shift-Tab` (`BackTab`, or `Tab` with the `SHIFT`
/// modifier — terminals report either) cycles backward. Backward cycling is
/// new: nothing in the workspace mapped `Shift-Tab` before this phase.
pub(crate) fn tab_cycle_key(key: &KeyEvent) -> Option<TabStep> {
    match key.code {
        KeyCode::BackTab => Some(TabStep::Prev),
        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => Some(TabStep::Prev),
        KeyCode::Tab => Some(TabStep::Next),
        _ => None,
    }
}

/// Builds a `Tabs` widget: the current tab BOLD, every other DIM, no
/// highlight background — the two-tone palette applied to the widget the
/// workspace has never imported before this phase.
pub(crate) fn tab_bar(titles: &[String], current: usize) -> Tabs<'static> {
    let lines: Vec<RLine<'static>> = titles
        .iter()
        .enumerate()
        .map(|(idx, title)| {
            let style = if idx == current {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default().add_modifier(Modifier::DIM)
            };
            RLine::from(Span::styled(title.clone(), style))
        })
        .collect();
    Tabs::new(lines)
        .select(current)
        .divider(" ")
        .highlight_style(Style::default())
}

/// Renders a `ScrollList`-backed pane's visible rows from its window
/// computation. P551: this kit draws no scrollbar — the bordered box is
/// considered enough signal; a future surface that wants one writes it
/// explicitly. Consolidates what were per-consumer copies of window→items
/// into the one call every `ScrollList`-backed pane in this kit renders
/// through.
pub(crate) fn render_list_pane<T>(
    frame: &mut ratatui::Frame<'_>,
    inner: Rect,
    items: &[T],
    list: &ScrollList,
    label: impl Fn(&T) -> String,
    is_marked: impl Fn(&T) -> bool,
) -> std::ops::Range<usize> {
    let rows = inner.height as usize;
    let window = list.window(rows);
    let rendered =
        tui_kit::windowed_list_in_range(items, window.clone(), list.selected(), label, is_marked);
    frame.render_widget(List::new(rendered), inner);
    window
}

/// The `ViewportScroll`-backed sibling of [`render_list_pane`]: renders a
/// pane's visible lines from one window computation, same as that function
/// (no scrollbar drawn here either — see its doc comment).
/// `scroll` is taken by value (a cheap `Copy`, mirroring
/// [`PaneScrolls::get`]'s own scratch-copy pattern) since this render pass
/// never persists it — a caller that mutates scroll state on a key does so
/// through its own `PaneScrolls::get_mut`, never through this function.
/// Consolidates what were two verbatim copies (`tui_demo`'s detail and notes
/// panes) into the one call every `ViewportScroll`-backed pane renders
/// through.
pub(crate) fn render_lines_pane(
    frame: &mut ratatui::Frame<'_>,
    inner: Rect,
    lines: &[RLine<'static>],
    mut scroll: ViewportScroll,
) -> std::ops::Range<usize> {
    let rows = inner.height as usize;
    scroll.set_len(lines.len());
    let window = scroll.window(rows);
    let visible: Vec<RLine<'static>> = lines[window.clone()].to_vec();
    frame.render_widget(Paragraph::new(visible), inner);
    window
}

/// Wraps styled logical lines to `width` display columns without dropping
/// content. Story panes use these physical rows for their viewport, so
/// resize cannot make it describe a different window.
pub(crate) fn wrapped_lines(lines: &[RLine<'static>], width: u16) -> Vec<RLine<'static>> {
    lines
        .iter()
        .flat_map(|line| wrap_line(line, width as usize))
        .collect()
}

/// The mutable sibling of [`render_lines_pane`] for panes whose draw pass must
/// clamp persisted state against its current inner rectangle.
pub(crate) fn render_wrapped_lines_pane(
    frame: &mut ratatui::Frame<'_>,
    inner: Rect,
    lines: &[RLine<'static>],
    scroll: &mut ViewportScroll,
) -> std::ops::Range<usize> {
    let rows = inner.height as usize;
    scroll.set_len(lines.len());
    let window = scroll.visible_range(rows);
    frame.render_widget(Paragraph::new(lines[window.clone()].to_vec()), inner);
    window
}

type StyledChar = (char, Style);

fn char_width(ch: char) -> usize {
    let mut buf = [0u8; 4];
    Span::raw(ch.encode_utf8(&mut buf) as &str).width()
}

fn wrap_line(line: &RLine<'static>, width: usize) -> Vec<RLine<'static>> {
    if width == 0 || line.spans.is_empty() {
        return vec![RLine::default()];
    }
    let chars: Vec<StyledChar> = line
        .spans
        .iter()
        .flat_map(|span| span.content.chars().map(move |ch| (ch, span.style)))
        .collect();
    if chars.is_empty() {
        return vec![RLine::default()];
    }
    let mut rows = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = start;
        let mut used = 0;
        while end < chars.len() {
            let char_width = char_width(chars[end].0);
            if used + char_width > width {
                break;
            }
            used += char_width;
            end += 1;
        }
        let end = if end == start { start + 1 } else { end };
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (ch, style) in &chars[start..end.min(chars.len())] {
            match spans.last_mut() {
                Some(last) if last.style == *style => {
                    if let std::borrow::Cow::Owned(text) = &mut last.content {
                        text.push(*ch);
                    }
                }
                _ => spans.push(Span::styled(ch.to_string(), *style)),
            }
        }
        rows.push(RLine::from(spans));
        start = end;
    }
    rows
}

/// Per-pane scroll state: a thin `PaneId -> ViewportScroll` map, so each
/// scrollable pane keeps its own position independently. No new scroll type
/// — [`ViewportScroll`] already owns clamping and windowing.
#[derive(Default)]
pub(crate) struct PaneScrolls {
    scrolls: HashMap<PaneId, ViewportScroll>,
}

impl PaneScrolls {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn get_mut(&mut self, id: PaneId) -> &mut ViewportScroll {
        self.scrolls.entry(id).or_default()
    }

    /// A read-only copy of `id`'s scroll state (default if never touched) —
    /// for a draw pass that only holds `&DemoState`/`&self`, mirroring
    /// [`super::tui_kit::ScrollList::window`]'s scratch-copy pattern.
    pub(crate) fn get(&self, id: PaneId) -> ViewportScroll {
        self.scrolls.get(&id).copied().unwrap_or_default()
    }

    pub(crate) fn reset(&mut self, id: PaneId) {
        self.scrolls.insert(id, ViewportScroll::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(id: PaneId) -> PaneTree {
        PaneTree::Leaf {
            id,
            title: id.to_string(),
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn shift_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    fn area(width: u16, height: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    #[test]
    fn tree_tiles_area_with_no_gaps_or_overlap() {
        let tree = PaneTree::Split {
            dir: Direction::Horizontal,
            children: vec![
                (Constraint::Percentage(50), leaf("left")),
                (Constraint::Percentage(50), leaf("right")),
            ],
        };
        let layout = tree.resolve(area(100, 20));
        let left = layout.rect("left").expect("left");
        let right = layout.rect("right").expect("right");
        assert_eq!(left.x, 0);
        assert_eq!(left.x + left.width, right.x);
        assert_eq!(right.x + right.width, 100);
        assert_eq!(left.height, 20);
        assert_eq!(right.height, 20);
    }

    #[test]
    fn screen_regions_cover_the_full_frame_at_wide_and_narrow_widths() {
        for area in [area(160, 40), area(40, 12)] {
            let [tabs, body, footer] = screen_regions(area);
            assert_eq!(tabs.x, area.x);
            assert_eq!(body.x, area.x);
            assert_eq!(footer.x, area.x);
            assert_eq!(tabs.width, area.width);
            assert_eq!(body.width, area.width);
            assert_eq!(footer.width, area.width);
            assert_eq!(tabs.y + tabs.height, body.y);
            assert_eq!(body.y + body.height, footer.y);
            assert_eq!(footer.y + footer.height, area.y + area.height);
        }
    }

    #[test]
    fn nested_split_resolves() {
        let tree = PaneTree::Split {
            dir: Direction::Horizontal,
            children: vec![
                (Constraint::Percentage(50), leaf("left")),
                (
                    Constraint::Percentage(50),
                    PaneTree::Split {
                        dir: Direction::Vertical,
                        children: vec![
                            (Constraint::Percentage(50), leaf("top-right")),
                            (Constraint::Percentage(50), leaf("bottom-right")),
                        ],
                    },
                ),
            ],
        };
        let layout = tree.resolve(area(100, 20));
        let top = layout.rect("top-right").expect("top-right");
        let bottom = layout.rect("bottom-right").expect("bottom-right");
        assert_eq!(top.y, 0);
        assert_eq!(top.y + top.height, bottom.y);
        assert_eq!(bottom.y + bottom.height, 20);
        assert_eq!(tree.leaf_ids(), vec!["left", "top-right", "bottom-right"]);
    }

    #[test]
    fn zero_height_area_degrades_without_panic() {
        let tree = PaneTree::Split {
            dir: Direction::Vertical,
            children: vec![
                (Constraint::Percentage(50), leaf("top")),
                (Constraint::Percentage(50), leaf("bottom")),
            ],
        };
        let layout = tree.resolve(area(1, 0));
        assert_eq!(layout.rect("top").expect("top").height, 0);
    }

    #[test]
    fn one_column_area_degrades_without_panic() {
        let tree = leaf("only");
        let layout = tree.resolve(area(1, 5));
        assert_eq!(layout.rect("only").expect("only").width, 1);
    }

    #[test]
    fn focus_ring_next_prev_wrap() {
        let mut ring = FocusRing::new(vec!["a", "b", "c"]);
        assert_eq!(ring.current(), Some("a"));
        ring.next();
        assert_eq!(ring.current(), Some("b"));
        ring.next();
        ring.next();
        assert_eq!(ring.current(), Some("a"));
        ring.prev();
        assert_eq!(ring.current(), Some("c"));
    }

    #[test]
    fn focus_ring_single_leaf_is_a_no_op() {
        let mut ring = FocusRing::new(vec!["only"]);
        ring.next();
        assert_eq!(ring.current(), Some("only"));
        ring.prev();
        assert_eq!(ring.current(), Some("only"));
    }

    #[test]
    fn focus_ring_directional_move_picks_expected_neighbour() {
        // left | top-right / bottom-right, a fixed rect set mirroring the
        // demo's own tree shape.
        let tree = PaneTree::Split {
            dir: Direction::Horizontal,
            children: vec![
                (Constraint::Length(30), leaf("left")),
                (
                    Constraint::Percentage(100),
                    PaneTree::Split {
                        dir: Direction::Vertical,
                        children: vec![
                            (Constraint::Percentage(50), leaf("top-right")),
                            (Constraint::Percentage(50), leaf("bottom-right")),
                        ],
                    },
                ),
            ],
        };
        let layout = tree.resolve(area(100, 20));
        let mut ring = FocusRing::new(tree.leaf_ids());
        ring.move_dir(MoveDir::Right, &layout);
        assert!(ring.current() == Some("top-right") || ring.current() == Some("bottom-right"));
        let landed_top = ring.current() == Some("top-right");
        if landed_top {
            ring.move_dir(MoveDir::Down, &layout);
            assert_eq!(ring.current(), Some("bottom-right"));
        } else {
            ring.move_dir(MoveDir::Up, &layout);
            assert_eq!(ring.current(), Some("top-right"));
        }
        ring.move_dir(MoveDir::Left, &layout);
        assert_eq!(ring.current(), Some("left"));
    }

    #[test]
    fn focus_ring_directional_move_single_leaf_is_a_no_op() {
        let tree = leaf("only");
        let layout = tree.resolve(area(10, 10));
        let mut ring = FocusRing::new(tree.leaf_ids());
        ring.move_dir(MoveDir::Right, &layout);
        assert_eq!(ring.current(), Some("only"));
    }

    #[test]
    fn reconcile_keeps_current_focus_when_it_survives_into_the_new_ids() {
        let mut ring = FocusRing::new(vec!["list", "preview"]);
        ring.next();
        assert_eq!(ring.current(), Some("preview"));
        ring.reconcile(vec!["preview", "list"], "list");
        assert_eq!(ring.current(), Some("preview"));
    }

    #[test]
    fn reconcile_falls_back_when_current_focus_is_not_among_the_new_ids() {
        let mut ring = FocusRing::new(vec!["list", "preview"]);
        ring.next();
        assert_eq!(ring.current(), Some("preview"));
        // `preview` has no rect in the narrower tree — the ring must never
        // report a pane that will not be drawn as focused.
        ring.reconcile(vec!["list"], "list");
        assert_eq!(ring.current(), Some("list"));
    }

    #[test]
    fn tab_cycle_key_maps_tab_and_shift_tab_and_back_tab() {
        assert_eq!(tab_cycle_key(&key(KeyCode::Tab)), Some(TabStep::Next));
        assert_eq!(tab_cycle_key(&key(KeyCode::BackTab)), Some(TabStep::Prev));
        assert_eq!(tab_cycle_key(&shift_key(KeyCode::Tab)), Some(TabStep::Prev));
        assert_eq!(tab_cycle_key(&key(KeyCode::Char('z'))), None);
    }

    #[test]
    fn pane_scrolls_are_independent_per_id() {
        let mut scrolls = PaneScrolls::new();
        scrolls.get_mut("a").set_len(30);
        scrolls
            .get_mut("a")
            .apply(super::super::tui_kit::ScrollDelta::Down(100), 10);
        assert_eq!(scrolls.get_mut("a").window(10), 20..30);
        assert_eq!(scrolls.get_mut("b").window(10), 0..0);
    }

    #[test]
    fn list_pane_window_tracks_the_rendered_position_after_paging() {
        // Mirrors `tui_demo`'s own key path exactly: every key-handling-time
        // `apply`/`move_by` is fed `rows == usize::MAX` (no rect is known
        // until the next draw) — the exact call shape that used to leave a
        // persisted `offset` field pinned to 0 (the
        // `scrollbar-position-not-render-offset` blocker's root cause).
        // `render_list_pane` no longer exposes or reads any such field: it
        // sources the rendered rows from one `ScrollList::window(rows)` call,
        // so this asserts that resolved window is correct and non-zero.
        use super::super::tui_kit::ScrollList;
        let mut list = ScrollList::new();
        list.set_len(60);
        for _ in 0..25 {
            list.move_by(10, usize::MAX); // 25 page-sized downward moves.
        }
        assert_eq!(list.selected(), 59, "selection clamps at the last row");
        let rendered_window = list.window(10);
        assert_eq!(
            rendered_window,
            50..60,
            "the 10-row window a render pass draws tracks the real scrolled \
             position, non-zero"
        );
    }

    #[test]
    fn lines_pane_window_tracks_the_rendered_position_past_content_end() {
        // The opposite-direction sibling of the list-pane case: scrolling
        // past a short pane's content must still resolve to the LAST valid
        // window, not an out-of-range one — `render_lines_pane` sources the
        // rendered lines from this same `ViewportScroll::window(rows)` call.
        let mut scroll = ViewportScroll::default();
        scroll.set_len(23);
        scroll.apply(super::super::tui_kit::ScrollDelta::Down(100), usize::MAX);
        let rendered_window = scroll.window(8);
        assert_eq!(
            rendered_window,
            15..23,
            "an 8-row viewport over 23 lines resolves to the last 8 lines \
             even after scrolling well past the content end"
        );
        assert_ne!(rendered_window.start, 0, "the resolved window is scrolled");
    }
}
