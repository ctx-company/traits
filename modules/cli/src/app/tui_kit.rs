//! P468 shared TUI widget kit: scrollable lists/viewports, the master-detail
//! split, modal dialogs, the single-keypress action model (`ModalHost`),
//! screen chrome, and the `$EDITOR` round-trip primitive. Backs every screen
//! in [`super::dashboard`], [`super::run_view`], and [`super::tui_demo`].
//! P505 adds [`MarkSet`] (multi-select) and [`windowed_list_in_range`]
//! alongside it, plus a `len()` read accessor each on `ScrollList`/
//! `ViewportScroll`; deliberately NOT an `offset()` accessor on either — an
//! `offset` clamped with `rows == usize::MAX` at key-handling time is not a
//! value any consumer should read as "the current position" (the P505
//! review's `scrollbar-position-not-render-offset` blocker), so
//! [`super::tui_panes`]'s scrollbar sources its position from a resolved
//! [`ScrollList::window`]/[`ViewportScroll::window`] instead. The pane tree
//! and chrome themselves live in `tui_panes`, a sibling of this kit rather
//! than more mass here.
//! Two-tone palette doctrine carries over unchanged: named ANSI foreground
//! colors plus `DIM`/`BOLD` modifiers only, no backgrounds, no reverse-video
//! selection — [`Clear`] resets cells to the terminal's own default
//! background rather than painting one, so modal rendering stays compliant.
//! No fixture/snapshot tests here by owner ruling (2026-07-24): every test
//! below asserts state-machine behavior, never a rendered frame.

use std::collections::BTreeSet;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as RLine, Span};
use ratatui::widgets::{Block, Borders, Clear, ListItem, Paragraph};

use super::tui_ratatui::RatatuiPane;

/// Rows moved per page-scroll (`PgUp`/`PgDn`).
const PAGE_STEP: usize = 10;

/// One scroll-key press resolved to a direction/magnitude, decoupled from
/// which widget (a selecting [`ScrollList`] or a selection-less
/// [`ViewportScroll`]) ultimately applies it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ScrollDelta {
    Up(usize),
    Down(usize),
    /// Jump to the first row (Home).
    Start,
    /// Jump to the last row (End).
    End,
}

/// Shared arrows/j-k/PgUp-PgDn key mapping for every scrollable kit widget.
/// Returns `None` for any key with no scroll meaning, so callers can chain it
/// before their own key handling without swallowing unrelated keys.
pub(crate) fn scroll_key(key: &KeyEvent) -> Option<ScrollDelta> {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => Some(ScrollDelta::Up(1)),
        KeyCode::Down | KeyCode::Char('j') => Some(ScrollDelta::Down(1)),
        KeyCode::PageUp => Some(ScrollDelta::Up(PAGE_STEP)),
        KeyCode::PageDown => Some(ScrollDelta::Down(PAGE_STEP)),
        KeyCode::Home | KeyCode::Char('g') => Some(ScrollDelta::Start),
        KeyCode::End | KeyCode::Char('G') => Some(ScrollDelta::End),
        _ => None,
    }
}

/// Selecting scrollable-list state: a current selection plus the scroll
/// offset needed to keep it visible within a rendered rect. `clamp` is the
/// sole authority for keeping `selected`/`offset` in bounds — every mutator
/// funnels through it, so a rect never renders past its own row budget.
#[derive(Default, Clone, Copy, Debug)]
pub(crate) struct ScrollList {
    selected: usize,
    offset: usize,
    len: usize,
}

impl ScrollList {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Sets the backing row count (e.g. after a reload) and re-clamps.
    pub(crate) fn set_len(&mut self, len: usize) {
        self.len = len;
        self.clamp(usize::MAX);
    }

    /// Resets to the first row (e.g. on switching to a different list).
    pub(crate) fn reset(&mut self) {
        self.selected = 0;
        self.offset = 0;
    }

    pub(crate) fn selected(&self) -> usize {
        self.selected
    }

    /// Repoints the selection directly at `index` (e.g. re-locating a row
    /// whose position moved after a reload), re-clamping to the current
    /// length rather than nudging via `move_by`.
    pub(crate) fn set_selected(&mut self, index: usize) {
        self.selected = index;
        self.clamp(usize::MAX);
    }

    /// Applies a resolved [`ScrollDelta`] to the selection: a page-sized
    /// delta (`PgUp`/`PgDn`, per [`scroll_key`]) routes through [`Self::page`],
    /// everything else through [`Self::move_by`].
    pub(crate) fn apply(&mut self, delta: ScrollDelta, rows: usize) {
        match delta {
            ScrollDelta::Up(n) if n == PAGE_STEP => self.page(false, rows),
            ScrollDelta::Down(n) if n == PAGE_STEP => self.page(true, rows),
            ScrollDelta::Up(n) => self.move_by(-(n as i64), rows),
            ScrollDelta::Down(n) => self.move_by(n as i64, rows),
            ScrollDelta::Start => self.set_selected(0),
            ScrollDelta::End => self.set_selected(self.len.saturating_sub(1)),
        }
    }

    pub(crate) fn move_by(&mut self, delta: i64, rows: usize) {
        if self.len == 0 {
            self.selected = 0;
            self.offset = 0;
            return;
        }
        let next = (self.selected as i64 + delta).clamp(0, self.len as i64 - 1);
        self.selected = next as usize;
        self.clamp(rows);
    }

    /// One page (`PAGE_STEP` rows) up/down; `positive` moves down.
    pub(crate) fn page(&mut self, positive: bool, rows: usize) {
        let delta = PAGE_STEP as i64 * if positive { 1 } else { -1 };
        self.move_by(delta, rows);
    }

    /// Keeps `offset` such that `selected` stays within `[offset, offset +
    /// rows)`, and both `selected`/`offset` within `[0, len)`. `rows ==
    /// usize::MAX` (used by `set_len`, before a rect size is known) only
    /// clamps `selected`/`offset` to the list length, deferring the visible
    /// window to the next `visible_range` call.
    pub(crate) fn clamp(&mut self, rows: usize) {
        if self.len == 0 {
            self.selected = 0;
            self.offset = 0;
            return;
        }
        self.selected = self.selected.min(self.len - 1);
        self.offset = self.offset.min(self.selected);
        if rows == 0 {
            return;
        }
        if rows != usize::MAX && self.selected >= self.offset + rows {
            self.offset = self.selected + 1 - rows;
        }
        let max_offset = self.len.saturating_sub(rows.min(self.len));
        self.offset = self.offset.min(max_offset);
    }

    /// The half-open row range visible in a rect of `rows` rows, re-clamping
    /// first so a caller can call this directly after a resize.
    pub(crate) fn visible_range(&mut self, rows: usize) -> std::ops::Range<usize> {
        self.clamp(rows);
        if self.len == 0 || rows == 0 {
            return 0..0;
        }
        let end = (self.offset + rows).min(self.len);
        self.offset..end
    }

