//! Terminal presentation primitives for interactive CLI output.
//!
//! Product semantics stay in core/io. This module only owns the small ANSI
//! palette below and the capability check that gates it. It never sets a
//! background and only uses named ANSI colours (green/red) plus faint, all of
//! which the terminal maps to the user's own scheme — so nothing here assumes a
//! light or dark background.

use std::io::{IsTerminal, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const RESET: &str = "\x1b[0m";
const FAINT: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const BOLD: &str = "\x1b[1m";
const DEFAULT_TERMINAL_ROWS: usize = 24;

/// How a text segment is shaded. `Default` is the terminal's own foreground and
/// `Muted` a faint that inherits it (both theme-agnostic); `Pass`/`Fail`/`Warn`
/// use the named ANSI green/red/yellow, which read correctly on light and dark
/// alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Default,
    Muted,
    Pass,
    Fail,
    Warn,
    Bold,
}

/// Format a relative duration in compact human-readable units.
pub(crate) fn human_elapsed_text(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

/// Format an elapsed duration as an unbounded, zero-padded clock.
pub(crate) fn elapsed_text(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

pub(crate) fn token_text(tokens: u64) -> String {
    if tokens < 1_000 {
        return format!("{tokens} tok");
    }
    if tokens >= 1_000_000 {
        return format!("{:.1}m tok", tokens as f64 / 1_000_000.0);
    }
    format!("{:.1}k tok", tokens as f64 / 1_000.0)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Segment {
    text: String,
    tone: Tone,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Line {
    segments: Vec<Segment>,
}

impl Line {
    pub(crate) fn blank() -> Self {
        Self::default()
    }

    pub(crate) fn push(&mut self, text: impl Into<String>, tone: Tone) {
        self.segments.push(Segment {
            text: text.into(),
            tone,
        });
    }

    /// Exposes each segment's text and tone without leaking the private
    /// `Segment` type — the ratatui backend paints from this instead of the
    /// ANSI escape sequences [`paint`] builds.
    pub(crate) fn segments(&self) -> impl Iterator<Item = (&str, Tone)> {
        self.segments
            .iter()
            .map(|segment| (segment.text.as_str(), segment.tone))
    }
}

/// What the live line should show right now: a compacted narration (normal
/// weight) or a fallback — raw thinking or `(initialization)` — shown dimmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LiveDisplay {
    pub(crate) text: String,
    pub(crate) muted: bool,
    pub(crate) finished: bool,
}

impl LiveDisplay {
    fn dim(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            muted: true,
            finished: false,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct LiveLine {
    buffer: String,
    passthrough: Option<String>,
    narration: Option<String>,
    pending_since: Option<Instant>,
    finished: bool,
    /// The narrator's own running "settled + current" estimate of
    /// in-progress thinking tokens (P496), shown as a pill only while no
    /// narration has landed yet — a landed narration is always better
    /// information than a token count, so this never overwrites one.
    thinking_tokens: Option<u64>,
}

impl LiveLine {
    const MAX_BUFFER_BYTES: usize = 16 * 1024;

    /// Returns every text delta drained from `chunk` this call (P470: the
    /// live run pane's CURRENT-step stream needs the FULL list, not just the
    /// last one — see `crate::app::harness_stream::drain_text_deltas`).
    /// `--progress stream`'s [`LiveOutputPanel::push_bytes`] ignores the
    /// return; this method's own passthrough-line behavior (showing only the
    /// latest delta) is unchanged.
    pub(crate) fn push_bytes(&mut self, chunk: &[u8]) -> Vec<String> {
        if chunk.is_empty() {
            return Vec::new();
        }
        self.finished = false;
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let deltas = crate::app::harness_stream::drain_text_deltas(&mut self.buffer);
        if let Some(text) = deltas.last() {
            self.passthrough = Some(quote_line(text));
            // New un-narrated activity: start the soft-timeout window. We keep
            // showing the last narration until it elapses (or the narrator
            // returns), so raw thinking never flashes immediately.
            if self.pending_since.is_none() {
                self.pending_since = Some(Instant::now());
            }
        }
        trim_buffer(&mut self.buffer, Self::MAX_BUFFER_BYTES);
        deltas
    }

    pub(crate) fn push_summary(&mut self, summary: impl Into<String>) {
        self.finished = false;
        self.narration = Some(clean_live_text(&summary.into()));
        // The narrator caught up with the current activity; stop the soft-timeout
        // raw fallback until new thinking arrives.
        self.pending_since = None;
    }

    pub(crate) fn finish(&mut self, summary: &str) {
        self.narration = Some(clean_live_text(summary));
        self.pending_since = None;
        self.finished = true;
    }

    /// P496: record the narrator's latest observed thinking-token total.
    /// Display-only — never touches `narration`, so a landed narration is
    /// never displaced by a later pill update.
    pub(crate) fn set_thinking_tokens(&mut self, tokens: u64) {
        self.thinking_tokens = Some(tokens);
    }

    /// The line to show right now. A landed narration wins; while a narration is
    /// pending we keep the last narration (or the initialization fallback) until
    /// the soft timeout, after which the raw thinking is revealed dimmed until
    /// the narrator returns.
    pub(crate) fn current_display(&self) -> LiveDisplay {
        if let Some(since) = self.pending_since
            && since.elapsed() >= NARRATION_SOFT_TIMEOUT
            && let Some(raw) = &self.passthrough
        {
            return LiveDisplay::dim(raw.clone());
        }
        match &self.narration {
            Some(narration) => LiveDisplay {
                text: narration.clone(),
                muted: false,
                finished: self.finished,
            },
            None => match self.thinking_tokens {
                Some(tokens) => LiveDisplay::dim(format!("thinking… ~{}", token_text(tokens))),
                None => LiveDisplay::dim("(initialization)"),
            },
        }
    }
}

/// Whether styled output should be written to stdout. False for `NO_COLOR`,
/// `CI`, `TERM=dumb`, an explicit opt-out, or a non-terminal stdout (pipes) —
/// in which case the caller emits the plain report instead.
pub(crate) fn stdout_supports_color(disable_presentation: bool) -> bool {
    if disable_presentation
        || std::env::var_os("NO_COLOR").is_some()
        || std::env::var_os("CI").is_some()
    {
        return false;
    }
    if std::env::var("TERM").is_ok_and(|term| term.eq_ignore_ascii_case("dumb")) {
        return false;
    }
    std::io::stdout().is_terminal()
}

pub(crate) fn stderr_supports_live(disable_presentation: bool) -> bool {
    if disable_presentation
        || std::env::var_os("NO_COLOR").is_some()
        || std::env::var_os("CI").is_some()
    {
        return false;
    }
    if std::env::var("TERM").is_ok_and(|term| term.eq_ignore_ascii_case("dumb")) {
        return false;
    }
    std::io::stderr().is_terminal()
}

#[derive(Clone)]
pub(crate) struct LiveOutputPanel {
    state: Arc<Mutex<LiveOutputState>>,
}

/// While a narration is pending, keep showing the last narration (or
/// `(initialization)`); only after this soft timeout is the raw thinking revealed
/// (dimmed), and it is replaced the moment the narrator returns.
const NARRATION_SOFT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug)]
struct LiveOutputState {
    label: String,
    live: LiveLine,
    rendered_lines: usize,
}

impl LiveOutputPanel {
    const DEFAULT_COLUMNS: usize = 120;

    pub(crate) fn new(label: impl Into<String>) -> Self {
        Self {
            state: Arc::new(Mutex::new(LiveOutputState {
                label: label.into(),
                live: LiveLine::default(),
                rendered_lines: 0,
            })),
        }
    }

    pub(crate) fn finish(&self, summary: &str) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let before = state.live.current_display();
        state.live.finish(summary);
        if before == state.live.current_display() {
            return;
        }
        let lines = summary_lines(&state);
        let _ = render_panel_lines(&mut state, &lines);
    }

    /// Overlay a compacted narrator summary. It takes precedence over the live
    /// passthrough text for [`NARRATION_SOFT_TIMEOUT`]; if narration stalls or fails, the
    /// panel falls back to the passthrough so it is never blank or stuck stale.
    pub(crate) fn push_summary(&self, summary: impl Into<String>) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let before = state.live.current_display();
        state.live.push_summary(summary);
        if before == state.live.current_display() {
            return;
        }
        let lines = summary_lines(&state);
        let _ = render_panel_lines(&mut state, &lines);
    }

    /// P496: update the between-narrations token pill. Presentation only —
    /// a no-op once a narration has landed (see [`LiveLine::current_display`]).
    pub(crate) fn push_thinking_tokens(&self, tokens: u64) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let before = state.live.current_display();
        state.live.set_thinking_tokens(tokens);
        if before == state.live.current_display() {
            return;
        }
        let lines = summary_lines(&state);
        let _ = render_panel_lines(&mut state, &lines);
    }

    /// The reliable floor: frame the harness NDJSON and show the model's own
    /// latest thinking/response text — cleaned, quoted, preview-truncated — so
    /// the panel always shows current activity even while a narrator is running.
    pub(crate) fn push_bytes(&self, chunk: &[u8]) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let before = state.live.current_display();
        let _ = state.live.push_bytes(chunk);
        if before == state.live.current_display() {
            return;
        }
        let lines = summary_lines(&state);
        let _ = render_panel_lines(&mut state, &lines);
    }
}

