//! Shared scrollback-output kit for human-facing CLI commands (P465).
//!
//! Locks the ctx.gate-style panel grammar every migrated command reuses: a
//! `╭─ product ─ headline` top border, a `│` gutter on every body/section
//! line, ordered rows and sections, aligned lowercase-labelled rows, an
//! explicit `next` action row, and a `╰─ state` closing line. Styled output
//! reuses [`tui`]'s named ANSI/faint palette; anywhere styling is
//! unavailable (non-TTY, `NO_COLOR`, `CI`, `TERM=dumb`) the same semantic
//! content renders as plain indented text with no ANSI or box glyphs.
//!
//! Domain facts stay with callers: this module only retains what it is
//! given (labels, values, section titles, status, closing state) and lays
//! it out unchanged. Only row labels are normalized (lowercased) — product,
//! headline, section titles, values, and the closing state render exactly
//! as supplied. `doctor`'s default source-inspection report (P466) is the
//! kit's first adopter, driving its compact panel and the `--json`/human/
//! `--verbose` output selection.

use crate::app::tui::{self, Line, Tone};

/// Cap on a panel's total column width, independent of the terminal — long
/// enough for real command output, short enough to stay readable.
const MAX_PANEL_WIDTH: usize = 96;
/// Floor on the label column so a handful of long labels can't collapse it
/// to nothing on a normal-width terminal; still overridden by the frame's
/// own width bound on narrower terminals so no rendered line can exceed it.
const MIN_LABEL_COLUMN: usize = 8;
/// The label column never exceeds this fraction of the panel width, so a
/// single very long label can't push every value off past the right edge.
const MAX_LABEL_FRACTION: usize = 3;
/// The value column is never narrower than this, even at the label column's
/// widest permitted bound.
const MIN_VALUE_WIDTH: usize = 1;
/// Body indent applied to every row — top-level and nested inside a section
/// alike — beneath the gutter bar, so both read as `│   {label} value`.
const BODY_INDENT: &str = "  ";
/// The locked gutter bar prefixing every body/section content line.
const BAR: &str = "│";

/// The closed set of tones a panel row may use — narrower than [`Tone`]
/// itself so callers cannot reach for `Tone::Warn`/`Tone::Bold` here. Labels
/// and frame furniture are always muted regardless of this value; this tone
/// only shades the row's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowTone {
    Default,
    Pass,
    Fail,
    Warn,
}

impl RowTone {
    fn tone(self) -> Tone {
        match self {
            RowTone::Default => Tone::Default,
            RowTone::Pass => Tone::Pass,
            RowTone::Fail => Tone::Fail,
            RowTone::Warn => Tone::Warn,
        }
    }
}

/// One `label: value` row. The label is lowercased on construction so the
/// house doctrine (lowercase labels) is structural rather than a
/// per-caller convention; the value is preserved exactly as supplied.
#[derive(Clone, Debug)]
pub(crate) struct PanelRow {
    label: String,
    value: String,
    tone: RowTone,
}

impl PanelRow {
    /// Untoned convenience over [`PanelRow::toned`]. `doctor`'s compact
    /// panel — the kit's only production caller so far (P466) — tones every
    /// row it builds, so this constructor currently has no production
    /// caller of its own; kept for the kit's own presentation-layer tests
    /// and future adopters that don't need a tone.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "no untoned production caller yet")
    )]
    pub(crate) fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self::toned(label, value, RowTone::Default)
    }

    /// Sanitizes `label` and `value` through [`tui::clean_live_text`] before
    /// storing them — the boundary sanitization point every panel row goes
    /// through regardless of caller (P467 §3.1b). Idempotent, so callers
    /// that already sanitized their own text (`doctor`'s pre-P467 `safe()`
    /// calls) are unaffected.
    pub(crate) fn toned(label: impl Into<String>, value: impl Into<String>, tone: RowTone) -> Self {
        Self {
            label: tui::clean_live_text(&label.into()).to_lowercase(),
            value: tui::clean_live_text(&value.into()),
            tone,
        }
    }

    fn label_text(&self) -> String {
        format!("{}:", self.label)
    }
}

/// An ordered group of rows under a section title, preserved exactly as
/// supplied (only row labels are lowercased, not section titles).
#[derive(Clone, Debug)]
pub(crate) struct PanelSection {
    title: String,
    rows: Vec<PanelRow>,
}

impl PanelSection {
    /// Sanitizes `title` through [`tui::clean_live_text`] at construction —
    /// rows arrive already sanitized by [`PanelRow::toned`] (P467 §3.1b).
    pub(crate) fn new(title: impl Into<String>, rows: Vec<PanelRow>) -> Self {
        Self {
            title: tui::clean_live_text(&title.into()),
            rows,
        }
    }
}

#[derive(Clone, Debug)]
enum PanelItem {
    Row(PanelRow),
    Section(PanelSection),
}

/// The panel's required closing state. `Blocked` and `Critical` both read as
/// `Tone::Fail` — only `Passed` reads as `Tone::Pass` — matching the house
/// doctrine that anything short of passing renders as a stop. Closing text
/// is preserved exactly as supplied.
#[derive(Clone, Debug)]
pub(crate) enum PanelStatus {
    Passed(String),
    Blocked(String),
    Critical(String),
}

impl PanelStatus {
    fn text(&self) -> &str {
        match self {
            PanelStatus::Passed(text)
            | PanelStatus::Blocked(text)
            | PanelStatus::Critical(text) => text,
        }
    }

    fn tone(&self) -> Tone {
        match self {
            PanelStatus::Passed(_) => Tone::Pass,
            PanelStatus::Blocked(_) | PanelStatus::Critical(_) => Tone::Fail,
        }
    }