    /// A read-only variant of [`Self::visible_range`] for callers that
    /// cannot hold a `&mut` across a render pass (e.g. a widget builder
    /// invoked from inside a `frame.render_widget` closure): computes the
    /// same window on a scratch copy instead of persisting the offset.
    pub(crate) fn window(&self, rows: usize) -> std::ops::Range<usize> {
        let mut scratch = *self;
        scratch.visible_range(rows)
    }

    /// `"12/47"`-style 1-based position indicator; `"0/0"` for an empty list.
    pub(crate) fn position_text(&self) -> String {
        if self.len == 0 {
            "0/0".to_string()
        } else {
            format!("{}/{}", self.selected + 1, self.len)
        }
    }
}

/// Selection-less scroll state for detail/preview panes: only an offset, no
/// current row.
#[derive(Default, Clone, Copy, Debug)]
pub(crate) struct ViewportScroll {
    offset: usize,
    len: usize,
}

impl ViewportScroll {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn set_len(&mut self, len: usize) {
        self.len = len;
        self.clamp(usize::MAX);
    }

    pub(crate) fn apply(&mut self, delta: ScrollDelta, rows: usize) {
        match delta {
            ScrollDelta::Start => self.offset = 0,
            ScrollDelta::End => self.offset = self.len,
            ScrollDelta::Up(n) => {
                self.offset = (self.offset as i64 - n as i64).clamp(0, i64::MAX) as usize;
            }
            ScrollDelta::Down(n) => {
                self.offset = self.offset.saturating_add(n);
            }
        }
        self.clamp(rows);
    }

    pub(crate) fn clamp(&mut self, rows: usize) {
        let max_offset = if rows == usize::MAX {
            self.len
        } else {
            self.len.saturating_sub(rows.min(self.len))
        };
        self.offset = self.offset.min(max_offset);
    }

    pub(crate) fn visible_range(&mut self, rows: usize) -> std::ops::Range<usize> {
        self.clamp(rows);
        if self.len == 0 || rows == 0 {
            return 0..0;
        }
        let end = (self.offset + rows).min(self.len);
        self.offset..end
    }

    /// A read-only variant of [`Self::visible_range`], mirroring
    /// [`ScrollList::window`].
    pub(crate) fn window(&self, rows: usize) -> std::ops::Range<usize> {
        let mut scratch = *self;
        scratch.visible_range(rows)
    }

    /// Whether the window at `rows` already reaches the tail. P470: follow
    /// mode must derive from the RESULTING position after a scroll delta is
    /// applied, never from the key's direction — this is the read callers
    /// use to make that call.
    pub(crate) fn is_at_bottom(&self, rows: usize) -> bool {
        if self.len == 0 {
            return true;
        }
        let max_offset = self.len.saturating_sub(rows.min(self.len));
        self.offset >= max_offset
    }
}

/// The master-detail width-bound rules used by the trait editor: the left pane keeps a fixed floor
/// (`left_min`), the right pane needs at least `right_min` plus `separator`
/// before a split is offered at all, and is capped at `right_max`; any width
/// beyond both flows to the left pane up to its own content width.
pub(crate) struct SplitBounds {
    pub(crate) left_min: u16,
    pub(crate) right_min: u16,
    pub(crate) right_max: u16,
    pub(crate) separator: u16,
}

/// The rectangles a split area resolves to, or a signal that the area is too
/// narrow to split at all.
pub(crate) enum SplitLayout {
    Full(Rect),
    Split {
        left: Rect,
        separator: Rect,
        right: Rect,
    },
}

impl SplitBounds {
    pub(crate) fn compute(&self, area: Rect, left_content_width: u16) -> SplitLayout {
        let min_split = self.left_min + self.separator + self.right_min;
        if area.width < min_split {
            return SplitLayout::Full(area);
        }
        let right_width = (area.width - self.left_min - self.separator).min(self.right_max);
        let after_right = area.width - right_width - self.separator;
        let left_width = after_right.min(left_content_width.max(self.left_min));
        let left = Rect {
            x: area.x,
            y: area.y,
            width: left_width,
            height: area.height,
        };
        let separator = Rect {
            x: area.x + left_width,
            y: area.y,
            width: self.separator,
            height: area.height,
        };
        let right = Rect {
            x: area.x + left_width + self.separator,
            y: area.y,
            width: right_width,
            height: area.height,
        };
        SplitLayout::Split {
            left,
            separator,
            right,
        }
    }
}

/// Master-detail focus, driven by the consistent-key doctrine: `enter`
/// focuses the detail pane, `esc` backs out to the master list. Deliberately
/// not `Tab`, which the dashboard already spends on screen cycling.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Focus {
    #[default]
    Master,
    Detail,
}

impl Focus {
    /// Applies `key` if it means a focus transition from the current state,
    /// returning whether it did (so callers can decide whether the key was
    /// otherwise consumed).
    pub(crate) fn handle_key(&mut self, key: &KeyEvent) -> bool {
        match (*self, key.code) {
            (Focus::Master, KeyCode::Enter) => {
                *self = Focus::Detail;
                true
            }
            (Focus::Detail, KeyCode::Esc) => {
                *self = Focus::Master;
                true
            }
            _ => false,
        }
    }
}

/// A modal dialog's own state machine: `Confirm` (yes/no) or `TextInput`
/// (single- or multi-line). Editing is deliberately minimal — insert,
/// backspace, cursor movement — anything heavier is what the `$EDITOR`
/// round-trip primitive is for.
pub(crate) enum Modal {
    Confirm {
        title: String,
        body: String,
    },
    TextInput {
        title: String,
        /// Optional read-only text rendered above the input (P473 §4.6 —
        /// e.g. every trait a block-approve will touch, so the modal states
        /// what will be trusted before any keypress confirms it). Empty for
        /// every pre-P473 caller, which renders exactly as before.
        body: String,
        input: TextInput,
        multiline: bool,
    },
}

/// Reusable single-line or multi-line Unicode editing state.  Both modal and
/// inline callers use this so cursor semantics cannot drift.
#[derive(Clone, Debug, Default)]
pub(crate) struct TextInput {
    buffer: String,
    /// Char (not byte) index.
    cursor: usize,
}

impl TextInput {
    pub(crate) fn new(seed: impl Into<String>) -> Self {
        let buffer = seed.into();
        let cursor = buffer.chars().count();
        Self { buffer, cursor }
    }

    pub(crate) fn text(&self) -> &str {
        &self.buffer
    }

    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    pub(crate) fn reset(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
    }

    pub(crate) fn handle_key(&mut self, multiline: bool, key: &KeyEvent) -> ModalOutcome {
        text_input_key(&mut self.buffer, &mut self.cursor, multiline, key)
    }
}

/// The result of routing one key through a [`Modal`]. `Pending` means the
/// modal is still open and consumed the key internally (or ignored it);
/// every other variant means the modal has resolved and must be torn down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ModalOutcome {
    Pending,
    Cancelled,
    Confirmed,
    Submitted(String),
}