/// Shared styled/plain report boundary: build and emit the styled `Line`s for
/// a color-capable stdout, otherwise run the caller's independent plain
/// fallback. `styled_lines` is only invoked when styling will actually be
/// used, so callers can defer building it. The plain fallback is fallible so
/// callers can route it through [`write_plain_line`] instead of a bare
/// `println!`.
pub(crate) fn emit_report<S, P>(
    disable_presentation: bool,
    styled_lines: S,
    plain: P,
) -> crate::Result<()>
where
    S: FnOnce() -> Vec<Line>,
    P: FnOnce() -> crate::Result<()>,
{
    if stdout_supports_color(disable_presentation) {
        emit_lines(&styled_lines())
    } else {
        plain()
    }
}

/// Write one line of plain (non-styled) fallback report text to stdout,
/// surfacing write failures (e.g. a broken pipe) as a `crate::Result` instead
/// of the panic `println!` would raise.
pub(crate) fn write_plain_line(text: impl std::fmt::Display) -> crate::Result<()> {
    writeln!(std::io::stdout(), "{text}").map_err(write_error)
}

/// Standard styled command line: a muted `$ ` prompt followed by the command
/// in default tone — the check grammar every report surface reuses.
pub(crate) fn command_line(command: impl Into<String>) -> Line {
    let mut line = Line::blank();
    line.push("$ ", Tone::Muted);
    line.push(command, Tone::Default);
    line
}

