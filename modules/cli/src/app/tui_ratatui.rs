//! ratatui-backed live single-run pane for `--progress tui`, at rendering
//! parity with the hand-rolled ANSI redraw in [`super::tui`]. Same two-tone
//! palette doctrine carries over unchanged: named ANSI foreground colors plus
//! `DIM`/`BOLD` modifiers only — no backgrounds, no truecolor, no light/dark
//! detection. The hand-rolled redraw stays in place (it still backs
//! `LiveOutputPanel` for `--progress stream`, and `check`'s line-mode
//! styling) until P423 retires it.

use std::io::Stderr;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once, mpsc};
use std::time::{Duration, Instant};

use crossterm::cursor::Show;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as RatatuiLine, Span};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{TerminalOptions, Viewport};

use super::tui::{Line, Tone};

/// Monotonic pane identity: bumped once per successfully constructed
/// [`RatatuiPane`], never reused. Backs [`TORN_DOWN_GENERATION`] so torn-down
/// state is scoped to the exact pane instance that owned the terminal when
/// the panic fired, never leaking forward onto a later pane in the same
/// process and never leaking backward from a prior pane's leftover state —
/// each `RatatuiPane` is born with a fresh generation and is never "born
/// detached" by a predecessor's quit/panic.
static PANE_GENERATION: AtomicU64 = AtomicU64::new(0);
/// The generation of whichever pane, if any, currently owns the terminal.
/// Read by the panic hook so it can stamp exactly that pane as torn down.
static ACTIVE_GENERATION: AtomicU64 = AtomicU64::new(0);
/// Set by the panic hook to the generation of the pane it tore down. A pane
/// whose own generation matches this value must never draw again, even
/// though its `Terminal` handle still looks alive from its own point of
/// view — the real terminal underneath it was already restored to normal
/// mode by the hook.
static TORN_DOWN_GENERATION: AtomicU64 = AtomicU64::new(0);
static PANIC_HOOK: Once = Once::new();

/// Which screen mode the currently-active pane (per [`ACTIVE_GENERATION`])
/// owns, so the process-global panic hook — which has no `&self` to read a
/// mode off of — restores the terminal correctly for either mode. Stale
/// once `ACTIVE_GENERATION` returns to 0 (no active pane); harmless, since
/// the hook only reads it guarded by `active != 0`.
static ACTIVE_SCREEN: AtomicU8 = AtomicU8::new(PaneScreen::Alt as u8);

/// Which terminal mode a [`RatatuiPane`] owns: the historical full alternate
/// screen (dashboard, demo, trait editor), or P244's inline viewport (the
/// live run pane), which leaves the caller's scrollback above it intact for
/// the whole run and commits its final frame to scrollback on clean
/// teardown instead of restoring a blank alternate screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PaneScreen {
    Alt = 0,
    Inline = 1,
}

impl PaneScreen {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => PaneScreen::Inline,
            _ => PaneScreen::Alt,
        }
    }
}

/// Floor for the inline viewport's row budget — small terminals still get a
/// usable pane rather than being squeezed to nothing.
const INLINE_HEIGHT_FLOOR: u16 = 6;
const INLINE_RESIZE_INTERVAL: Duration = Duration::from_millis(50);

/// The inline pane's row budget. Ratatui 0.29 cannot resize an existing
/// `Viewport::Inline`, so P543 rebuilds only the terminal on a size change.
/// Two constraints, and which one wins depends on the terminal's own size:
/// on a terminal comfortably taller than [`INLINE_HEIGHT_FLOOR`], one row is
/// reserved so the pane never overwrites the line the shell prompt will
/// return to. On a terminal AT or BELOW the floor, the floor's need for a
/// minimally usable pane wins instead — the prompt row is not reserved — but
/// the result is still capped at `terminal_rows`, so the pane never claims
/// more rows than actually exist.
fn inline_viewport_height(terminal_rows: u16) -> u16 {
    if terminal_rows <= 1 {
        return terminal_rows;
    }
    let leave_one = terminal_rows - 1;
    leave_one.max(INLINE_HEIGHT_FLOOR).min(terminal_rows)
}

fn inline_terminal(rows: u16) -> std::io::Result<Terminal<CrosstermBackend<Stderr>>> {
    Terminal::with_options(
        CrosstermBackend::new(std::io::stderr()),
        TerminalOptions {
            viewport: Viewport::Inline(inline_viewport_height(rows)),
        },
    )
}