impl Modal {
    pub(crate) fn confirm(title: impl Into<String>, body: impl Into<String>) -> Self {
        Modal::Confirm {
            title: title.into(),
            body: body.into(),
        }
    }

    pub(crate) fn text_input(
        title: impl Into<String>,
        seed: impl Into<String>,
        multiline: bool,
    ) -> Self {
        Self::text_input_with_body(title, String::new(), seed, multiline)
    }

    /// Like [`Self::text_input`], but with a read-only `body` rendered above
    /// the input (P473 §4.6). An empty `body` renders identically to
    /// [`Self::text_input`] — every existing caller (SESSIONS/TRAITS/MERGES
    /// modals) is byte-identical.
    pub(crate) fn text_input_with_body(
        title: impl Into<String>,
        body: impl Into<String>,
        seed: impl Into<String>,
        multiline: bool,
    ) -> Self {
        Modal::TextInput {
            title: title.into(),
            body: body.into(),
            input: TextInput::new(seed),
            multiline,
        }
    }

    pub(crate) fn title(&self) -> &str {
        match self {
            Modal::Confirm { title, .. } => title,
            Modal::TextInput { title, .. } => title,
        }
    }

    /// Routes one key. Esc always cancels; a confirm dialog accepts
    /// `y`/enter to confirm and `n`/esc to cancel; a single-line input
    /// submits on enter; a multi-line input inserts a newline on enter and
    /// submits on `ctrl-d` (hinted in the modal's own footer row).
    pub(crate) fn handle_key(&mut self, key: &KeyEvent) -> ModalOutcome {
        match self {
            Modal::Confirm { .. } => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => ModalOutcome::Confirmed,
                KeyCode::Char('n') | KeyCode::Esc => ModalOutcome::Cancelled,
                _ => ModalOutcome::Pending,
            },
            Modal::TextInput {
                input, multiline, ..
            } => input.handle_key(*multiline, key),
        }
    }
}

fn char_byte_index(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len())
}

/// Cursor (row, column) within a multi-line buffer, both 0-based, for
/// terminal cursor placement.
fn cursor_row_col(text: &str, char_index: usize) -> (usize, usize) {
    let mut row = 0;
    let mut col = 0;
    for ch in text.chars().take(char_index) {
        if ch == '\n' {
            row += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (row, col)
}

fn text_input_key(
    buffer: &mut String,
    cursor: &mut usize,
    multiline: bool,
    key: &KeyEvent,
) -> ModalOutcome {
    match key.code {
        KeyCode::Esc => ModalOutcome::Cancelled,
        KeyCode::Enter if !multiline => ModalOutcome::Submitted(buffer.clone()),
        KeyCode::Enter if multiline => {
            insert_char(buffer, cursor, '\n');
            ModalOutcome::Pending
        }
        KeyCode::Char('d') if multiline && key.modifiers.contains(KeyModifiers::CONTROL) => {
            ModalOutcome::Submitted(buffer.clone())
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            insert_char(buffer, cursor, ch);
            ModalOutcome::Pending
        }
        KeyCode::Backspace => {
            if *cursor > 0 {
                let start = char_byte_index(buffer, *cursor - 1);
                let end = char_byte_index(buffer, *cursor);
                buffer.replace_range(start..end, "");
                *cursor -= 1;
            }
            ModalOutcome::Pending
        }
        KeyCode::Left => {
            *cursor = cursor.saturating_sub(1);
            ModalOutcome::Pending
        }
        KeyCode::Right => {
            *cursor = (*cursor + 1).min(buffer.chars().count());
            ModalOutcome::Pending
        }
        KeyCode::Home => {
            *cursor = 0;
            ModalOutcome::Pending
        }
        KeyCode::End => {
            *cursor = buffer.chars().count();
            ModalOutcome::Pending
        }
        KeyCode::Up if multiline => {
            let (row, col) = cursor_row_col(buffer, *cursor);
            if row > 0 {
                *cursor = char_index_at(buffer, row - 1, col);
            }
            ModalOutcome::Pending
        }
        KeyCode::Down if multiline => {
            let (row, col) = cursor_row_col(buffer, *cursor);
            let line_count = buffer.chars().filter(|c| *c == '\n').count() + 1;
            if row + 1 < line_count {
                *cursor = char_index_at(buffer, row + 1, col);
            }
            ModalOutcome::Pending
        }
        _ => ModalOutcome::Pending,
    }
}

fn insert_char(buffer: &mut String, cursor: &mut usize, ch: char) {
    let byte = char_byte_index(buffer, *cursor);
    buffer.insert(byte, ch);
    *cursor += 1;
}

/// The char index of column `col` (clamped to the line's length) on line
/// `row` (0-based), counted across `\n`-separated lines.
fn char_index_at(text: &str, row: usize, col: usize) -> usize {
    let mut index = 0;
    for (line_no, line) in text.split('\n').enumerate() {
        let line_len = line.chars().count();
        if line_no == row {
            return index + col.min(line_len);
        }
        index += line_len + 1; // +1 for the '\n' consumed by split.
    }
    text.chars().count()
}

/// The single-keypress action-model plumbing: a screen maps one keypress to
/// an action `Tag` plus the [`Modal`] that confirms/collects it. While a
/// modal is open, every key must route through [`Self::handle_key`] (the
/// focus trap) rather than the screen's own key handling; on resolution the
/// host hands back `(Tag, ModalOutcome)` exactly once and closes. This makes
/// press-twice/double-tap-confirm patterns structurally impossible for kit
/// consumers: there is no path to observe a `Tag` without its modal having
/// resolved.
pub(crate) struct ModalHost<Tag> {
    active: Option<(Tag, Modal)>,
}

impl<Tag> Default for ModalHost<Tag> {
    fn default() -> Self {
        Self { active: None }
    }
}

impl<Tag: Clone> ModalHost<Tag> {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn is_open(&self) -> bool {
        self.active.is_some()
    }

    /// Opens `modal` under `tag` — the one keypress that starts an action.
    pub(crate) fn open(&mut self, tag: Tag, modal: Modal) {
        self.active = Some((tag, modal));
    }

    pub(crate) fn modal(&self) -> Option<&Modal> {
        self.active.as_ref().map(|(_, modal)| modal)
    }

    /// Routes `key` to the open modal, if any. Returns `Some((tag, outcome))`
    /// exactly once per `open` call, on the key that resolves the modal
    /// (cancel, confirm, or submit) — and closes the host in that same call,
    /// so a caller cannot poll a resolved outcome twice.
    pub(crate) fn handle_key(&mut self, key: &KeyEvent) -> Option<(Tag, ModalOutcome)> {
        let (_, modal) = self.active.as_mut()?;
        let outcome = modal.handle_key(key);
        if outcome == ModalOutcome::Pending {
            return None;
        }
        let (tag, _) = self.active.take().expect("checked Some above");
        Some((tag, outcome))
    }
}

/// A multi-select mark set keyed by the caller's own stable row key (e.g. a
/// trait id or digest), never by list index. P473's block-approve trust
/// screen reloads and re-sorts every 2s — index-keyed marks would silently
/// mark the wrong rows after a reorder, the worst possible failure mode on a
/// security-boundary screen — so this is keyed by whatever the caller
/// already treats as identity. [`Self::retain_existing`] drops marks for
/// rows that vanished on reload.
pub(crate) struct MarkSet<K: Ord> {
    marked: BTreeSet<K>,
}

impl<K: Ord> Default for MarkSet<K> {
    fn default() -> Self {
        Self {
            marked: BTreeSet::new(),
        }
    }
}

impl<K: Ord + Clone> MarkSet<K> {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Flips `key`'s mark: unmarks it if already marked, marks it otherwise.
    pub(crate) fn toggle(&mut self, key: K) {
        if !self.marked.remove(&key) {
            self.marked.insert(key);
        }
    }

    pub(crate) fn contains(&self, key: &K) -> bool {
        self.marked.contains(key)
    }

    pub(crate) fn len(&self) -> usize {
        self.marked.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.marked.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.marked.clear();
    }

    /// Drops every mark whose key is no longer present in `existing` (e.g.
    /// after a reload) — the count stays correct across a reorder since keys,
    /// not positions, are what is compared.
    pub(crate) fn retain_existing(&mut self, existing: &[K]) {
        let existing: BTreeSet<&K> = existing.iter().collect();
        self.marked.retain(|key| existing.contains(key));
    }
}

/// Windows `items` to an ALREADY-RESOLVED `window`, rendering a leading
/// two-column marker glyph (`"✓ "`/`"  "`, so columns never shift depending
/// on mark state) per `is_marked`, and the landed selection highlight (green
/// fg) on `selected` — the single builder every consumer with a mark
/// predicate renders its rows through, so a second hand-rolled window+mark
/// loop never grows beside this one. Takes the window rather than computing
/// its own so a caller that also needs it for something else in the same
/// frame ([`super::tui_panes::render_list_pane`]'s scrollbar — the P505
/// review's `scrollbar-position-not-render-offset` blocker) computes it
/// exactly once and passes it to both, rather than risking two separate
/// `ScrollList::window` calls silently drifting apart.
pub(crate) fn windowed_list_in_range<T>(
    items: &[T],
    window: std::ops::Range<usize>,
    selected: usize,
    label: impl Fn(&T) -> String,
    is_marked: impl Fn(&T) -> bool,
) -> Vec<ListItem<'static>> {
    items
        .iter()
        .enumerate()
        .skip(window.start)
        .take(window.len())
        .map(|(idx, item)| {
            let marker = if is_marked(item) { "✓ " } else { "  " };
            let item_widget = ListItem::new(format!("{marker}{}", label(item)));
            if idx == selected {
                item_widget.style(Style::default().fg(Color::Green))
            } else {
                item_widget
            }
        })
        .collect()
}

/// Draws the dim `│` separator column a master-detail split's own
/// `SplitLayout::Split { separator, .. }` rect resolves to, for the rect's
/// full height — the one implementation every split consumer paints its
/// separator through (P244 fix `separator-column-third-copy`: this used to
/// be reimplemented once per screen, and had already drifted to two
/// spellings of the identical `DIM`-only style).
pub(crate) fn render_separator_column(frame: &mut ratatui::Frame<'_>, rect: Rect) {
    let rows = (0..rect.height)
        .map(|_| {
            RLine::from(Span::styled(
                "│",
                Style::default().add_modifier(Modifier::DIM),
            ))
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(rows), rect);
}

/// A left-aligned bold title bar, one row tall.
pub(crate) fn title_bar(title: impl Into<String>) -> Paragraph<'static> {
    Paragraph::new(RLine::from(Span::styled(
        title.into(),
        Style::default().add_modifier(Modifier::BOLD),
    )))
}

/// A dim keymap-hints row, plus an optional message row beneath it — the
/// same shape every screen's status footer renders.
pub(crate) fn keymap_footer(hints: impl Into<String>, message: Option<&str>) -> Paragraph<'static> {
    let mut lines = vec![RLine::from(Span::styled(
        hints.into(),
        Style::default().add_modifier(Modifier::DIM),
    ))];
    if let Some(message) = message {
        lines.push(RLine::from(message.to_string()));
    }
    Paragraph::new(lines)
}