/// A muted label followed by a default-toned value, on one line.
pub(crate) fn labeled_line(label: &str, value: &str) -> Line {
    let mut line = Line::blank();
    line.push(label.to_string(), Tone::Muted);
    line.push(value.to_string(), Tone::Default);
    line
}

/// Print the styled lines to stdout, painting each segment by its tone.
pub(crate) fn emit_lines(lines: &[Line]) -> crate::Result<()> {
    let mut stdout = std::io::stdout().lock();
    for line in lines {
        let mut rendered = String::new();
        for segment in &line.segments {
            rendered.push_str(&paint(segment.tone, &segment.text));
        }
        writeln!(stdout, "{rendered}").map_err(write_error)?;
    }
    stdout.flush().map_err(write_error)?;
    Ok(())
}

/// Named-ANSI two-tone a plain stderr progress line (never a `Panel`), gated
/// by the same terminal/`NO_COLOR`/`CI`/`TERM=dumb` capability check the
/// styled panel path uses — a caller writing raw stderr lines (e.g.
/// `handle_generate`'s per-round observer) stays in the palette doctrine
/// without duplicating the gate.
pub(crate) fn stderr_line(text: &str, tone: Tone) -> String {
    if stderr_supports_live(false) {
        paint(tone, text)
    } else {
        text.to_string()
    }
}

fn paint(tone: Tone, text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    match tone {
        Tone::Default => text.to_string(),
        Tone::Muted => format!("{FAINT}{text}{RESET}"),
        Tone::Pass => format!("{GREEN}{text}{RESET}"),
        Tone::Fail => format!("{RED}{text}{RESET}"),
        Tone::Warn => format!("{YELLOW}{text}{RESET}"),
        Tone::Bold => format!("{BOLD}{text}{RESET}"),
    }
}