    /// Sanitizes the closing text through [`tui::clean_live_text`], preserving
    /// the variant (and therefore the tone). Called by [`Panel::new`] and
    /// [`emit_one_row`] so every path into the boundary is covered.
    fn sanitized(self) -> Self {
        match self {
            PanelStatus::Passed(text) => PanelStatus::Passed(tui::clean_live_text(&text)),
            PanelStatus::Blocked(text) => PanelStatus::Blocked(tui::clean_live_text(&text)),
            PanelStatus::Critical(text) => PanelStatus::Critical(tui::clean_live_text(&text)),
        }
    }
}

/// A structured ctx.gate-style panel: a `╭─ product ─ headline` top border,
/// ordered rows/sections behind a `│` gutter, an optional explicit `next`
/// action row, and a required `╰─ state` closing line. Construct with
/// [`Panel::new`], which requires the product, headline, and closing state
/// up front, then add rows/sections with the builder methods.
#[derive(Clone, Debug)]
pub(crate) struct Panel {
    product: String,
    headline: String,
    items: Vec<PanelItem>,
    next: Option<PanelRow>,
    status: PanelStatus,
}

impl Panel {
    /// Sanitizes `product`, `headline`, and the closing status text through
    /// [`tui::clean_live_text`] at construction — the same boundary applied
    /// to every row and section title (P467 §3.1b), so nothing a caller
    /// hands to a panel can carry raw ANSI/control bytes into the terminal.
    pub(crate) fn new(
        product: impl Into<String>,
        headline: impl Into<String>,
        status: PanelStatus,
    ) -> Self {
        Self {
            product: tui::clean_live_text(&product.into()),
            headline: tui::clean_live_text(&headline.into()),
            items: Vec::new(),
            next: None,
            status: status.sanitized(),
        }
    }

    pub(crate) fn row(mut self, row: PanelRow) -> Self {
        self.items.push(PanelItem::Row(row));
        self
    }

    pub(crate) fn section(mut self, section: PanelSection) -> Self {
        self.items.push(PanelItem::Section(section));
        self
    }

    /// Sets the panel's explicit `next` action row. Structurally distinct
    /// from [`Panel::row`] so every panel's next-step guidance lands in the
    /// same place (after the last section, before the closing border)
    /// instead of being reconstructed per command.
    pub(crate) fn next(mut self, row: PanelRow) -> Self {
        self.next = Some(row);
        self
    }

    fn all_rows(&self) -> impl Iterator<Item = &PanelRow> {
        self.items
            .iter()
            .flat_map(|item| match item {
                PanelItem::Row(row) => std::slice::from_ref(row),
                PanelItem::Section(section) => section.rows.as_slice(),
            })
            .chain(self.next.iter())
    }

    fn frame_width(&self) -> usize {
        let terminal = tui::terminal_columns()
            .unwrap_or(MAX_PANEL_WIDTH)
            .saturating_sub(1);
        MAX_PANEL_WIDTH.min(terminal.max(1))
    }

    /// The label column's upper bound at a given frame width: whatever is
    /// left after the gutter, body indent, the label/value separator space,
    /// and at least [`MIN_VALUE_WIDTH`] columns for the value. Narrower than
    /// this and a row could not fit `label_column` + [`MIN_VALUE_WIDTH`]
    /// inside `frame_width` at all.
    fn max_label_column(frame_width: usize) -> usize {
        frame_width.saturating_sub(gutter_prefix_width() + BODY_INDENT.len() + 1 + MIN_VALUE_WIDTH)
    }

    /// One deterministic label column, computed from display width (not
    /// byte length) across every row this panel holds. Bounded by both
    /// [`MAX_LABEL_FRACTION`] of the frame and, ultimately, by
    /// [`Self::max_label_column`] so the value column can never be pushed
    /// below [`MIN_VALUE_WIDTH`] regardless of how narrow the frame is.
    fn label_column(&self, frame_width: usize) -> usize {
        let widest = self
            .all_rows()
            .map(|row| tui::display_width(&row.label_text()))
            .max()
            .unwrap_or(MIN_LABEL_COLUMN);
        let max_label_column = Self::max_label_column(frame_width);
        let fraction_bound = (frame_width / MAX_LABEL_FRACTION).max(MIN_LABEL_COLUMN);
        widest.min(fraction_bound).min(max_label_column)
    }

    /// The value column at a given frame width and label column — always at
    /// least [`MIN_VALUE_WIDTH`], since `label_column` is itself bounded by
    /// [`Self::max_label_column`] to leave room for it.
    fn value_width(&self, frame_width: usize, label_column: usize) -> usize {
        frame_width
            .saturating_sub(gutter_prefix_width() + BODY_INDENT.len() + label_column + 1)
            .max(MIN_VALUE_WIDTH)
    }