/// A rect of `width` x `height` centered within `area`, clamped so it never
/// exceeds `area`'s own bounds.
fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn wrapped_rows(text: &str, width: u16) -> u16 {
    let width = usize::from(width.max(1));
    text.split('\n')
        .map(|line| line.chars().count().max(1).div_ceil(width) as u16)
        .sum()
}

/// Hard-wrap `text` to `width` columns, splitting each source line into
/// `width`-character chunks (never mid-multi-byte-char, since it slices on
/// `chars()`). Row count matches [`wrapped_rows`] exactly — both walk the
/// same char-chunking rule — so a caller that sized a modal from
/// `wrapped_rows` gets back precisely that many rendered rows from this.
fn wrap_text_lines(text: &str, width: u16) -> Vec<String> {
    let width = usize::from(width.max(1));
    let mut lines = Vec::new();
    for line in text.split('\n') {
        if line.is_empty() {
            lines.push(String::new());
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        for chunk in chars.chunks(width) {
            lines.push(chunk.iter().collect());
        }
    }
    lines
}

/// Hard-wrap a `TextInput` modal's read-only `body` to `width` columns and
/// truncate it to whatever rows fit in `inner_height` after reserving room
/// for `buffer_line_count` input rows plus the surrounding blank/hint rows
/// (blocker: answer-modal-body-overflow-unhandled — the input row and submit
/// hint must always stay on-screen with typed input visible, and any elided
/// question text must carry an explicit marker rather than clipping
/// silently). Returns the rows to render and how many were hidden; `0` hidden
/// rows for a body that already fits renders byte-identical to the
/// pre-budget behavior.
fn budgeted_body_lines(
    body: &str,
    width: u16,
    inner_height: u16,
    buffer_line_count: u16,
) -> (Vec<String>, usize) {
    if body.is_empty() {
        return (Vec::new(), 0);
    }
    let footer_rows = buffer_line_count + 2;
    let body_reserved = footer_rows + 1;
    let available_for_body = inner_height.saturating_sub(body_reserved) as usize;
    let wrapped_body = wrap_text_lines(body, width);
    if wrapped_body.len() <= available_for_body {
        return (wrapped_body, 0);
    }
    let budget = available_for_body.saturating_sub(1);
    let shown = wrapped_body.len().min(budget);
    let hidden = wrapped_body.len() - shown;
    (wrapped_body[..shown].to_vec(), hidden)
}

fn confirm_size(area: Rect, body: &str) -> (u16, u16) {
    let width = area.width.clamp(1, 70);
    // Borders consume two columns; the hint is one additional wrapped row.
    let inner_width = width.saturating_sub(2).max(1);
    let height = (wrapped_rows(body, inner_width) + 2 + 2)
        .min(area.height)
        .max(1);
    (width, height)
}

/// Draws `modal` centered over `area`: [`Clear`] resets those cells to the
/// terminal's own default background (it paints no color, so the
/// no-backgrounds doctrine holds) then a dim single-line border frames the
/// dialog. Text-input cursor placement uses the real terminal cursor
/// (`frame.set_cursor_position`), never reverse-video.
pub(crate) fn render_modal(frame: &mut ratatui::Frame<'_>, area: Rect, modal: &Modal) {
    let (width, height) = match modal {
        Modal::Confirm { body, .. } => confirm_size(area, body),
        Modal::TextInput {
            multiline, body, ..
        } => {
            let width = 70u16.min(area.width.max(1));
            let inner_width = width.saturating_sub(2).max(1);
            let body_rows = if body.is_empty() {
                0
            } else {
                wrapped_rows(body, inner_width) + 1
            };
            let base = if *multiline { 12 } else { 5 };
            (width, base + body_rows)
        }
    };
    let rect = centered_rect(area, width, height);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().add_modifier(Modifier::DIM))
        .title(modal.title().to_string());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    match modal {
        Modal::Confirm { body, .. } => {
            let lines = vec![
                RLine::from(body.clone()),
                RLine::default(),
                RLine::from(Span::styled(
                    "y/enter confirm  n/esc cancel",
                    Style::default().add_modifier(Modifier::DIM),
                )),
            ];
            frame.render_widget(
                Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }),
                inner,
            );
        }
        Modal::TextInput {
            body,
            input,
            multiline,
            ..
        } => {
            let hint = if *multiline {
                "ctrl-d submit  esc cancel"
            } else {
                "enter submit  esc cancel"
            };
            // The input line and submit hint must always stay on-screen with
            // typed input visible (blocker: answer-modal-body-overflow-unhandled).
            let buffer_line_count = input.text().split('\n').count() as u16;
            let (rendered_body, hidden_count) =
                budgeted_body_lines(body, inner.width, inner.height, buffer_line_count);
            let mut lines: Vec<RLine<'static>> = Vec::new();
            let body_row_offset = if body.is_empty() {
                0
            } else {
                for line in &rendered_body {
                    lines.push(RLine::from(Span::styled(
                        line.clone(),
                        Style::default().add_modifier(Modifier::DIM),
                    )));
                }
                if hidden_count > 0 {
                    lines.push(RLine::from(Span::styled(
                        format!("… (truncated; {hidden_count} more lines)"),
                        Style::default().add_modifier(Modifier::DIM),
                    )));
                }
                lines.push(RLine::default());
                rendered_body.len() as u16 + u16::from(hidden_count > 0) + 1
            };
            lines.extend(input.text().split('\n').map(|l| RLine::from(l.to_string())));
            lines.push(RLine::default());
            lines.push(RLine::from(Span::styled(
                hint,
                Style::default().add_modifier(Modifier::DIM),
            )));
            frame.render_widget(Paragraph::new(lines), inner);
            let (row, col) = cursor_row_col(input.text(), input.cursor());
            let cursor_x = inner.x + (col as u16).min(inner.width.saturating_sub(1));
            let cursor_y =
                inner.y + (body_row_offset + row as u16).min(inner.height.saturating_sub(1));
            frame.set_cursor_position((cursor_x, cursor_y));
        }
    }
}