const PASSTHROUGH_PREVIEW_CHARS: usize = 100;

/// Wrap a raw model text delta as a one-line quoted preview for the passthrough
/// panel: control characters collapsed to spaces, head-truncated with an ellipsis.
fn quote_line(text: &str) -> String {
    let cleaned = clean_live_text(text);
    let trimmed = cleaned.trim();
    let preview: String = trimmed.chars().take(PASSTHROUGH_PREVIEW_CHARS).collect();
    if preview.chars().count() < trimmed.chars().count() {
        format!("\"{preview}…\"")
    } else {
        format!("\"{preview}\"")
    }
}

fn summary_lines(state: &LiveOutputState) -> Vec<Line> {
    let max_width = terminal_columns()
        .unwrap_or(LiveOutputPanel::DEFAULT_COLUMNS)
        .saturating_sub(1)
        .max(1);
    let label = format!("{}:", state.label);
    let display = state.live.current_display();
    let summary = clean_live_text(&display.text);
    let mut line = Line::blank();
    let label_width = display_width(&label) + 1;
    if label_width >= max_width {
        line.push(truncate_display_width(&label, max_width), Tone::Muted);
        return vec![line];
    }
    line.push(label, Tone::Muted);
    line.push(" ", Tone::Muted);
    line.push(
        truncate_display_width(&summary, max_width - label_width),
        if display.finished {
            Tone::Pass
        } else if display.muted {
            Tone::Muted
        } else {
            Tone::Default
        },
    );
    vec![line]
}

fn render_panel_lines(state: &mut LiveOutputState, lines: &[Line]) -> std::io::Result<()> {
    render_repaint_lines(&mut state.rendered_lines, lines)
}

fn render_repaint_lines(rendered_lines: &mut usize, lines: &[Line]) -> std::io::Result<()> {
    let mut stderr = std::io::stderr().lock();
    let max_lines = repaint_line_limit();
    let rewind_lines = (*rendered_lines).min(max_lines);
    if rewind_lines > 0 {
        write!(stderr, "\x1b[{}F", rewind_lines)?;
    }
    let max_width = terminal_columns()
        .unwrap_or(LiveOutputPanel::DEFAULT_COLUMNS)
        .saturating_sub(1)
        .max(1);
    let visible_start = lines.len().saturating_sub(max_lines);
    for line in lines.iter().skip(visible_start) {
        let line = clip_line(line, max_width);
        write!(stderr, "\r\x1b[2K")?;
        write!(stderr, "{}", render_line(&line))?;
        writeln!(stderr)?;
    }
    write!(stderr, "\x1b[0J")?;
    *rendered_lines = lines.len().min(max_lines);
    stderr.flush()
}

fn clip_line(line: &Line, max_width: usize) -> Line {
    let mut clipped = Line::blank();
    let mut used = 0;
    for segment in &line.segments {
        if used >= max_width {
            break;
        }
        let width = display_width(&segment.text);
        if used + width <= max_width {
            clipped.push(segment.text.clone(), segment.tone);
            used += width;
            continue;
        }
        let remaining = max_width - used;
        clipped.push(
            truncate_display_width_end(&segment.text, remaining),
            segment.tone,
        );
        break;
    }
    clipped
}

/// Clip a terminal string to `max_width` columns, retaining its leading text.
/// Unlike byte/character slicing, this accounts for wide and combining Unicode.
pub(crate) fn truncate_display_width_end(line: &str, max_width: usize) -> String {
    let width = display_width(line);
    if width <= max_width {
        return line.to_string();
    }
    let marker = "...";
    let marker_width = display_width(marker);
    if max_width <= marker_width {
        return marker.chars().take(max_width).collect();
    }
    let target = max_width - marker_width;
    let mut head = String::new();
    let mut used = 0;
    for ch in line.chars() {
        let width = char_display_width(ch);
        if used + width > target {
            break;
        }
        head.push(ch);
        used += width;
    }
    format!("{head}{marker}")
}