/// Installs a panic hook (once per process) that restores the terminal
/// before chaining to whatever hook was previously installed. `Drop` already
/// restores the terminal on every normal unwind-free exit path; this hook
/// additionally marks the pane that was active at panic time as torn down
/// so a panic caught elsewhere (e.g. `catch_unwind` around a worker) can
/// never resume drawing onto the now-restored normal screen. Gated on a
/// nonzero active generation: once every pane has cleanly detached (clearing
/// `ACTIVE_GENERATION` back to 0), a later panic owns no terminal state and
/// must not emit alternate-screen/restore escapes onto the already-restored
/// line-mode screen.
fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let active = ACTIVE_GENERATION.load(Ordering::SeqCst);
            if active != 0 {
                TORN_DOWN_GENERATION.store(active, Ordering::SeqCst);
                restore_terminal(PaneScreen::from_u8(ACTIVE_SCREEN.load(Ordering::SeqCst)));
            }
            previous(info);
        }));
    });
}

/// Restores cooked mode and a visible cursor, plus the normal screen for an
/// [`PaneScreen::Alt`] pane. Idempotent and safe to call on a terminal that
/// is already restored (or was never fully entered) — every step best-effort
/// no-ops rather than propagating. An inline pane never entered the
/// alternate screen, so restoring it never leaves it — only cooked mode and
/// cursor visibility need undoing. Also defensively ends decode-warning
/// capture (see [`ctx_traits_io::decode_diagnostics::begin_capture`]) for an
/// inline pane: the clean-teardown path already drains it via
/// [`RatatuiPane::commit_inline_scrollback`] before reaching here, so this is
/// a no-op on that path — it exists only so a panic mid-run (which reaches
/// this function without ever calling `commit_inline_scrollback`) can never
/// leave capture stuck on, silently swallowing every later decode warning
/// for the rest of the process.
fn restore_terminal(screen: PaneScreen) {
    let _ = disable_raw_mode();
    match screen {
        PaneScreen::Alt => {
            let _ = execute!(std::io::stderr(), LeaveAlternateScreen, Show);
        }
        PaneScreen::Inline => {
            let _ = ctx_traits_io::decode_diagnostics::end_capture();
            let _ = execute!(std::io::stderr(), Show);
        }
    }
}

/// Whether the pump thread's hard-wired ctrl-c handling (P551:
/// `interrupt::request_kill()` — an instant kill, not the cooperative
/// `request_stop()`) fires for a given [`RatatuiPane`]. Correct for the live
/// run pane (there is nothing else ctrl-c could mean there); wrong for
/// dashboard/demo-style screens with a top-level exit-confirm flow, where
/// cancelling that confirmation must not leave the global kill flag set.
/// `ForwardKey` panes still receive the key on their channel — only the
/// side-effecting global kill request is skipped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CtrlCPolicy {
    RequestStop,
    ForwardKey,
}

/// Shared switches for the dedicated input-pump thread: `stop` ends the
/// thread (set before the terminal is restored, so the pump can never eat a
/// keystroke meant for the shell), `paused` parks it without reading (set
/// around [`RatatuiPane::suspend`], so the pump never steals keys from an
/// `$EDITOR` that owns the tty), `ctrl_c_policy` selects the ctrl-c behavior
/// above.
struct PumpControl {
    stop: AtomicBool,
    paused: AtomicBool,
    /// Latest terminal row count observed by the input pump. A single atomic
    /// coalesces drag-resize bursts; the render tick consumes it once.
    resize_rows: AtomicU16,
    input_generation: Arc<AtomicU64>,
    wake: Mutex<Option<Arc<dyn Fn() + Send + Sync + 'static>>>,
    ctrl_c_policy: CtrlCPolicy,
}

/// One ratatui alternate-screen pane for a live `--progress tui` run: an RAII
/// terminal-ownership state machine. Construction is transactional (any
/// setup step failing after an earlier one succeeded rolls that earlier step
/// back before returning), teardown is idempotent from every path (normal
/// drop, explicit quit, or the process panic hook), and draw calls are
/// gated on both this instance's own state and the process-wide torn-down
/// generation so a panic caught elsewhere can never cause a later draw onto
/// the restored normal screen.
pub(crate) struct RatatuiPane {
    terminal: Option<Terminal<CrosstermBackend<Stderr>>>,
    generation: u64,
    screen: PaneScreen,
    detached: bool,
    /// Key presses forwarded by the pump thread. Reading events on a
    /// dedicated thread (instead of polling inside draw calls) is what keeps
    /// the quit keys responsive during silent stretches — a worker thinking
    /// for minutes produces no stream chunks, therefore no renders, and
    /// before this pump that meant no key handling at all.
    keys: mpsc::Receiver<crossterm::event::KeyEvent>,
    pump: Arc<PumpControl>,
    /// The row budget used to construct the current inline terminal. Keeping
    /// it on the pane makes duplicate resize events no-ops without recreating
    /// any run-panel presentation state.
    inline_height: Option<u16>,
    last_inline_resize: Instant,
}