/// Renders a persistent, single-line conversation modal. The caller owns the
/// exchange data and dispatch lifecycle; this reusable shape owns only modal
/// geometry, wrapping, viewport clamping, and input cursor placement.
pub(crate) fn render_conversation_modal(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    title: &str,
    rows: &[String],
    input: &TextInput,
    scroll: &mut ViewportScroll,
    follow: bool,
) {
    let width = area.width.clamp(1, 70);
    let height = area.height.clamp(1, 18);
    let rect = centered_rect(area, width, height);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().add_modifier(Modifier::DIM))
        .title(title.to_string());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let body_height = conversation_body_rows(area) as u16;
    let mut wrapped = Vec::new();
    for row in rows {
        wrapped.extend(wrap_text_lines(row, inner.width));
    }
    scroll.set_len(wrapped.len());
    if follow {
        scroll.apply(ScrollDelta::End, body_height as usize);
    } else {
        scroll.clamp(body_height as usize);
    }
    let visible = scroll.window(body_height as usize);
    if body_height > 0 {
        frame.render_widget(
            Paragraph::new(
                wrapped[visible]
                    .iter()
                    .map(|line| RLine::from(line.clone()))
                    .collect::<Vec<_>>(),
            ),
            Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: body_height,
            },
        );
    }
    let input_y = inner.y + inner.height.saturating_sub(2);
    let (visible_input, input_cursor_col) = conversation_input_window(input, inner.width);
    frame.render_widget(
        Paragraph::new(vec![
            RLine::from(format!("Ask: {visible_input}")),
            RLine::from(Span::styled(
                "enter send  esc/q close  arrows/pg scroll",
                Style::default().add_modifier(Modifier::DIM),
            )),
        ]),
        Rect {
            x: inner.x,
            y: input_y,
            width: inner.width,
            height: inner.height.saturating_sub(body_height),
        },
    );
    frame.set_cursor_position((
        inner.x + (5u16.saturating_add(input_cursor_col as u16)).min(inner.width.saturating_sub(1)),
        input_y,
    ));
}

/// Returns the input window and the cursor's terminal-column offset within it.
/// `TextInput` indexes chars, while terminals position cursors in display cells.
fn conversation_input_window(input: &TextInput, width: u16) -> (String, usize) {
    let available = width.saturating_sub(5) as usize;
    let chars: Vec<char> = input.text().chars().collect();
    let cursor = input.cursor().min(chars.len());
    let char_width = |ch: char| {
        let mut buf = [0u8; 4];
        Span::raw(ch.encode_utf8(&mut buf) as &str).width()
    };
    // Keep one terminal cell after the cursor. `Frame` cannot place a cursor
    // at the right edge, and clamping there can land inside a wide glyph.
    let cursor_available = available.saturating_sub(1);
    let mut start = cursor;
    let mut used = 0;
    while start > 0 {
        let next = char_width(chars[start - 1]);
        if used + next > cursor_available {
            break;
        }
        start -= 1;
        used += next;
    }
    let cursor_col = used;
    let mut end = cursor;
    while end < chars.len() {
        let next = char_width(chars[end]);
        if used + next > available {
            break;
        }
        used += next;
        end += 1;
    }
    (chars[start..end].iter().collect(), cursor_col)
}

/// The history viewport shared by conversation key reducers and rendering.
pub(crate) fn conversation_body_rows(area: Rect) -> usize {
    let height = area.height.clamp(1, 18);
    height.saturating_sub(2).saturating_sub(2) as usize
}