    /// The styled ctx.gate-style rendering at an explicit frame width: a
    /// `╭─ product ─ headline` top border with no separator directly after
    /// it, indented `│   {label} value` rows, each section introduced by
    /// exactly one bare `│` separator immediately before its `│ {title}`
    /// line (never after), the explicit `next` row with no separator before
    /// or after it, and a `╰─ state` closing line with no right wall and no
    /// separator before it. [`Self::styled_lines`] delegates here using the
    /// panel's own detected width; tests call this directly to exercise the
    /// same renderer at an explicit narrow width.
    fn render_styled(&self, frame_width: usize) -> Vec<Line> {
        let label_column = self.label_column(frame_width);
        let value_width = self.value_width(frame_width, label_column);

        let mut lines = top_border_lines(&self.product, &self.headline, frame_width);
        for item in &self.items {
            match item {
                PanelItem::Row(row) => {
                    lines.extend(row_lines(row, label_column, value_width));
                }
                PanelItem::Section(section) => {
                    lines.push(gutter_separator_line());
                    lines.extend(section_title_lines(&section.title, frame_width));
                    for row in &section.rows {
                        lines.extend(row_lines(row, label_column, value_width));
                    }
                }
            }
        }
        if let Some(next) = &self.next {
            lines.extend(row_lines(next, label_column, value_width));
        }
        lines.extend(bottom_border_lines(
            self.status.text(),
            self.status.tone(),
            frame_width,
        ));
        lines
    }

    /// The styled ctx.gate-style rendering at the panel's own detected
    /// frame width. See [`Self::render_styled`] for the locked grammar.
    pub(crate) fn styled_lines(&self) -> Vec<Line> {
        self.render_styled(self.frame_width())
    }

    /// The plain projection: the same semantic fields in the same order —
    /// title, rows, sections, next action, closing state — with no ANSI
    /// escapes and no frame glyphs.
    pub(crate) fn plain_lines(&self) -> Vec<String> {
        let mut lines = vec![format!("{} {}", self.product, self.headline)];
        for item in &self.items {
            match item {
                PanelItem::Row(row) => lines.push(plain_row_line(row)),
                PanelItem::Section(section) => {
                    lines.push(String::new());
                    lines.push(format!("{}:", section.title));
                    for row in &section.rows {
                        lines.push(plain_row_line(row));
                    }
                }
            }
        }
        if let Some(next) = &self.next {
            lines.push(String::new());
            lines.push(plain_row_line(next));
        }
        lines.push(String::new());
        lines.push(self.status.text().to_string());
        lines
    }

    fn emit_plain(&self) -> crate::Result<()> {
        for line in self.plain_lines() {
            tui::write_plain_line(line)?;
        }
        Ok(())
    }

    /// The one-row styled projection (P467 §3.1a): the same top-border and
    /// closing-state text `render_styled` would emit as two lines, collapsed
    /// onto one — `{product} {headline} · {state}` — with the closing
    /// state's tone preserved exactly as [`Self::render_styled`] applies it
    /// to the bottom border. Ignores `items`/`next`; callers of
    /// [`emit_one_row`] never add either.
    fn one_row_styled_line(&self) -> Line {
        let mut line = Line::blank();
        line.push(
            format!("{} {} · ", self.product, self.headline),
            Tone::Default,
        );
        line.push(self.status.text().to_string(), self.status.tone());
        line
    }

    /// The one-row plain projection: same field order as
    /// [`Self::one_row_styled_line`], no ANSI or frame glyphs.
    fn one_row_plain_line(&self) -> String {
        format!(
            "{} {} · {}",
            self.product,
            self.headline,
            self.status.text()
        )
    }
}

/// Emits a single line in house vocabulary — `{product} {headline} ·
/// {status}` — through the same styled/plain gate [`emit_human`] uses, for
/// commands whose default output is one fact, not a panel (P467 §3.1a).
/// There is no verbose/detail variant: a one-row command has no detail body
/// to hide behind `--verbose` by construction.
pub(crate) fn emit_one_row(
    disable_presentation: bool,
    product: impl Into<String>,
    headline: impl Into<String>,
    status: PanelStatus,
) -> crate::Result<()> {
    let panel = Panel::new(product, headline, status);
    if tui::stdout_supports_color(disable_presentation) {
        tui::emit_lines(&[panel.one_row_styled_line()])?;
    } else {
        tui::write_plain_line(panel.one_row_plain_line())?;
    }
    Ok(())
}

/// Width of the gutter (`"│ "`) that prefixes every top-level body/section
/// content line.
fn gutter_prefix_width() -> usize {
    tui::display_width(BAR) + 1
}

fn gutter_prefix() -> String {
    format!("{BAR} ")
}

/// A bare `│` separator line: no trailing space, no frame glyphs beyond the
/// gutter bar itself.
fn gutter_separator_line() -> Line {
    let mut line = Line::blank();
    line.push(BAR.to_string(), Tone::Muted);
    line
}

/// Renders a section's `│ {title}` line, wrapping by display width (hard-
/// splitting a single overlong word) onto further `│ `-prefixed
/// continuation lines rather than exceeding `frame_width`.
fn section_title_lines(title: &str, frame_width: usize) -> Vec<Line> {
    let prefix = gutter_prefix();
    let width = frame_width
        .saturating_sub(tui::display_width(&prefix))
        .max(1);
    wrap_display_width(title, width)
        .into_iter()
        .map(|chunk| {
            let mut line = Line::blank();
            line.push(prefix.clone(), Tone::Muted);
            line.push(chunk, Tone::Default);
            line
        })
        .collect()
}

/// Renders the `╭─ product ─ headline` top border. When the combined
/// product/headline text is wider than the frame, it wraps at word
/// boundaries (hard-splitting a single overlong word) onto gutter-prefixed
/// continuation lines rather than dropping content.
fn top_border_lines(product: &str, headline: &str, frame_width: usize) -> Vec<Line> {
    let prefix = "╭─ ";
    let glue = " ─ ";
    let content = format!("{product}{glue}{headline}");
    wrap_border(
        prefix,
        &gutter_prefix(),
        &content,
        frame_width,
        Tone::Default,
    )
}