impl RatatuiPane {
    /// Constructs a pane whose pump forwards ctrl-c as a plain key without
    /// the `request_stop()` side effect — for dashboard/demo/editor contexts
    /// under a top-level exit-confirm flow.
    pub(crate) fn new_forwarding_ctrl_c() -> std::io::Result<Self> {
        Self::new_with_options(PaneScreen::Alt, CtrlCPolicy::ForwardKey)
    }

    /// P244: constructs the live run pane on ratatui's `Viewport::Inline`
    /// instead of the alternate screen — the caller's scrollback above the
    /// pane survives the whole run, and the final frame is committed to
    /// scrollback on clean teardown (see [`Self::commit_inline_scrollback`])
    /// rather than being discarded by an alternate-screen restore. Ctrl-c
    /// policy matches [`Self::new`]: this is still the live-run pane, with
    /// nothing else ctrl-c could mean there.
    pub(crate) fn new_inline() -> std::io::Result<Self> {
        Self::new_with_options(PaneScreen::Inline, CtrlCPolicy::RequestStop)
    }

    fn new_with_options(screen: PaneScreen, ctrl_c_policy: CtrlCPolicy) -> std::io::Result<Self> {
        install_panic_hook();
        enable_raw_mode()?;
        let mut inline_height = None;
        let terminal = match screen {
            PaneScreen::Alt => {
                if let Err(err) = execute!(std::io::stderr(), EnterAlternateScreen) {
                    let _ = disable_raw_mode();
                    return Err(err);
                }
                match Terminal::new(CrosstermBackend::new(std::io::stderr())) {
                    Ok(terminal) => terminal,
                    Err(err) => {
                        // Roll back every mode already entered before
                        // propagating — otherwise a `Terminal::new` failure
                        // leaves the caller's terminal stuck in
                        // raw/alternate-screen mode.
                        let _ = execute!(std::io::stderr(), LeaveAlternateScreen, Show);
                        let _ = disable_raw_mode();
                        return Err(err);
                    }
                }
            }
            PaneScreen::Inline => {
                let rows = match crossterm::terminal::size() {
                    Ok((_, rows)) => rows,
                    Err(err) => {
                        let _ = disable_raw_mode();
                        return Err(err);
                    }
                };
                let height = inline_viewport_height(rows);
                let terminal = match inline_terminal(rows) {
                    Ok(terminal) => terminal,
                    Err(err) => {
                        let _ = disable_raw_mode();
                        return Err(err);
                    }
                };
                inline_height = Some(height);
                terminal
            }
        };
        let generation = PANE_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        ACTIVE_GENERATION.store(generation, Ordering::SeqCst);
        ACTIVE_SCREEN.store(screen as u8, Ordering::SeqCst);
        if screen == PaneScreen::Inline {
            // P244 fix (`inline-pane-stderr-interleave`): route every decode
            // warning reachable on the drive loop's per-frame path (§
            // `commit_inline_scrollback`'s own doc comment names the exact
            // call sites) into the buffer this pane drains at clean teardown,
            // instead of letting a raw `eprintln!` scroll the real screen out
            // from under a viewport that has no way to notice it happened.
            ctx_traits_io::decode_diagnostics::begin_capture();
        }
        let pump = Arc::new(PumpControl {
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            resize_rows: AtomicU16::new(0),
            input_generation: Arc::new(AtomicU64::new(0)),
            wake: Mutex::new(None),
            ctrl_c_policy,
        });
        let (sender, keys) = mpsc::channel();
        {
            let pump = Arc::clone(&pump);
            std::thread::spawn(move || {
                loop {
                    if pump.stop.load(Ordering::SeqCst) {
                        return;
                    }
                    if pump.paused.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(50));
                        continue;
                    }
                    // Bounded poll so the stop/paused switches are observed
                    // promptly; the stop check runs again before `read` so a
                    // leave() that raced the poll window still wins before
                    // any byte is consumed off the tty.
                    if !event::poll(Duration::from_millis(100)).unwrap_or(false) {
                        continue;
                    }
                    if pump.stop.load(Ordering::SeqCst) || pump.paused.load(Ordering::SeqCst) {
                        continue;
                    }
                    let Ok(read) = event::read() else { return };
                    match read {
                        Event::Resize(_, rows) => {
                            pump.resize_rows.store(rows, Ordering::SeqCst);
                            notify_input(&pump);
                        }
                        Event::Key(key) => {
                            if key.kind != crossterm::event::KeyEventKind::Press {
                                continue;
                            }
                            // Ctrl-c acts HERE, not just when the channel is next
                            // drained: renders (and therefore `poll_detach`) only
                            // happen when output arrives, and a silent multi-minute
                            // frame must not postpone the stop request. The key is
                            // still forwarded so `poll_detach` performs the visible
                            // detach + note on its next run.
                            if key.code == KeyCode::Char('c')
                                && key.modifiers.contains(KeyModifiers::CONTROL)
                                && pump.ctrl_c_policy == CtrlCPolicy::RequestStop
                            {
                                crate::app::interrupt::request_kill();
                            }
                            if sender.send(key).is_err() {
                                return;
                            }
                            notify_input(&pump);
                        }
                        _ => {}
                    }
                }
            });
        }
        Ok(Self {
            terminal: Some(terminal),
            generation,
            screen,
            detached: false,
            keys,
            pump,
            inline_height,
            last_inline_resize: Instant::now(),
        })
    }

    pub(crate) fn input_generation(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.pump.input_generation)
    }

    pub(crate) fn install_input_wake(&mut self, wake: Arc<dyn Fn() + Send + Sync + 'static>) {
        if let Ok(mut installed) = self.pump.wake.lock() {
            *installed = Some(wake);
        }
    }

    /// Applies the most recent coalesced inline resize. The old viewport is
    /// re-anchored and cleared before replacement construction; a failed
    /// replacement keeps it as the redraw path. No full `leave` path runs
    /// during this swap.
    pub(crate) fn apply_resize(&mut self) -> bool {
        if self.screen != PaneScreen::Inline || self.detached() {
            return false;
        }
        let rows = self.pump.resize_rows.swap(0, Ordering::SeqCst);
        let height = inline_viewport_height(rows);
        if rows == 0 || self.inline_height == Some(height) {
            return false;
        }
        if self.last_inline_resize.elapsed() < INLINE_RESIZE_INTERVAL {
            self.requeue_resize(rows);
            return false;
        }
        let Some(old_terminal) = self.terminal.as_mut() else {
            return false;
        };
        if old_terminal.clear().is_err() {
            self.requeue_resize(rows);
            return false;
        }
        match inline_terminal(rows) {
            Ok(mut terminal) => {
                // A fresh terminal has blank diff buffers, so clear its
                // viewport before its first draw to erase any old-frame cells.
                if terminal.clear().is_err() {
                    self.requeue_resize(rows);
                    return true;
                }
                self.terminal = Some(terminal);
                self.inline_height = Some(height);
                self.last_inline_resize = Instant::now();
                true
            }
            Err(_) => {
                // The old terminal was cleared above and remains owned here;
                // request its redraw and retry the resize on a later tick.
                self.requeue_resize(rows);
                true
            }
        }
    }

    pub(crate) fn resize_pending(&self) -> bool {
        self.pump.resize_rows.load(Ordering::SeqCst) != 0
    }

    fn requeue_resize(&self, rows: u16) {
        // A newer event wins over a failed/debounced older size.
        let _ = self
            .pump
            .resize_rows
            .compare_exchange(0, rows, Ordering::SeqCst, Ordering::SeqCst);
    }

    /// True once this pane can no longer draw: the user asked to stop
    /// (Ctrl-C), or the process panic hook tore down the terminal while this
    /// pane was the active one. Per-instance, not process-global — a later
    /// pane in the same process is never born detached by an earlier one's
    /// quit or panic.
    pub(crate) fn detached(&self) -> bool {
        self.detached || TORN_DOWN_GENERATION.load(Ordering::SeqCst) == self.generation
    }

    /// Non-blocking drain of key presses forwarded by the pump thread. P470:
    /// the live-run pane binds scroll (↑/↓/j/k/PgUp/PgDn) and focus-cycle
    /// (Tab); P551 adds `q` (routed by the caller into its own confirm-quit
    /// modal — this method has no opinion on `q`, it only forwards it like
    /// any other unhandled key). Ctrl-C is still handled entirely inside
    /// this method (never returned to the caller): it requests an instant
    /// kill of the active harness/setup process group (the same flag a real
    /// `SIGINT` sets for cooperative-stop bookkeeping, but in raw mode
    /// Ctrl-C never becomes a signal — this key path is its only meaning
    /// here, and unlike a real `SIGINT` it does not wait for the current
    /// frame) and leaves the pane, printing a one-line plain-text note once
    /// the terminal is back in cooked mode so the user knows what state the
    /// run is in. Every other drained key is returned for the caller to
    /// apply to its own scroll/focus/modal state.
    pub(crate) fn poll_detach(&mut self) -> Vec<KeyEvent> {
        if self.detached() {
            return Vec::new();
        }
        let mut unhandled = Vec::new();
        while let Ok(key) = self.keys.try_recv() {
            let ctrl_c =
                key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
            if ctrl_c {
                crate::app::interrupt::request_kill();
                self.detached = true;
                self.leave();
                eprintln!("run killed; terminal restored");
                return unhandled;
            }
            unhandled.push(key);
        }
        unhandled
    }

    /// General-purpose draw entry point for the P423 dashboard's screen
    /// hierarchy: callers build arbitrary ratatui widgets against the given
    /// frame instead of being limited to [`Self::render`]'s single styled-line
    /// paragraph. Gated on `detached()` exactly like every other draw path so
    /// a panic caught elsewhere can never cause a later frame onto the
    /// restored normal screen.
    pub(crate) fn draw(
        &mut self,
        widget: impl FnOnce(&mut ratatui::Frame<'_>),
    ) -> std::io::Result<()> {
        if self.detached() {
            return Ok(());
        }
        let Some(terminal) = self.terminal.as_mut() else {
            return Ok(());
        };
        terminal.draw(widget)?;
        Ok(())
    }

    /// Blocking-with-timeout wait for the next key press, for callers (the
    /// dashboard) that need more than [`Self::poll_detach`]'s quit-only
    /// handling. Returns `None` on timeout or once this pane is detached.
    /// Presses arrive from the same pump thread as `poll_detach`'s (already
    /// filtered to `Press` there, so `Repeat`/`Release` reported by some
    /// terminals never double-handle a physical keystroke).
    pub(crate) fn poll_key(
        &mut self,
        timeout: Duration,
    ) -> std::io::Result<Option<crossterm::event::KeyEvent>> {
        if self.detached() {
            return Ok(None);
        }
        match self.keys.recv_timeout(timeout) {
            Ok(key) => Ok(Some(key)),
            Err(_) => Ok(None),
        }
    }

    /// Mark this pane detached from an explicit dashboard-level action (e.g.
    /// the top-level quit key), restoring the terminal exactly like
    /// [`Self::poll_detach`]'s quit path.
    pub(crate) fn quit(&mut self) {
        if !self.detached() {
            self.detached = true;
            self.leave();
        }
    }

    /// P244 §3.2 point 6: on a clean teardown of an inline pane, commits
    /// `lines` (the run's final rendered tree) into scrollback via
    /// `Terminal::insert_before` — so the pane's last frame lands in the
    /// user's real scrollback instead of vanishing once [`Self::leave`]
    /// hands the terminal back. `insert_before_no_scrolling_regions` already
    /// clears the viewport itself once it finishes (ratatui 0.29's own
    /// implementation), so this does not clear a second time.
    ///
    /// Also drains and appends whatever decode warnings were captured by
    /// [`ctx_traits_io::decode_diagnostics::begin_capture`] during the run
    /// (fix `inline-pane-stderr-interleave`) — the same warnings that would
    /// otherwise have corrupted the live viewport as raw `eprintln!` writes
    /// are appended here, after the tree, in the same styled-line surface
    /// every other diagnostic in this pane already uses.
    ///
    /// A no-op for an [`PaneScreen::Alt`] pane (nothing analogous applies),
    /// and for a detached/already-torn-down pane (nothing left to commit
    /// onto). Must be called before [`Self::quit`]/[`Self::leave`], while
    /// the terminal handle is still live. Only ever called once per pane, at
    /// teardown — `insert_before` without `scrolling-regions` repaints the
    /// whole viewport (draft §3.2 point 6), which is fine once and would
    /// flicker per-frame.
    pub(crate) fn commit_inline_scrollback(&mut self, lines: &[Line]) -> std::io::Result<()> {
        if self.screen != PaneScreen::Inline || self.detached() {
            return Ok(());
        }
        let Some(terminal) = self.terminal.as_mut() else {
            return Ok(());
        };
        let captured_warnings = ctx_traits_io::decode_diagnostics::end_capture();
        let mut rendered = lines.iter().map(render_line).collect::<Vec<_>>();
        rendered.extend(
            captured_warnings
                .into_iter()
                .map(|text| RatatuiLine::from(Span::styled(text, style_for(Tone::Warn)))),
        );
        let height = u16::try_from(rendered.len()).unwrap_or(u16::MAX);
        terminal.insert_before(height, |buf| {
            Paragraph::new(rendered).render(buf.area, buf);
        })?;
        Ok(())
    }

    /// Scope terminal ownership out to an external interactive program (an
    /// `$EDITOR` invocation): leave raw/alternate-screen mode, run `body`,
    /// then re-enter and force a full repaint on the next [`Self::draw`]
    /// (ratatui's own diff would otherwise skip repainting cells that look
    /// unchanged from its stale last-drawn buffer, leaving editor debris on
    /// screen). Re-entry runs even if `body` panics, via a drop guard, so a
    /// failing editor invocation can never strand the terminal in cooked
    /// mode.
    pub(crate) fn suspend<R>(&mut self, body: impl FnOnce() -> R) -> std::io::Result<R> {
        if self.detached() {
            // Nothing to suspend: the terminal is already restored (a panic
            // elsewhere tore it down, or the user already quit). Still run
            // `body` so callers that suspend purely to invoke an editor keep
            // working, just without anything left to re-enter afterward.
            return Ok(body());
        }
        // The guard is installed BEFORE either leave step runs (not after),
        // so a `?` that propagates out of `disable_raw_mode`/`execute!` below
        // still triggers `ReenterGuard::drop`'s rollback rather than
        // stranding the terminal half-left. `enable_raw_mode`/
        // `EnterAlternateScreen` are each idempotent against a mode that was
        // never actually left, so rolling back an incomplete leave is safe.
        let generation = self.generation;
        let screen = self.screen;
        let reentry_ok = std::rc::Rc::new(std::cell::Cell::new(true));
        struct ReenterGuard {
            generation: u64,
            screen: PaneScreen,
            reentry_ok: std::rc::Rc<std::cell::Cell<bool>>,
        }
        impl Drop for ReenterGuard {
            fn drop(&mut self) {
                // A panic caught elsewhere (or unwinding through this very
                // suspend) may already have run the panic hook and restored
                // the terminal to normal mode for this exact pane generation
                // — re-entering alternate/raw mode on top of that would fight
                // the hook's own restore and leave the real terminal in
                // whatever state loses the race. Once this generation is
                // recorded torn down, never re-enter.
                if TORN_DOWN_GENERATION.load(Ordering::SeqCst) == self.generation {
                    self.reentry_ok.set(false);
                    return;
                }
                let raw_ok = enable_raw_mode().is_ok();
                let screen_ok = match self.screen {
                    PaneScreen::Alt => execute!(std::io::stderr(), EnterAlternateScreen).is_ok(),
                    // Inline never left the alternate screen (it was never
                    // in one) — only raw mode needs re-entering.
                    PaneScreen::Inline => true,
                };
                self.reentry_ok.set(raw_ok && screen_ok);
            }
        }
        let guard = ReenterGuard {
            generation,
            screen,
            reentry_ok: reentry_ok.clone(),
        };
        // Park the pump BEFORE cooked mode returns: a pump still reading
        // would steal keystrokes from the interactive program (`$EDITOR`)
        // that owns the tty for the duration of `body`.
        self.pump.paused.store(true, Ordering::SeqCst);
        disable_raw_mode()?;
        match screen {
            PaneScreen::Alt => {
                execute!(std::io::stderr(), LeaveAlternateScreen, Show)?;
            }
            PaneScreen::Inline => {
                execute!(std::io::stderr(), Show)?;
            }
        }
        let result = body();
        drop(guard);
        self.pump.paused.store(false, Ordering::SeqCst);
        if reentry_ok.get() {
            if let Some(terminal) = self.terminal.as_mut() {
                terminal.clear()?;
            }
        } else {
            // Re-entry failed (or was skipped because the pane was already
            // torn down mid-suspend): terminal ownership can no longer be
            // trusted, so this pane must behave as detached from now on
            // rather than risk drawing onto whatever state the terminal is
            // actually left in. `body`'s own result is still returned — it
            // completed successfully — the caller's main loop already exits
            // once `detached()` is true.
            self.terminal = None;
            self.detached = true;
            let _ = ACTIVE_GENERATION.compare_exchange(
                generation,
                0,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
        }
        Ok(result)
    }

    fn leave(&mut self) {
        if self.terminal.take().is_some() {
            // Stop the pump BEFORE cooked mode returns, so it can never eat
            // a keystroke meant for the shell after the pane is gone.
            self.pump.stop.store(true, Ordering::SeqCst);
            restore_terminal(self.screen);
            // Idempotent regardless of race: worst case a concurrent panic
            // hook re-stamps `ACTIVE_GENERATION` for its own generation right
            // after this clears it, which is exactly the state it needs.
            let _ = ACTIVE_GENERATION.compare_exchange(
                self.generation,
                0,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
        }
    }
}

fn notify_input(pump: &PumpControl) {
    pump.input_generation.fetch_add(1, Ordering::SeqCst);
    let wake = pump.wake.lock().ok().and_then(|wake| wake.clone());
    if let Some(wake) = wake {
        wake();
    }
}

impl Drop for RatatuiPane {
    fn drop(&mut self) {
        self.leave();
    }
}

pub(crate) fn render_line(line: &Line) -> RatatuiLine<'static> {
    RatatuiLine::from(
        line.segments()
            .map(|(text, tone)| Span::styled(text.to_string(), style_for(tone)))
            .collect::<Vec<_>>(),
    )
}

/// Bridges the shared two-tone palette (`Tone`) onto ratatui `Style`s: named
/// ANSI foreground colors and `DIM`/`BOLD` modifiers only. Must stay in
/// lockstep with `tui::paint`'s escape codes, which the hand-rolled redraw
/// still uses.
fn style_for(tone: Tone) -> Style {
    match tone {
        Tone::Default => Style::default(),
        Tone::Muted => Style::default().add_modifier(Modifier::DIM),
        Tone::Pass => Style::default().fg(Color::Green),
        Tone::Fail => Style::default().fg(Color::Red),
        Tone::Warn => Style::default().fg(Color::Yellow),
        Tone::Bold => Style::default().add_modifier(Modifier::BOLD),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn input_notification_advances_generation_and_wakes_after_queueing() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let control = PumpControl {
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            resize_rows: AtomicU16::new(0),
            input_generation: Arc::new(AtomicU64::new(0)),
            wake: Mutex::new(Some({
                let wakes = Arc::clone(&wakes);
                Arc::new(move || {
                    wakes.fetch_add(1, Ordering::SeqCst);
                })
            })),
            ctrl_c_policy: CtrlCPolicy::RequestStop,
        };
        notify_input(&control);
        notify_input(&control);
        assert_eq!(control.input_generation.load(Ordering::SeqCst), 2);
        assert_eq!(wakes.load(Ordering::SeqCst), 2);
    }

    // P244 §3.2 point 3: leaves exactly one row for the shell prompt once
    // the terminal is comfortably taller than the floor.
    #[test]
    fn inline_viewport_height_leaves_one_row_for_the_prompt() {
        assert_eq!(inline_viewport_height(24), 23);
        assert_eq!(inline_viewport_height(100), 99);
    }

    // P244 blocker `inline-height-floor-noop`: below (or at) the floor, the
    // floor actually binds — the pane claims every row the terminal has
    // instead of reserving one for the prompt — rather than the floor being
    // an always-discarded no-op.
    #[test]
    fn inline_viewport_height_floor_binds_on_a_short_terminal() {
        assert_eq!(
            inline_viewport_height(INLINE_HEIGHT_FLOOR),
            INLINE_HEIGHT_FLOOR
        );
        assert_eq!(inline_viewport_height(5), 5);
        assert_eq!(inline_viewport_height(2), 2);
    }

    #[test]
    fn inline_viewport_height_never_exceeds_or_underflows_a_tiny_terminal() {
        assert_eq!(inline_viewport_height(0), 0);
        assert_eq!(inline_viewport_height(1), 1);
    }
}