/// Same clipping as [`truncate_display_width_end`], but when it actually
/// truncates, also records the `(rendered, full)` pair into task 0023's
/// per-frame ledger (`super::tui_select`) — so a mouse selection that
/// reaches the ellipsis can later be expanded back to the untruncated
/// source rather than copying the `...`-truncated cells that were on
/// screen. A no-op on the ledger when nothing was actually truncated.
pub(crate) fn truncate_display_width_end_recording(line: &str, max_width: usize) -> String {
    let truncated = truncate_display_width_end(line, max_width);
    super::tui_select::record_truncation(&truncated, line);
    truncated
}

fn render_line(line: &Line) -> String {
    let mut rendered = String::new();
    for segment in &line.segments {
        rendered.push_str(&paint(segment.tone, &segment.text));
    }
    rendered
}

fn trim_buffer(buffer: &mut String, max_bytes: usize) {
    if buffer.len() <= max_bytes {
        return;
    }
    let target = buffer.len().saturating_sub(max_bytes);
    let start = buffer
        .char_indices()
        .find_map(|(index, _)| (index >= target).then_some(index))
        .unwrap_or(target);
    buffer.drain(..start);
}

pub(crate) fn clean_live_text(line: &str) -> String {
    strip_ansi_sequences(line)
        .chars()
        .map(|ch| {
            if ch.is_control() || is_bidi_format_control(ch) {
                ' '
            } else {
                ch
            }
        })
        .collect()
}

/// True for Unicode `Cf`-category bidirectional-formatting controls (the
/// explicit embedding/override/isolate marks and the directional marks).
/// These are not C0/C1 control codes and are not touched by
/// `char::is_control`, but a terminal still honors them and can use them to
/// visually reorder or mask surrounding text (e.g. `U+202E RIGHT-TO-LEFT
/// OVERRIDE`), so any text reaching a terminal via [`clean_live_text`] must
/// have them stripped alongside ANSI/control sequences.
fn is_bidi_format_control(ch: char) -> bool {
    matches!(
        ch,
        '\u{200E}' | '\u{200F}' // LRM, RLM
            | '\u{061C}' // ALM (Arabic Letter Mark)
            | '\u{202A}'..='\u{202E}' // LRE, RLE, PDF, LRO, RLO
            | '\u{2066}'..='\u{2069}' // LRI, RLI, FSI, PDI
    )
}

fn strip_ansi_sequences(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != 0x1b {
            let Some(ch) = input[index..].chars().next() else {
                break;
            };
            output.push(ch);
            index += ch.len_utf8();
            continue;
        }
        index += 1;
        match bytes.get(index).copied() {
            Some(b'[') => {
                index += 1;
                while index < bytes.len() {
                    let byte = bytes[index];
                    index += 1;
                    if (0x40..=0x7e).contains(&byte) {
                        break;
                    }
                }
            }
            Some(b']') => {
                index += 1;
                while index < bytes.len() {
                    match bytes[index] {
                        0x07 => {
                            index += 1;
                            break;
                        }
                        0x1b if bytes.get(index + 1) == Some(&b'\\') => {
                            index += 2;
                            break;
                        }
                        _ => index += 1,
                    }
                }
            }
            Some(b'P' | b'^' | b'_') => {
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == 0x1b && bytes.get(index + 1) == Some(&b'\\') {
                        index += 2;
                        break;
                    }
                    index += 1;
                }
            }
            Some(_) => index += 1,
            None => {}
        }
    }
    output
}

fn truncate_display_width(line: &str, max_width: usize) -> String {
    let width = display_width(line);
    if width <= max_width {
        return line.to_string();
    }
    let marker = "...";
    let marker_width = display_width(marker);
    if max_width <= marker_width {
        return marker.chars().take(max_width).collect();
    }
    let target = max_width - marker_width;
    let mut tail = String::new();
    let mut used = 0;
    for ch in line.chars().rev() {
        let width = char_display_width(ch);
        if used + width > target {
            break;
        }
        tail.insert(0, ch);
        used += width;
    }
    format!("...{tail}")
}