/// Renders the `╰─ state` closing line with no right wall. Wraps overlong
/// state text the same way as the top border, using plain spaces (there is
/// no gutter below the closing line) for continuations.
fn bottom_border_lines(state: &str, tone: Tone, frame_width: usize) -> Vec<Line> {
    let prefix = "╰─ ";
    let continuation = " ".repeat(tui::display_width(prefix));
    wrap_border(prefix, &continuation, state, frame_width, tone)
}

fn wrap_border(
    prefix: &str,
    continuation_prefix: &str,
    content: &str,
    frame_width: usize,
    tone: Tone,
) -> Vec<Line> {
    let first_width = frame_width
        .saturating_sub(tui::display_width(prefix))
        .max(1);
    let continuation_width = frame_width
        .saturating_sub(tui::display_width(continuation_prefix))
        .max(1);
    let width = first_width.min(continuation_width);
    wrap_display_width(content, width)
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            let mut line = Line::blank();
            if index == 0 {
                line.push(prefix.to_string(), Tone::Muted);
            } else {
                line.push(continuation_prefix.to_string(), Tone::Muted);
            }
            line.push(chunk, tone);
            line
        })
        .collect()
}

/// Renders one row's `│   {label} value` line(s) behind the gutter. Every
/// row — top-level or nested inside a section — carries the same
/// [`BODY_INDENT`] beneath the gutter bar, so both read as `│   {label}
/// value`. Both the label and the value independently wrap by display width
/// when either is too wide for its column, so no rendered line ever exceeds
/// the selected frame width and no semantic content is dropped.
fn row_lines(row: &PanelRow, label_column: usize, value_width: usize) -> Vec<Line> {
    let label_chunks = wrap_display_width(&row.label_text(), label_column);
    let value_chunks = wrap_display_width(&row.value, value_width);
    let rows = label_chunks.len().max(value_chunks.len());
    (0..rows)
        .map(|index| {
            let mut line = Line::blank();
            line.push(gutter_prefix(), Tone::Muted);
            line.push(BODY_INDENT.to_string(), Tone::Muted);
            match label_chunks.get(index) {
                Some(chunk) => {
                    let pad = label_column.saturating_sub(tui::display_width(chunk)) + 1;
                    line.push(chunk.clone(), Tone::Muted);
                    line.push(" ".repeat(pad), Tone::Muted);
                }
                None => line.push(" ".repeat(label_column + 1), Tone::Muted),
            }
            if let Some(chunk) = value_chunks.get(index) {
                line.push(chunk.clone(), row.tone.tone());
            }
            line
        })
        .collect()
}

/// Renders one row's plain `label value` line with the shared
/// [`BODY_INDENT`] applied to every row — top-level and next rows included —
/// matching the styled renderer's invariant that all body rows share one
/// indentation regardless of position.
fn plain_row_line(row: &PanelRow) -> String {
    format!("{BODY_INDENT}{} {}", row.label_text(), row.value)
}