/// Suspends the pane and runs `$EDITOR` on `path` in place, returning
/// whether it exited zero. Moved from `dashboard::edit_selected_trait_source`
/// and generalized over the path.
pub(crate) fn edit_file(pane: &mut RatatuiPane, path: &camino::Utf8Path) -> crate::Result<bool> {
    pane.suspend(|| ctx_traits_io::process::run_editor_inherited(path).map_err(|e| e.to_string()))
        .map_err(|source| {
            ctx_traits_io::Error::from(ctx_traits_io::environment::Error::Filesystem {
                path: path.to_string(),
                source,
            })
        })?
        .map_err(|message| crate::Error::Command { message })
}

#[cfg(test)]
mod tests {
    #[test]
    fn text_input_shared_unicode_reset_and_submit() {
        let mut input = super::TextInput::default();
        assert!(matches!(
            input.handle_key(false, &KeyEvent::from(KeyCode::Char('é'))),
            super::ModalOutcome::Pending
        ));
        assert_eq!(input.text(), "é");
        input.reset();
        assert_eq!(input.text(), "");
        assert_eq!(
            input.handle_key(false, &KeyEvent::from(KeyCode::Enter)),
            super::ModalOutcome::Submitted(String::new())
        );
    }

    #[test]
    fn text_input_unicode_movement_deletion_reset_and_modal_submit() {
        let mut input = TextInput::new("aéz");
        input.handle_key(false, &KeyEvent::from(KeyCode::Left));
        input.handle_key(false, &KeyEvent::from(KeyCode::Backspace));
        assert_eq!(input.text(), "az");
        assert_eq!(input.cursor(), 1);
        input.handle_key(false, &KeyEvent::from(KeyCode::Char('界')));
        assert_eq!(input.text(), "a界z");
        input.reset();
        assert_eq!(input.text(), "");
        let mut modal = Modal::text_input("title", "é", false);
        assert_eq!(
            modal.handle_key(&KeyEvent::from(KeyCode::Enter)),
            ModalOutcome::Submitted("é".to_string())
        );
    }
    use super::*;
    use crossterm::event::KeyEventKind;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn scroll_list_empty_stays_at_zero() {
        let mut list = ScrollList::new();
        list.set_len(0);
        list.move_by(5, 10);
        assert_eq!(list.selected(), 0);
        assert_eq!(list.position_text(), "0/0");
        assert_eq!(list.visible_range(10), 0..0);
    }

    #[test]
    fn scroll_list_one_row() {
        let mut list = ScrollList::new();
        list.set_len(1);
        list.move_by(5, 3);
        assert_eq!(list.selected(), 0);
        assert_eq!(list.position_text(), "1/1");
    }

    #[test]
    fn scroll_list_clamps_and_scrolls_overflowing() {
        let mut list = ScrollList::new();
        list.set_len(50);
        assert_eq!(list.visible_range(10), 0..10);
        list.move_by(9, 10);
        assert_eq!(list.selected(), 9);
        assert_eq!(list.visible_range(10), 0..10);
        list.move_by(1, 10);
        assert_eq!(list.selected(), 10);
        assert_eq!(list.visible_range(10), 1..11);
        list.move_by(-100, 10);
        assert_eq!(list.selected(), 0);
        assert_eq!(list.visible_range(10), 0..10);
        list.move_by(100, 10);
        assert_eq!(list.selected(), 49);
        assert_eq!(list.position_text(), "50/50");
        assert_eq!(list.visible_range(10), 40..50);
    }

    #[test]
    fn scroll_list_paging() {
        let mut list = ScrollList::new();
        list.set_len(100);
        list.page(true, 10);
        assert_eq!(list.selected(), PAGE_STEP);
        list.page(false, 10);
        assert_eq!(list.selected(), 0);
    }

    #[test]
    fn scroll_key_maps_arrows_and_paging() {
        assert_eq!(scroll_key(&key(KeyCode::Down)), Some(ScrollDelta::Down(1)));
        assert_eq!(
            scroll_key(&key(KeyCode::Char('j'))),
            Some(ScrollDelta::Down(1))
        );
        assert_eq!(scroll_key(&key(KeyCode::Up)), Some(ScrollDelta::Up(1)));
        assert_eq!(
            scroll_key(&key(KeyCode::Char('k'))),
            Some(ScrollDelta::Up(1))
        );
        assert_eq!(
            scroll_key(&key(KeyCode::PageDown)),
            Some(ScrollDelta::Down(PAGE_STEP))
        );
        assert_eq!(
            scroll_key(&key(KeyCode::PageUp)),
            Some(ScrollDelta::Up(PAGE_STEP))
        );
        assert_eq!(scroll_key(&key(KeyCode::Char('z'))), None);
    }

    #[test]
    fn viewport_scroll_clamps_to_content() {
        let mut viewport = ViewportScroll::new();
        viewport.set_len(30);
        viewport.apply(ScrollDelta::Down(100), 10);
        assert_eq!(viewport.window(10), 20..30);
        viewport.apply(ScrollDelta::Up(100), 10);
        assert_eq!(viewport.window(10), 0..10);
    }

    // P470 blocker `down-key-forces-follow-jump`: `is_at_bottom` is the
    // POSITION check callers derive follow-mode from — never the key's own
    // direction — so a single step up from the tail reads as not-at-bottom,
    // and stepping back down one row reads as at-bottom again.
    #[test]
    fn viewport_scroll_is_at_bottom_reflects_position_not_direction() {
        let mut viewport = ViewportScroll::new();
        viewport.set_len(30);
        viewport.apply(ScrollDelta::Down(100), 10);
        assert!(viewport.is_at_bottom(10));
        viewport.apply(ScrollDelta::Up(1), 10);
        assert!(!viewport.is_at_bottom(10));
        viewport.apply(ScrollDelta::Down(1), 10);
        assert!(viewport.is_at_bottom(10));
    }

    #[test]
    fn viewport_scroll_is_at_bottom_holds_for_empty_content() {
        let viewport = ViewportScroll::new();
        assert!(viewport.is_at_bottom(10));
    }

    #[test]
    fn guide_conversation_modal_places_unicode_cursor_by_display_width() {
        let mut input = TextInput::new("a界b");
        input.handle_key(false, &key(KeyCode::End));
        assert_eq!(
            conversation_input_window(&input, 20),
            ("a界b".to_string(), 4)
        );

        // The input window follows the edited cursor instead of clipping the
        // suffix and leaving the cursor at an unrelated terminal column.
        let input = TextInput::new("abcdefgh界");
        assert_eq!(conversation_input_window(&input, 9), ("h界".to_string(), 3));

        use ratatui::{Terminal, backend::TestBackend, layout::Position};
        let mut terminal = Terminal::new(TestBackend::new(11, 6)).expect("terminal");
        let mut scroll = ViewportScroll::new();
        terminal
            .draw(|frame| {
                render_conversation_modal(
                    frame,
                    frame.area(),
                    "Guide",
                    &[],
                    &input,
                    &mut scroll,
                    true,
                );
            })
            .expect("draw");
        let rendered: String = (0..11)
            .map(|x| {
                terminal
                    .backend()
                    .buffer()
                    .cell((x, 3))
                    .expect("cell")
                    .symbol()
            })
            .collect();
        assert!(rendered.contains("h界"));
        assert_eq!(
            terminal.get_cursor_position().expect("cursor"),
            Position::new(9, 3)
        );
    }