/// Approximate a string's terminal column width (double-width CJK/emoji
/// count as 2, combining marks as 0) — the same measure used throughout this
/// module's own alignment and clipping, exposed so other presentation code
/// does not need a second width algorithm.
pub(crate) fn display_width(text: &str) -> usize {
    text.chars().map(char_display_width).sum()
}

fn char_display_width(ch: char) -> usize {
    let code = ch as u32;
    if ch.is_control()
        || matches!(
            code,
            0x0300..=0x036f
                | 0x1ab0..=0x1aff
                | 0x1dc0..=0x1dff
                | 0x20d0..=0x20ff
                | 0xfe20..=0xfe2f
        )
    {
        0
    } else if matches!(
        code,
        0x1100..=0x115f
            | 0x2329..=0x232a
            | 0x2e80..=0xa4cf
            | 0xac00..=0xd7a3
            | 0xf900..=0xfaff
            | 0xfe10..=0xfe19
            | 0xfe30..=0xfe6f
            | 0xff00..=0xff60
            | 0xffe0..=0xffe6
            | 0x1f300..=0x1faff
    ) {
        2
    } else {
        1
    }
}

/// The current terminal's column width (`stderr`'s `ioctl`, falling back to
/// `$COLUMNS`), or `None` off a terminal — callers that need a concrete width
/// supply their own default.
pub(crate) fn terminal_columns() -> Option<usize> {
    terminal_size_from_ioctl()
        .and_then(|size| size.columns)
        .or_else(terminal_columns_from_env)
}

pub(crate) fn terminal_rows() -> Option<usize> {
    terminal_size_from_ioctl()
        .and_then(|size| size.rows)
        .or_else(terminal_rows_from_env)
}

pub(crate) fn repaint_line_limit() -> usize {
    terminal_rows()
        .unwrap_or(DEFAULT_TERMINAL_ROWS)
        .saturating_sub(1)
        .max(1)
}

#[derive(Clone, Copy)]
struct TerminalSize {
    columns: Option<usize>,
    rows: Option<usize>,
}

#[cfg(unix)]
fn terminal_size_from_ioctl() -> Option<TerminalSize> {
    use std::os::fd::AsRawFd;

    let mut size = std::mem::MaybeUninit::<libc::winsize>::zeroed();
    // SAFETY: ioctl only writes to the provided winsize pointer for TIOCGWINSZ;
    // the file descriptor is stderr's valid raw fd for the duration of the call.
    let result = unsafe {
        libc::ioctl(
            std::io::stderr().as_raw_fd(),
            libc::TIOCGWINSZ,
            size.as_mut_ptr(),
        )
    };
    if result == 0 {
        // SAFETY: a successful TIOCGWINSZ call initialized the winsize value.
        let size = unsafe { size.assume_init() };
        return Some(TerminalSize {
            columns: (size.ws_col > 0).then_some(size.ws_col as usize),
            rows: (size.ws_row > 0).then_some(size.ws_row as usize),
        });
    }
    None
}

#[cfg(not(unix))]
fn terminal_size_from_ioctl() -> Option<TerminalSize> {
    None
}

fn terminal_columns_from_env() -> Option<usize> {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|columns| *columns > 0)
}

fn terminal_rows_from_env() -> Option<usize> {
    std::env::var("LINES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|rows| *rows > 0)
}

fn write_error(source: std::io::Error) -> crate::Error {
    crate::Error::Command {
        message: format!("write styled terminal output: {source}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_text_uses_an_unbounded_zero_padded_clock() {
        let cases = [
            (0, "00:00:00"),
            (59, "00:00:59"),
            (60, "00:01:00"),
            (128, "00:02:08"),
            (3_599, "00:59:59"),
            (3_600, "01:00:00"),
            (93_784, "26:03:04"),
            (360_000, "100:00:00"),
        ];
        for (seconds, expected) in cases {
            assert_eq!(elapsed_text(Duration::from_secs(seconds)), expected);
        }
    }

    #[test]
    fn human_elapsed_text_preserves_compact_relative_units() {
        assert_eq!(human_elapsed_text(Duration::from_secs(8)), "8s");
        assert_eq!(human_elapsed_text(Duration::from_secs(128)), "2m 8s");
        assert_eq!(human_elapsed_text(Duration::from_secs(93_784)), "26h 3m 4s");
    }
}