/// Greedy word-wrap by display width, matching [`tui`]'s own Unicode-width
/// approximation rather than byte length. A single word wider than `width`
/// is itself hard-split by display width rather than emitted whole, so no
/// rendered line can exceed the selected width. Always returns at least one
/// (possibly empty) chunk so a row with an empty value still renders a row.
fn wrap_display_width(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for word in text.split(' ') {
        let word_width = tui::display_width(word);
        if word_width > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }
            lines.extend(hard_wrap(word, width));
            continue;
        }
        if current.is_empty() {
            current.push_str(word);
            current_width = word_width;
            continue;
        }
        if current_width + 1 + word_width <= width {
            current.push(' ');
            current.push_str(word);
            current_width += 1 + word_width;
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
            current_width = word_width;
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

/// Hard-splits a single word that is itself wider than `width`, by display
/// width rather than byte length, so wide characters can't overflow a
/// chunk.
fn hard_wrap(word: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for ch in word.chars() {
        let ch_width = tui::display_width(&ch.to_string()).max(1);
        if current_width + ch_width > width && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push(ch);
        current_width += ch_width;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// The three outcomes a migrated command's output selector resolves to.
/// `Json` always wins over `--verbose`: it is a distinct variant here so a
/// caller can never hand `Json` to [`emit_human`], which only accepts
/// [`HumanOutputMode`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputMode {
    Json,
    Human(HumanOutputMode),
}

impl OutputMode {
    /// `json` takes precedence over `verbose` — matches the phase contract
    /// that JSON branches directly to the command's serializer and never
    /// enters the panel renderer.
    pub(crate) fn select(json: bool, verbose: bool) -> Self {
        if json {
            OutputMode::Json
        } else if verbose {
            OutputMode::Human(HumanOutputMode::Verbose)
        } else {
            OutputMode::Human(HumanOutputMode::Compact)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HumanOutputMode {
    Compact,
    Verbose,
}

/// Emit a panel through the shared styled/plain gate, then — only for
/// [`HumanOutputMode::Verbose`] — call `detail` once to emit the full-detail
/// body a migrated command already knows how to render (styled or plain)
/// unchanged. `detail` is never invoked for [`HumanOutputMode::Compact`].
pub(crate) fn emit_human<D>(
    disable_presentation: bool,
    panel: &Panel,
    mode: HumanOutputMode,
    detail: D,
) -> crate::Result<()>
where
    D: FnOnce() -> crate::Result<()>,
{
    if tui::stdout_supports_color(disable_presentation) {
        tui::emit_lines(&panel.styled_lines())?;
    } else {
        panel.emit_plain()?;
    }
    if mode == HumanOutputMode::Verbose {
        detail()?;
    }
    Ok(())
}

/// How a visible `ctx traits` command's default (non-`--json`) output is
/// rendered. [`presentation_for`] is the single source of truth this maps
/// from. Two independent things bind a registry claim to reality: the
/// registration-layer unit test in this file (every visible name resolves,
/// every entry still names a live command), and `tests/proof_output_style.rs`
/// — a crate-external integration test that, because `presentation` is a
/// `pub mod` and this type and [`presentation_for`] are `pub`, can call
/// `presentation_for` itself and drive the real `ctx` binary in the same
/// test, asserting the shape its own answer promises rather than a
/// hardcoded literal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandPresentation {
    /// Default output is a [`Panel`].
    Panel,
    /// Default output is a single [`emit_one_row`] line.
    OneRow,
    /// Default output does not route through the kit, for the stated
    /// reason. Every variant requires a non-empty reason (enforced by the
    /// drift gate, not by the type) so an exemption is always legible next
    /// to the table entry that grants it, not in a doc that can drift from
    /// the code. Reserved for a command that will never route through the
    /// kit (a permanent boundary, e.g. §1's explicit non-scope) — never a
    /// placeholder for work not yet done.
    Exempt(&'static str),
}

/// Every name [`presentation_for`] resolves — the same set its match arms
/// cover, kept as a flat list so the drift gate's reverse direction ("every
/// registry entry still names a live visible command") can check it against
/// the real Clap tree without needing to enumerate every possible string
/// against the fallible classifier. Duplicating the name list here (rather
/// than deriving it from the match) is deliberate: a name added to one but
/// not the other fails a test immediately instead of the two silently
/// drifting apart. `pub` (not `pub(crate)`) so `tests/proof_output_style.rs`
/// can enumerate the SAME list to prove its honesty-layer tests plus its
/// explicit network/provider carve-out set cover every registered name —
/// the mechanical binding `registry-claim-unbound-to-actual-output` asks
/// for, rather than a second name list that could drift from this one.
pub const REGISTERED_COMMAND_NAMES: &[&str] = &[
    "doctor",
    "init",
    "new",
    "list",
    "build",
    "check",
    "diff",
    "explain",
    "export",
    "host",
    "generate",
    "refine",
    "critique",
    "run",
    "merge",
    "activate",
    "trust",
    "import",
    "cache",
    "config",
    // P567: `vendor`/`install`/`remove`/`update`/`outdated`/`info`/`publish`
    // moved under this group and are now hidden aliases, so they are no longer
    // visible top-level commands and must not appear above. Their subcommands render
    // through the same panels they always did.
    "dependency",
];

/// Classifies one `ctx traits` visible top-level command name (Clap's
/// kebab-case rendering) by how its default human output is rendered.
/// Modeled on `help_surface::classify`'s exhaustiveness discipline: any name
/// not covered here is a generation error, not a silently-passing gap, so a
/// newly added or renamed visible command cannot land without an entry
/// here. Hidden commands (`#[command(hide = true)]`) are never looked up —
/// callers filter to visible names first, matching P467 §1's scoping rule
/// that hidden ≠ removed but also hidden ≠ product surface.
///
pub fn presentation_for(name: &str) -> Result<CommandPresentation, String> {
    use CommandPresentation::Panel;

    let presentation = match name {
        "doctor" | "init" | "new" | "list" | "build" | "host" | "generate" | "refine"
        | "critique" | "merge" | "activate" | "trust" | "import" | "cache" | "config" | "check"
        | "diff" | "explain" | "export" | "run" | "dependency" => Panel,
        other => {
            return Err(format!(
                "presentation_for: unclassified visible command {other:?}; add it to the \
                 registry in presentation.rs (Panel, OneRow, or Exempt with a reason)"
            ));
        }
    };
    Ok(presentation)
}

/// Render a `#[serde(rename_all = "kebab-case")]` enum the same way
/// `--json` would — the kebab-case wire token, not its Rust `Debug` form
/// (`awaiting-agent-output`, not `AwaitingAgentOutput`). Falls back to the
/// serialized JSON text for a non-string variant (a struct/tuple variant)
/// — one place, one decision, rather than a per-enum `Display` impl.
pub(crate) fn wire_name(value: &impl serde::Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(s)) => s,
        Ok(other) => other.to_string(),
        Err(_) => "unknown".to_string(),
    }
}

/// Shell-readable joined argv, quoting an element only when it contains
/// whitespace or a shell-meaningful character — replaces `argv={argv:?}`
/// Rust-list formatting in human output.
pub(crate) fn argv_display(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| {
            if arg.is_empty()
                || arg
                    .chars()
                    .any(|ch| ch.is_whitespace() || "\"'$`\\".contains(ch))
            {
                format!("{arg:?}")
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `value` or `unset` — replaces `Some(x)`/`None` `Debug` formatting
/// (`exit=Some(101)`, `absent (default: None)`) in human output.
pub(crate) fn optional<T: std::fmt::Display>(value: Option<T>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "unset".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn reference_panel(status: PanelStatus) -> Panel {
        Panel::new("ctx", "doctor", status)
            .row(PanelRow::new("trait", "guarded-change"))
            .row(PanelRow::toned("status", "ok", RowTone::Pass))
            .section(PanelSection::new(
                "Summary",
                vec![
                    PanelRow::new("resolved", "harness.toml"),
                    PanelRow::toned("tier", "project", RowTone::Default),
                ],
            ))
            .next(PanelRow::new("next", "run `ctx traits check`"))
    }

    fn plain_texts(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|line| line.segments().map(|(text, _)| text).collect::<String>())
            .collect()
    }

    fn tones(lines: &[Line]) -> Vec<Tone> {
        lines
            .iter()
            .flat_map(|line| line.segments().map(|(_, tone)| tone))
            .collect()
    }

    #[test]
    fn styled_panel_matches_locked_ctx_gate_grammar() {
        let panel = reference_panel(PanelStatus::Passed("passed".to_string()));
        let lines = panel.styled_lines();
        let plain = plain_texts(&lines);

        // Exact expected line sequence for `reference_panel`: top border, no
        // separator directly after it, two top-level `│   ` rows, exactly
        // one bare `│` immediately before the section title, the section's
        // `│   ` rows, the `next` row with no separator before or after it,
        // and the closing line with no separator before it.
        let expected = vec![
            "╭─ ctx ─ doctor".to_string(),
            "│   trait:    guarded-change".to_string(),
            "│   status:   ok".to_string(),
            "│".to_string(),
            "│ Summary".to_string(),
            "│   resolved: harness.toml".to_string(),
            "│   tier:     project".to_string(),
            "│   next:     run `ctx traits check`".to_string(),
            "╰─ passed".to_string(),
        ];
        assert_eq!(
            plain, expected,
            "line sequence must match the locked grammar exactly"
        );

        // No square glyphs anywhere, and the closing line has no right wall.
        for glyph in ["┌", "┐", "└", "┘"] {
            assert!(
                !plain.join("\n").contains(glyph),
                "must not contain {glyph:?}"
            );
        }
        assert!(!plain.last().unwrap().ends_with('│'));

        // No bare gutter directly after the top border.
        assert_ne!(
            plain[1], "│",
            "no bare gutter directly after the top border"
        );
        // No extra bare gutter appears before the closing line.
        assert_ne!(
            plain[plain.len() - 2],
            "│",
            "no bare gutter directly before the closing line"
        );
        // Exactly one bare `│` line in the whole panel, immediately
        // preceding the section title.
        let bare_indices: Vec<usize> = plain
            .iter()
            .enumerate()
            .filter(|(_, l)| l.as_str() == "│")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            bare_indices.len(),
            1,
            "exactly one bare gutter line: {plain:?}"
        );
        assert_eq!(
            plain[bare_indices[0] + 1],
            "│ Summary",
            "the bare gutter must immediately precede the section title"
        );
    }

    #[test]
    fn non_label_content_survives_mixed_case_unchanged() {
        let panel = Panel::new("CTX", "Doctor", PanelStatus::Passed("Passed".to_string()))
            .row(PanelRow::new("Trait", "GuardedChange"))
            .section(PanelSection::new(
                "Approval commands",
                vec![PanelRow::new("Tier", "Project")],
            ));

        let styled = plain_texts(&panel.styled_lines()).join("\n");
        assert!(styled.contains("╭─ CTX ─ Doctor"));
        assert!(styled.contains("Approval commands"));
        assert!(styled.contains("GuardedChange"));
        assert!(styled.contains("Project"));
        assert!(styled.ends_with("Passed"));
        // Only the row label is lowercased.
        assert!(styled.contains("trait:"));
        assert!(!styled.contains("Trait:"));

        let plain = panel.plain_lines().join("\n");
        assert!(plain.starts_with("CTX Doctor"));
        assert!(plain.contains("Approval commands"));
        assert!(plain.contains("GuardedChange"));
        assert!(plain.contains("Project"));
        assert!(plain.ends_with("Passed"));
        assert!(plain.contains("trait:"));
        assert!(!plain.contains("Trait:"));
    }

    #[test]
    fn row_labels_are_lowercased_structurally() {
        let panel = Panel::new("ctx", "doctor", PanelStatus::Passed("passed".to_string()))
            .row(PanelRow::new("Trait", "value"));
        let plain = panel.plain_lines().join("\n");
        assert!(plain.contains("trait:"));
        assert!(!plain.contains("Trait:"));
    }

    #[test]
    fn alignment_uses_display_width_for_unequal_and_wide_labels() {
        let panel = Panel::new("ctx", "doctor", PanelStatus::Passed("passed".to_string()))
            .row(PanelRow::new("a", "short"))
            .row(PanelRow::new("much-longer-label", "value"))
            .row(PanelRow::new("宽度", "wide-char label"));
        let lines = panel.styled_lines();

        // A row line is `[gutter, body indent, label, padding, value]`: sum
        // the display width of every segment but the last (the value) to
        // get the column the value actually starts in.
        let value_starts: Vec<usize> = lines
            .iter()
            .filter(|line| {
                let segments: Vec<_> = line.segments().collect();
                segments.len() == 5 && segments[0].0 == gutter_prefix()
            })
            .map(|line| {
                let segments: Vec<_> = line.segments().collect();
                segments[..4]
                    .iter()
                    .map(|(text, _)| tui::display_width(text))
                    .sum()
            })
            .collect();
        assert!(value_starts.len() >= 3);
        assert!(
            value_starts.windows(2).all(|w| w[0] == w[1]),
            "value column should align across unequal-width labels: {value_starts:?}"
        );
    }

    #[test]
    fn narrow_width_bounds_every_line_and_preserves_content() {
        let long_label = "an-extremely-long-unbroken-label-that-cannot-fit-at-all";
        let long_value = "/an/extremely/long/unbroken/path/that/cannot/be/split/nicely/at/all";
        let long_title = "an overlong section title that will not fit on this narrow panel width";
        let width = 16usize;
        assert!(
            width < 20,
            "must exercise a width below the removed artificial floor"
        );

        let panel = Panel::new(
            "ctx",
            "an extremely long headline that will not fit on a narrow panel at all",
            PanelStatus::Passed("passed".to_string()),
        )
        .row(PanelRow::new(long_label, "x"))
        .row(PanelRow::new("v", long_value))
        .section(PanelSection::new(long_title, vec![PanelRow::new("k", "v")]));

        // No rendered line exceeds the explicit width, through the panel's
        // own renderer — the same one `styled_lines` delegates to.
        let lines = panel.render_styled(width);
        for line in &lines {
            let line_width: usize = line.segments().map(|(t, _)| tui::display_width(t)).sum();
            assert!(
                line_width <= width,
                "line exceeds explicit width {width}: {line_width}"
            );
        }

        // Reconstruct the overlong label and value from the same width-bound
        // row renderer `render_styled` itself calls, and check real
        // equality rather than a tautological containment check.
        let label_column = panel.label_column(width);
        let value_width = panel.value_width(width, label_column);

        let label_row_lines = row_lines(&PanelRow::new(long_label, "x"), label_column, value_width);
        assert!(label_row_lines.len() > 1, "overlong label should wrap");
        let reconstructed_label: String = label_row_lines
            .iter()
            .flat_map(|line| line.segments())
            .filter(|&(text, tone)| {
                tone == Tone::Muted
                    && text != gutter_prefix()
                    && text != BODY_INDENT
                    && !text.trim().is_empty()
            })
            .map(|(text, _)| text)
            .collect();
        assert_eq!(reconstructed_label, format!("{long_label}:"));

        let value_row_lines = row_lines(&PanelRow::new("v", long_value), label_column, value_width);
        assert!(value_row_lines.len() > 1, "overlong value should wrap");
        let reconstructed_value: String = value_row_lines
            .iter()
            .flat_map(|line| line.segments())
            .filter(|&(_, tone)| tone == Tone::Default)
            .map(|(text, _)| text)
            .collect();
        assert_eq!(reconstructed_value, long_value);

        // Section titles wrap too, and reconstruct by joining wrapped words
        // back with single spaces (word-wrap, not hard-wrap, since the
        // title has no single overlong word).
        let title_lines = section_title_lines(long_title, width);
        assert!(title_lines.len() > 1, "overlong section title should wrap");
        let reconstructed_title = title_lines
            .iter()
            .map(|line| {
                line.segments()
                    .find(|&(_, tone)| tone == Tone::Default)
                    .map(|(text, _)| text)
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(reconstructed_title, long_title);
    }

    #[test]
    fn plain_projection_matches_styled_order_with_no_ansi_or_frame_glyphs() {
        let panel = reference_panel(PanelStatus::Blocked("blocked".to_string()));
        let plain = panel.plain_lines();
        let joined = plain.join("\n");

        for glyph in ["╭", "╰", "│", "─", "\x1b["] {
            assert!(
                !joined.contains(glyph),
                "plain output must not contain {glyph:?}"
            );
        }

        let trait_index = joined.find("trait:").unwrap();
        let status_index = joined.find("status:").unwrap();
        let section_index = joined.find("Summary:").unwrap();
        let resolved_index = joined.find("resolved:").unwrap();
        let next_index = joined.find("next:").unwrap();
        let closing_index = joined.rfind("blocked").unwrap();
        assert!(trait_index < status_index);
        assert!(status_index < section_index);
        assert!(section_index < resolved_index);
        assert!(resolved_index < next_index);
        assert!(next_index < closing_index);

        // Every label/value row — top-level, section-nested, and the
        // explicit next row alike — carries the same body indent; only the
        // section title and blank separators are exempt from it.
        assert_eq!(plain[0], "ctx doctor");
        assert_eq!(plain[1], format!("{BODY_INDENT}trait: guarded-change"));
        assert_eq!(plain[2], format!("{BODY_INDENT}status: ok"));
        assert_eq!(plain[3], "");
        assert_eq!(plain[4], "Summary:");
        assert_eq!(plain[5], format!("{BODY_INDENT}resolved: harness.toml"));
        assert_eq!(plain[6], format!("{BODY_INDENT}tier: project"));
        assert_eq!(plain[7], "");
        assert_eq!(
            plain[8],
            format!("{BODY_INDENT}next: run `ctx traits check`")
        );
        assert_eq!(plain[9], "");
        assert_eq!(plain[10], "blocked");
    }

    #[test]
    fn panel_segments_use_only_muted_default_pass_fail_tones() {
        let panel = reference_panel(PanelStatus::Critical("critical".to_string()));
        let used = tones(&panel.styled_lines());
        assert!(
            used.iter()
                .all(|tone| matches!(tone, Tone::Muted | Tone::Default | Tone::Pass | Tone::Fail))
        );
        assert!(
            used.contains(&Tone::Pass),
            "status: ok row should read as pass"
        );
        assert!(
            used.contains(&Tone::Fail),
            "critical closing state should read as fail"
        );

        let bottom = panel.styled_lines().pop().unwrap();
        let bottom_has_fail = bottom.segments().any(|(_, tone)| tone == Tone::Fail);
        assert!(
            bottom_has_fail,
            "critical closing border should read as fail"
        );
    }

    #[test]
    fn passed_status_reads_as_pass_tone() {
        let panel = reference_panel(PanelStatus::Passed("passed".to_string()));
        let bottom = panel.styled_lines().pop().unwrap();
        assert!(bottom.segments().any(|(_, tone)| tone == Tone::Pass));
    }

    #[test]
    fn compact_mode_never_evaluates_detail_closure() {
        let panel = reference_panel(PanelStatus::Passed("passed".to_string()));
        let called = Cell::new(false);
        emit_human(true, &panel, HumanOutputMode::Compact, || {
            called.set(true);
            Ok(())
        })
        .unwrap();
        assert!(!called.get());
    }

    #[test]
    fn verbose_mode_evaluates_detail_closure_exactly_once_after_panel() {
        let panel = reference_panel(PanelStatus::Passed("passed".to_string()));
        let calls = Cell::new(0u32);
        emit_human(true, &panel, HumanOutputMode::Verbose, || {
            calls.set(calls.get() + 1);
            Ok(())
        })
        .unwrap();
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn one_row_line_carries_house_vocabulary_and_status_tone() {
        let panel = Panel::new(
            "ctx traits activate",
            "trait-a",
            PanelStatus::Passed("activated".to_string()),
        );
        let styled = panel.one_row_styled_line();
        let plain = plain_texts(std::slice::from_ref(&styled)).join("");
        assert_eq!(plain, "ctx traits activate trait-a · activated");
        assert!(tones(std::slice::from_ref(&styled)).contains(&Tone::Pass));

        assert_eq!(
            panel.one_row_plain_line(),
            "ctx traits activate trait-a · activated"
        );
    }

    #[test]
    fn emit_one_row_writes_through_the_same_styled_plain_gate() {
        emit_one_row(
            true,
            "ctx traits activate",
            "trait-a",
            PanelStatus::Passed("activated".to_string()),
        )
        .unwrap();
    }

    #[test]
    fn one_row_line_sanitizes_untrusted_input() {
        let panel = Panel::new(
            "ctx",
            "activate \u{1b}[31mtrait\u{1b}[0m",
            PanelStatus::Blocked("blocked\u{1b}[0m".to_string()),
        );
        let plain = panel.one_row_plain_line();
        assert!(!plain.contains('\u{1b}'));
    }

    #[test]
    fn presentation_registry_covers_migrated_and_pending_commands() {
        assert_eq!(presentation_for("doctor"), Ok(CommandPresentation::Panel));
        assert_eq!(presentation_for("init"), Ok(CommandPresentation::Panel));
        assert_eq!(presentation_for("check"), Ok(CommandPresentation::Panel));
        assert_eq!(presentation_for("diff"), Ok(CommandPresentation::Panel));
        assert_eq!(presentation_for("explain"), Ok(CommandPresentation::Panel));
        assert_eq!(presentation_for("export"), Ok(CommandPresentation::Panel));
        assert_eq!(presentation_for("run"), Ok(CommandPresentation::Panel));
    }

    #[test]
    fn presentation_registry_rejects_unlisted_names() {
        assert!(presentation_for("not-a-real-command").is_err());
    }

    /// The drift gate's registration layer (P467 §3.2): walks the real
    /// derived Clap tree (the same source of truth `help_surface::classify`
    /// uses) rather than a second hand-maintained name list, so a newly
    /// added visible command with no registry entry — and a stale registry
    /// entry for a command that has been hidden or renamed — both fail here
    /// rather than silently passing. Lives as a crate-internal unit test
    /// (not `tests/proof_output_style.rs`) because `presentation_for` and
    /// `CommandPresentation` are `pub(crate)`, unreachable from an external
    /// integration-test crate; the behavioral honesty-layer assertions that
    /// only need the crate's public surface (spawning the real binary) live
    /// in `tests/proof_output_style.rs` instead.
    #[test]
    fn every_visible_traits_command_has_a_registry_entry() {
        // Building the full derived Clap tree (~40 hidden variants plus the
        // visible surface) overflows `cargo test`'s default per-test thread
        // stack in a debug build; the real `ctx` binary never hits this
        // (its main thread gets the OS default, not the test harness's),
        // and `cli::command()` is otherwise only ever exercised through it
        // (`help_surface.rs`, `proof_cli_surface.rs`). Run this check on a
        // dedicated thread with a generous stack instead of depending on
        // `RUST_MIN_STACK` being set in every gate that runs this suite.
        std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(|| {
                let cli = crate::app::surface::cli::command();
                let traits_command = cli
                    .find_subcommand("traits")
                    .expect("`traits` subcommand always exists in the derived Cli tree");
                let visible_names: Vec<String> = traits_command
                    .get_subcommands()
                    .filter(|sub| !sub.is_hide_set())
                    .map(|sub| sub.get_name().to_string())
                    .collect();
                assert!(
                    !visible_names.is_empty(),
                    "sanity: the traits command tree must expose at least one visible subcommand"
                );
                for name in &visible_names {
                    assert!(
                        presentation_for(name).is_ok(),
                        "visible command {name:?} has no presentation registry entry"
                    );
                }
                for registered in REGISTERED_COMMAND_NAMES {
                    assert!(
                        visible_names.iter().any(|name| name == registered),
                        "registry entry {registered:?} does not name a live visible command \
                         (hidden, renamed, or removed?) — remove or update its row in \
                         presentation_for"
                    );
                }
            })
            .expect("spawn the registry-check thread")
            .join()
            .expect("registry-check thread must not panic");
    }

    #[test]
    fn json_wins_over_verbose_and_cannot_reach_human_renderer() {
        assert_eq!(OutputMode::select(true, true), OutputMode::Json);
        assert_eq!(
            OutputMode::select(false, true),
            OutputMode::Human(HumanOutputMode::Verbose)
        );
        assert_eq!(
            OutputMode::select(false, false),
            OutputMode::Human(HumanOutputMode::Compact)
        );
        // `emit_human` only accepts `HumanOutputMode` — `OutputMode::Json`
        // cannot be passed to it, enforced at compile time by the type
        // rather than by a runtime check.
    }
}