    // Exercises the trait editor's master-detail width policy independently of
    // the live run pane, which uses the pane tree directly.
    const TREE_MIN_WIDTH: u16 = 72;
    const SIDEBAR_MIN_WIDTH: u16 = 36;
    const SIDEBAR_MAX_WIDTH: u16 = 60;
    const SEPARATOR_WIDTH: u16 = 1;

    fn live_bounds() -> SplitBounds {
        SplitBounds {
            left_min: TREE_MIN_WIDTH,
            right_min: SIDEBAR_MIN_WIDTH,
            right_max: SIDEBAR_MAX_WIDTH,
            separator: SEPARATOR_WIDTH,
        }
    }

    fn area(width: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width,
            height: 24,
        }
    }

    #[test]
    fn split_bounds_narrow_area_is_full_width() {
        let layout = live_bounds().compute(area(100), 200);
        assert!(matches!(layout, SplitLayout::Full(rect) if rect.width == 100));
    }

    #[test]
    fn split_bounds_wide_area_clamps_sidebar_and_floors_tree() {
        match live_bounds().compute(area(200), 200) {
            SplitLayout::Split {
                left,
                separator,
                right,
            } => {
                assert_eq!(right.width, SIDEBAR_MAX_WIDTH);
                assert_eq!(separator.width, SEPARATOR_WIDTH);
                assert_eq!(left.width, 200 - SIDEBAR_MAX_WIDTH - SEPARATOR_WIDTH);
            }
            SplitLayout::Full(_) => panic!("expected a split"),
        }
    }

    #[test]
    fn split_bounds_edge_at_min_split_width_still_splits() {
        let min_split = TREE_MIN_WIDTH + SEPARATOR_WIDTH + SIDEBAR_MIN_WIDTH;
        match live_bounds().compute(area(min_split), 200) {
            SplitLayout::Split { left, right, .. } => {
                assert_eq!(left.width, TREE_MIN_WIDTH);
                assert_eq!(right.width, SIDEBAR_MIN_WIDTH);
            }
            SplitLayout::Full(_) => panic!("expected a split at the floor width"),
        }
        match live_bounds().compute(area(min_split - 1), 200) {
            SplitLayout::Full(_) => {}
            SplitLayout::Split { .. } => panic!("expected full width one column below the floor"),
        }
    }

    #[test]
    fn focus_moves_on_enter_and_esc_only() {
        let mut focus = Focus::Master;
        assert!(!focus.handle_key(&key(KeyCode::Esc)));
        assert_eq!(focus, Focus::Master);
        assert!(focus.handle_key(&key(KeyCode::Enter)));
        assert_eq!(focus, Focus::Detail);
        assert!(!focus.handle_key(&key(KeyCode::Enter)));
        assert!(focus.handle_key(&key(KeyCode::Esc)));
        assert_eq!(focus, Focus::Master);
    }

    #[test]
    fn confirm_modal_routes_yes_no_and_esc() {
        let mut modal = Modal::confirm("title", "body");
        assert_eq!(
            modal.handle_key(&key(KeyCode::Char('z'))),
            ModalOutcome::Pending
        );
        assert_eq!(
            modal.handle_key(&key(KeyCode::Char('y'))),
            ModalOutcome::Confirmed
        );

        let mut modal = Modal::confirm("title", "body");
        assert_eq!(
            modal.handle_key(&key(KeyCode::Enter)),
            ModalOutcome::Confirmed
        );

        let mut modal = Modal::confirm("title", "body");
        assert_eq!(
            modal.handle_key(&key(KeyCode::Char('n'))),
            ModalOutcome::Cancelled
        );

        let mut modal = Modal::confirm("title", "body");
        assert_eq!(
            modal.handle_key(&key(KeyCode::Esc)),
            ModalOutcome::Cancelled
        );
    }

    #[test]
    fn confirm_size_wraps_long_artifact_lists_and_clamps_to_terminal() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 12,
        };
        let body = "/very/long/path/to/a/session/with/many/nested/components.json\n/another/very/long/path/to/a/worktree/with/many/nested/components";
        let (width, height) = confirm_size(area, body);
        assert_eq!(width, 40);
        assert!(height > 6);
        assert!(height <= area.height);
    }

    #[test]
    fn single_line_input_submits_on_enter() {
        let mut modal = Modal::text_input("title", "", false);
        for ch in "hi".chars() {
            assert_eq!(
                modal.handle_key(&key(KeyCode::Char(ch))),
                ModalOutcome::Pending
            );
        }
        assert_eq!(
            modal.handle_key(&key(KeyCode::Enter)),
            ModalOutcome::Submitted("hi".to_string())
        );
    }

    // P473 §4.6: `text_input` (no body) must produce an empty body — every
    // pre-P473 modal caller renders unchanged — while
    // `text_input_with_body` carries the body through and still resolves
    // key handling identically (the body is display-only, never routed
    // through `handle_key`).
    #[test]
    fn text_input_body_defaults_empty_and_carries_through_with_body() {
        let modal = Modal::text_input("t", "seed", false);
        if let Modal::TextInput { body, .. } = &modal {
            assert!(body.is_empty());
        } else {
            panic!("expected text input");
        }

        let mut modal =
            Modal::text_input_with_body("t", "trait-a\ntrait-b\n+3 more", "seed", false);
        if let Modal::TextInput { body, .. } = &modal {
            assert_eq!(body, "trait-a\ntrait-b\n+3 more");
        } else {
            panic!("expected text input");
        }
        assert_eq!(
            modal.handle_key(&key(KeyCode::Esc)),
            ModalOutcome::Cancelled
        );
    }

    #[test]
    fn single_line_input_esc_cancels_without_value() {
        let mut modal = Modal::text_input("title", "seed", false);
        assert_eq!(
            modal.handle_key(&key(KeyCode::Esc)),
            ModalOutcome::Cancelled
        );
    }

    #[test]
    fn multiline_input_enter_inserts_newline_and_ctrl_d_submits() {
        let mut modal = Modal::text_input("title", "", true);
        for ch in "a".chars() {
            modal.handle_key(&key(KeyCode::Char(ch)));
        }
        assert_eq!(
            modal.handle_key(&key(KeyCode::Enter)),
            ModalOutcome::Pending
        );
        for ch in "b".chars() {
            modal.handle_key(&key(KeyCode::Char(ch)));
        }
        assert_eq!(
            modal.handle_key(&ctrl(KeyCode::Char('d'))),
            ModalOutcome::Submitted("a\nb".to_string())
        );
    }

    #[test]
    fn multiline_input_backspace_and_cursor_moves() {
        let mut modal = Modal::text_input("title", "ab", true);
        // cursor starts at end (2); left, left -> 0; Home is a no-op; type 'x'.
        modal.handle_key(&key(KeyCode::Left));
        modal.handle_key(&key(KeyCode::Left));
        modal.handle_key(&key(KeyCode::Home));
        assert_eq!(
            modal.handle_key(&key(KeyCode::Char('x'))),
            ModalOutcome::Pending
        );
        if let Modal::TextInput { input, .. } = &modal {
            assert_eq!(input.text(), "xab");
        } else {
            panic!("expected text input");
        }
        modal.handle_key(&key(KeyCode::End));
        assert_eq!(
            modal.handle_key(&key(KeyCode::Backspace)),
            ModalOutcome::Pending
        );
        if let Modal::TextInput { input, .. } = &modal {
            assert_eq!(input.text(), "xa");
        } else {
            panic!("expected text input");
        }
    }

    #[test]
    fn modal_host_dispatches_exactly_once_on_resolution() {
        let mut host: ModalHost<&'static str> = ModalHost::new();
        assert!(!host.is_open());
        assert_eq!(host.handle_key(&key(KeyCode::Char('y'))), None);

        host.open("approve", Modal::confirm("t", "b"));
        assert!(host.is_open());
        // A key that leaves the modal Pending: no dispatch, still open.
        assert_eq!(host.handle_key(&key(KeyCode::Char('z'))), None);
        assert!(host.is_open());

        let resolved = host.handle_key(&key(KeyCode::Char('y')));
        assert_eq!(resolved, Some(("approve", ModalOutcome::Confirmed)));
        assert!(!host.is_open());
        // Resolution fires exactly once: the same key again hits a closed host.
        assert_eq!(host.handle_key(&key(KeyCode::Char('y'))), None);
    }

    #[test]
    fn modal_host_focus_trap_cancel_never_yields_a_value() {
        let mut host: ModalHost<&'static str> = ModalHost::new();
        host.open("rename", Modal::text_input("t", "seed", false));
        let resolved = host.handle_key(&key(KeyCode::Esc));
        assert_eq!(resolved, Some(("rename", ModalOutcome::Cancelled)));
    }

    #[test]
    fn mark_set_toggle_flips_membership() {
        let mut marks: MarkSet<&'static str> = MarkSet::new();
        assert!(!marks.contains(&"a"));
        marks.toggle("a");
        assert!(marks.contains(&"a"));
        assert_eq!(marks.len(), 1);
        marks.toggle("a");
        assert!(!marks.contains(&"a"));
        assert!(marks.is_empty());
    }

    #[test]
    fn mark_set_retain_existing_drops_vanished_keys() {
        let mut marks: MarkSet<&'static str> = MarkSet::new();
        marks.toggle("a");
        marks.toggle("b");
        marks.toggle("c");
        assert_eq!(marks.len(), 3);
        marks.retain_existing(&["b", "c", "d"]);
        assert!(!marks.contains(&"a"));
        assert!(marks.contains(&"b"));
        assert!(marks.contains(&"c"));
        assert_eq!(marks.len(), 2);
    }

    #[test]
    fn mark_set_count_stable_across_reorder() {
        let mut marks: MarkSet<&'static str> = MarkSet::new();
        marks.toggle("a");
        marks.toggle("b");
        // A reorder is just a different `existing` order; membership by key
        // is unaffected.
        marks.retain_existing(&["b", "a"]);
        assert_eq!(marks.len(), 2);
        assert!(marks.contains(&"a"));
        assert!(marks.contains(&"b"));
    }

    #[test]
    fn windowed_list_in_range_marks_and_windows_without_shifting_columns() {
        let items = vec!["r0", "r1", "r2", "r3", "r4"];
        let mut list = ScrollList::new();
        list.set_len(items.len());
        list.move_by(3, 2); // selects index 3, scrolls window forward.
        let window = list.window(2);
        let rendered = windowed_list_in_range(
            &items,
            window,
            list.selected(),
            |s| s.to_string(),
            |s| *s == "r3",
        );
        assert_eq!(rendered.len(), 2);
    }

    #[test]
    fn key_event_kind_press_matches_pump_filter() {
        // Sanity check that the kit's own `KeyEvent::new` helper matches the
        // `Press` kind the pump thread forwards (see `tui_ratatui`), so unit
        // tests here exercise the same shape of event real input produces.
        assert_eq!(key(KeyCode::Enter).kind, KeyEventKind::Press);
    }

    #[test]
    fn wrap_text_lines_matches_wrapped_rows_count() {
        let body = "a short line\nand another one that is a bit longer than ten chars";
        let width = 10;
        let wrapped = wrap_text_lines(body, width);
        assert_eq!(wrapped.len() as u16, wrapped_rows(body, width));
        // Reassembling (ignoring the hard-wrap boundaries) preserves content.
        assert_eq!(wrapped.concat(), body.replace('\n', ""));
    }

    #[test]
    fn wrap_text_lines_preserves_short_lines_unchanged() {
        // A line shorter than the width renders as a single unmodified row —
        // this is what keeps existing short-body modals byte-identical.
        let body = "answer slot: slot:x (schema: schema:text)";
        let wrapped = wrap_text_lines(body, 70);
        assert_eq!(wrapped, vec![body.to_string()]);
    }

    #[test]
    fn budgeted_body_lines_fits_without_truncation() {
        let body = "line one\nline two";
        let (lines, hidden) = budgeted_body_lines(body, 68, 12, 1);
        assert_eq!(lines, vec!["line one".to_string(), "line two".to_string()]);
        assert_eq!(hidden, 0);
    }

    #[test]
    fn budgeted_body_lines_empty_body_stays_empty() {
        let (lines, hidden) = budgeted_body_lines("", 68, 12, 1);
        assert!(lines.is_empty());
        assert_eq!(hidden, 0);
    }

    #[test]
    fn budgeted_body_lines_truncates_and_marks_overflow() {
        // 20 body rows into a modal with room for only 6 rows total (minus
        // footer: 1 buffer row + blank + hint = 3 reserved, plus the leading
        // blank after the body = 4) leaves 2 rows for the body — 1 shown, 1
        // reserved for the truncation marker itself.
        let body = (0..20)
            .map(|i| format!("row {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (lines, hidden) = budgeted_body_lines(&body, 68, 6, 1);
        assert_eq!(lines, vec!["row 0".to_string()]);
        assert_eq!(hidden, 19);
    }

    #[test]
    fn budgeted_body_lines_wraps_a_single_long_line() {
        // A single line wider than the modal must wrap, not clip silently.
        let body = "a".repeat(25);
        let (lines, hidden) = budgeted_body_lines(&body, 10, 20, 1);
        assert_eq!(lines, vec!["a".repeat(10), "a".repeat(10), "a".repeat(5)]);
        assert_eq!(hidden, 0);
    }
}
