//! ratatui-backed live single-run pane for `--progress tui`, at rendering
//! parity with the hand-rolled ANSI redraw in [`super::tui`]. Same two-tone
//! palette doctrine carries over unchanged: named ANSI foreground colors plus
//! `DIM`/`BOLD` modifiers only — no backgrounds, no truecolor, no light/dark
//! detection. The hand-rolled redraw stays in place (it still backs
//! `LiveOutputPanel` for `--progress stream`, and `check`'s line-mode
//! styling) until P423 retires it.

use std::io::{IsTerminal, Stderr, Write};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once, mpsc};
use std::time::{Duration, Instant};

use crossterm::cursor::Show;
use crossterm::event::{
    self, DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture, Event,
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as RatatuiLine, Span};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{TerminalOptions, Viewport};

use super::tui::{Line, Tone};
use super::tui_select;

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

/// Async-signal-safe check for whether any [`RatatuiPane`] currently owns
/// the terminal — a single atomic load, safe to call from
/// `interrupt::handle_sigint`/`handle_sigterm`. Gates the signal-safe rescue
/// escape write (see [`crate::app::interrupt`]) so a headless kill, where no
/// pane was ever constructed, never writes terminal escapes onto a piped
/// stderr.
pub(crate) fn signal_safe_pane_active() -> bool {
    ACTIVE_GENERATION.load(Ordering::SeqCst) != 0
}

/// The canonical "leave every mode a pane can enter" escape sequence:
/// disable mouse reporting (`?1006l ?1003l ?1002l ?1000l`, reverse of
/// crossterm's `EnableMouseCapture` enable order), disable focus reporting
/// (`?1004l`), leave the alternate screen (`?1049l`), and show the cursor
/// (`?25h`), then a bare CR/LF so a following shell prompt does not run on
/// from wherever the cursor was left. Every step is idempotent against a
/// mode that was never entered (an inline pane never enters the alternate
/// screen, for instance), so this single sequence is correct regardless of
/// [`PaneScreen`]. Used verbatim, byte-for-byte, by
/// `interrupt::rescue_terminal_signal_safe` — kept `pub(crate)` and
/// colocated here rather than duplicated so it can never silently drift out
/// of lockstep with the crossterm calls [`restore_terminal`] issues below;
/// `tests::full_restore_escape_disables_every_mode_the_pane_enables` pins
/// the invariant.
pub(crate) const FULL_RESTORE_ESCAPE: &[u8] =
    b"\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[?1004l\x1b[?1049l\x1b[?25h\r\n";

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

const INLINE_RESIZE_INTERVAL: Duration = Duration::from_millis(50);

/// The inline pane owns the terminal's complete row budget. Ratatui 0.29
/// cannot resize an existing `Viewport::Inline`, so a size change rebuilds
/// only the terminal while the caller retains its presentation state.
fn inline_viewport_height(terminal_rows: u16) -> u16 {
    terminal_rows
}

fn pack_terminal_size(columns: u16, rows: u16) -> u32 {
    u32::from(columns) << 16 | u32::from(rows)
}

fn unpack_terminal_size(size: u32) -> (u16, u16) {
    ((size >> 16) as u16, size as u16)
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
    let _ = execute!(std::io::stderr(), DisableFocusChange);
    let _ = disable_raw_mode();
    match screen {
        PaneScreen::Alt => {
            // `DisableMouseCapture` is unconditional and best-effort here,
            // regardless of whether `EnableMouseCapture` ever actually
            // succeeded (it is idempotent against a terminal that never had
            // capture on) — this is the one path every Alt-screen exit
            // (drop, quit, panic hook, a failed construction rollback) goes
            // through, so it is the only place that needs to get this right.
            // Leaving a terminal with mouse reporting stuck on is a worse
            // trap than the feature this disables.
            let _ = execute!(
                std::io::stderr(),
                DisableMouseCapture,
                LeaveAlternateScreen,
                Show
            );
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
    /// Focus reporting is optional terminal protocol support. Start focused so
    /// terminals that never send DEC ?1004 events retain the normal view.
    focused: AtomicBool,
    /// Latest terminal dimensions observed by the input pump. A single atomic
    /// coalesces drag-resize bursts; the render tick consumes it once.
    resize_size: AtomicU32,
    /// Task 0023: mouse-drag selection, `Alt` panes only. `mouse_anchor` is
    /// the packed (column, row) of the mouse-down that started the current
    /// selection; `mouse_current` is the packed position of the latest
    /// drag/up event, coalesced the same way `resize_size` coalesces resize
    /// bursts — a fast drag never floods a channel, [`RatatuiPane::draw`]
    /// just reads the latest position each render. `mouse_selecting` is true
    /// from mouse-down until the pending mouseup has been consumed;
    /// `mouse_up_pending` is set by mouse-up and swapped off by `draw` once
    /// it has extracted and copied the selected text. `mouse_dirty` is set by
    /// every handled mouse arm (down/drag/up) and swapped off by
    /// [`RatatuiPane::take_mouse_changed`] — the signal a tick's `changed`
    /// computation folds in so a wake the pump raises for mouse input is
    /// actually recognised as something to paint, the same way a resize or a
    /// key already is.
    mouse_anchor: AtomicU32,
    mouse_current: AtomicU32,
    mouse_selecting: AtomicBool,
    mouse_up_pending: AtomicBool,
    mouse_dirty: AtomicBool,
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
    /// The dimensions used to construct the current inline terminal. Keeping
    /// them on the pane makes duplicate resize events no-ops without
    /// recreating any run-panel presentation state.
    inline_size: Option<(u16, u16)>,
    last_inline_resize: Instant,
}

/// Which screen an inline-preferring pane can actually be built on.
///
/// `Viewport::Inline` is the pane we want: it keeps the caller's scrollback
/// above the run and commits its last frame into it on teardown. Building one
/// costs a cursor-position query, and crossterm answers that by writing
/// `ESC [ 6 n` and looping on `poll_internal` (`cursor/sys/unix.rs`). That loop
/// returns an error on TIMEOUT but SWALLOWS a poll error and retries with no
/// backoff and no bound:
///
/// ```text
/// Err(_) => {}
/// ```
///
/// So on a terminal whose reply never reaches crossterm's reader the query
/// never returns. It spins one core at 100% inside `Terminal::with_options`,
/// before the first frame, with an empty screen and no diagnostic — and it
/// cannot be bounded from out here, because it never yields.
///
/// It reproduces exactly when stdin is not the terminal, which is what `just`
/// hands every recipe. Enabling crossterm's `use-dev-tty` does NOT avoid it;
/// that was tried and the hang is identical.
///
/// [`adopt_controlling_terminal`] removes the cause rather than working around
/// it, so this only picks the fallback when even that could not produce a
/// terminal — a genuinely headless invocation, where the alternate screen at
/// least renders (fullscreen viewport, no cursor query) instead of hanging.
fn inline_capable_screen() -> PaneScreen {
    if std::io::stdin().is_terminal() && !stdin_was_adopted() {
        PaneScreen::Inline
    } else {
        PaneScreen::Alt
    }
}

/// Put a real terminal on stdin when the process was handed something else.
///
/// crossterm reads terminal replies — cursor position, and every key event —
/// through one event source anchored to stdin. Given a pipe there, that source
/// answers `Err` forever, and both of crossterm's consumers spin on it rather
/// than failing: the cursor query in `cursor/sys/unix.rs` and our own input
/// pump both discard the error and retry immediately. The visible result is a
/// pane that never draws, or draws and then ignores every key at 100% CPU.
///
/// `just` hands every recipe a pipe on stdin, so `just implement` got exactly
/// that. The fix is not to detect the condition and degrade — it is to stop
/// being in it. A process with a controlling terminal can open `/dev/tty` and
/// put it on fd 0, after which stdin IS a terminal and every downstream
/// consumer, crossterm included, simply works. The inline pane comes back with
/// it, because the cursor query it needs can now be answered.
///
/// Idempotent, and a no-op when stdin is already a terminal or when no
/// controlling terminal exists (CI, a daemon, a detached spawn) — those keep
/// today's behaviour and fall to the alternate screen or to status progress.
/// True once [`adopt_controlling_terminal`] actually replaced stdin. The
/// inline viewport needs a cursor-position reply, and on an adopted stdin that
/// reply is contended: the startup pane's input pump is already draining the
/// same process-global crossterm reader, so the query can spin unanswered.
/// The alternate screen needs no cursor query at all, so an adopted terminal
/// takes that path — working input beats preserved scrollback.
static STDIN_ADOPTED: AtomicBool = AtomicBool::new(false);

pub(crate) fn stdin_was_adopted() -> bool {
    STDIN_ADOPTED.load(Ordering::SeqCst)
}

pub(crate) fn adopt_controlling_terminal() {
    static ADOPTED: Once = Once::new();
    ADOPTED.call_once(|| {
        if std::io::stdin().is_terminal() {
            return;
        }
        // Prefer duplicating a stream we ALREADY hold on the terminal —
        // stderr is where the pane draws, stdout next — over opening
        // `/dev/tty`. Both make `isatty` true, but they are not equivalent to
        // the thing that has to happen next: crossterm registers the fd with
        // mio/kqueue, and the `/dev/tty` alias (device 2,0) is not the pty
        // slave (16,N) and fails to register. The result is a reader whose
        // source is `None` forever — "Failed to initialize input reader" on
        // every poll, and not one key delivered. Duplicating fd 2 hands over
        // the real slave, which registers.
        let source_fd = if std::io::stderr().is_terminal() {
            libc::STDERR_FILENO
        } else if std::io::stdout().is_terminal() {
            libc::STDOUT_FILENO
        } else {
            return;
        };
        // `dup2` duplicates onto fd 0; the borrowed handle is closed on drop
        // and fd 0 stays valid. A failure leaves the original stdin in place,
        // which is exactly the no-op we want.
        unsafe {
            if libc::dup2(source_fd, libc::STDIN_FILENO) >= 0 {
                STDIN_ADOPTED.store(true, Ordering::SeqCst);
            }
        }
    });
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
        Self::new_with_options(inline_capable_screen(), CtrlCPolicy::RequestStop)
    }

    fn new_with_options(screen: PaneScreen, ctrl_c_policy: CtrlCPolicy) -> std::io::Result<Self> {
        // Before anything asks crossterm a question: make sure stdin is a
        // terminal it can actually read the answer from.
        adopt_controlling_terminal();
        install_panic_hook();
        // Every pane-owning surface (dashboard, demo, editor, the live run
        // pane) needs the SIGTERM/SIGHUP/SIGINT rescue ladder, not just the
        // drive loop's own `drive.rs` call — both call sites are idempotent
        // via the same `Once`.
        crate::app::interrupt::install();
        enable_raw_mode()?;
        if let Err(err) = execute!(std::io::stderr(), EnableFocusChange) {
            let _ = execute!(std::io::stderr(), DisableFocusChange);
            let _ = disable_raw_mode();
            return Err(err);
        }
        let mut inline_size = None;
        let terminal = match screen {
            PaneScreen::Alt => {
                if let Err(err) = execute!(std::io::stderr(), EnterAlternateScreen) {
                    restore_terminal(PaneScreen::Alt);
                    return Err(err);
                }
                // Task 0023: the alternate screen has no usable native
                // select-and-copy, so capture the terminal's own mouse
                // reporting to drive it ourselves. `restore_terminal`'s Alt
                // arm disables this unconditionally, so every rollback below
                // (and every later teardown path) already undoes it.
                if let Err(err) = execute!(std::io::stderr(), EnableMouseCapture) {
                    restore_terminal(PaneScreen::Alt);
                    return Err(err);
                }
                match Terminal::new(CrosstermBackend::new(std::io::stderr())) {
                    Ok(terminal) => terminal,
                    Err(err) => {
                        // Roll back every mode already entered before
                        // propagating — otherwise a `Terminal::new` failure
                        // leaves the caller's terminal stuck in
                        // raw/alternate-screen mode.
                        restore_terminal(PaneScreen::Alt);
                        return Err(err);
                    }
                }
            }
            PaneScreen::Inline => {
                let (columns, rows) = match crossterm::terminal::size() {
                    Ok(size) => size,
                    Err(err) => {
                        restore_terminal(PaneScreen::Inline);
                        return Err(err);
                    }
                };
                let terminal = match inline_terminal(rows) {
                    Ok(terminal) => terminal,
                    Err(err) => {
                        restore_terminal(PaneScreen::Inline);
                        return Err(err);
                    }
                };
                inline_size = Some((columns, rows));
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
            focused: AtomicBool::new(true),
            resize_size: AtomicU32::new(0),
            mouse_anchor: AtomicU32::new(0),
            mouse_current: AtomicU32::new(0),
            mouse_selecting: AtomicBool::new(false),
            mouse_up_pending: AtomicBool::new(false),
            mouse_dirty: AtomicBool::new(false),
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
                    // A timeout (`Ok(false)`) is the normal quiet tick. An
                    // ERROR is the input source itself being unreadable, and
                    // it does not heal: retrying it immediately is an
                    // unbounded busy loop that reads no keys and burns a core
                    // — which is exactly what a piped stdin produced, a live
                    // view that rendered and then ignored every keystroke at
                    // ~46% CPU. `adopt_controlling_terminal` should prevent
                    // ever getting here; if it could not, stop the pump
                    // instead of spinning on a source that will never answer.
                    match event::poll(Duration::from_millis(100)) {
                        Ok(true) => {}
                        Ok(false) => continue,
                        Err(_) => return,
                    }
                    if pump.stop.load(Ordering::SeqCst) || pump.paused.load(Ordering::SeqCst) {
                        continue;
                    }
                    let Ok(read) = event::read() else { return };
                    match read {
                        Event::Resize(columns, rows) => {
                            pump.resize_size
                                .store(pack_terminal_size(columns, rows), Ordering::SeqCst);
                            notify_input(&pump);
                        }
                        Event::FocusLost => set_focus(&pump, false),
                        Event::FocusGained => set_focus(&pump, true),
                        // Only reachable on an `Alt` pane — `EnableMouseCapture`
                        // is never sent for `Inline`, so no terminal reports
                        // mouse events there and this arm simply never matches.
                        Event::Mouse(mouse) => {
                            if !handle_mouse_event(&pump, &sender, mouse) {
                                return;
                            }
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
            inline_size,
            last_inline_resize: Instant::now(),
        })
    }

    /// P081: a pane that never touches the real terminal (no raw mode, no
    /// alternate-screen/cursor escapes) — for tests that need a `RunPanel`
    /// (e.g. exercising `apply_ledger_seed` through `RunPanel::new_observer`)
    /// without the side effects of a live `RatatuiPane`. Already `detached`,
    /// so every draw/poll call is the existing no-op path (`:631`, `:702`)
    /// rather than a special case.
    #[cfg(test)]
    pub(crate) fn new_detached_for_test() -> Self {
        let (_sender, keys) = mpsc::channel();
        Self {
            terminal: None,
            generation: 0,
            screen: PaneScreen::Alt,
            detached: true,
            keys,
            pump: Arc::new(PumpControl {
                stop: AtomicBool::new(true),
                paused: AtomicBool::new(false),
                focused: AtomicBool::new(true),
                resize_size: AtomicU32::new(0),
                mouse_anchor: AtomicU32::new(0),
                mouse_current: AtomicU32::new(0),
                mouse_selecting: AtomicBool::new(false),
                mouse_up_pending: AtomicBool::new(false),
                mouse_dirty: AtomicBool::new(false),
                input_generation: Arc::new(AtomicU64::new(0)),
                wake: Mutex::new(None),
                ctrl_c_policy: CtrlCPolicy::RequestStop,
            }),
            inline_size: None,
            last_inline_resize: Instant::now(),
        }
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
        let size = self.pump.resize_size.swap(0, Ordering::SeqCst);
        let (columns, rows) = unpack_terminal_size(size);
        if size == 0 || self.inline_size == Some((columns, rows)) {
            return false;
        }
        if self.last_inline_resize.elapsed() < INLINE_RESIZE_INTERVAL {
            self.requeue_resize(size);
            return false;
        }
        let Some(old_terminal) = self.terminal.as_mut() else {
            return false;
        };
        if old_terminal.clear().is_err() {
            self.requeue_resize(size);
            return false;
        }
        match inline_terminal(rows) {
            Ok(mut terminal) => {
                // A fresh terminal has blank diff buffers, so clear its
                // viewport before its first draw to erase any old-frame cells.
                if terminal.clear().is_err() {
                    self.requeue_resize(size);
                    return true;
                }
                self.terminal = Some(terminal);
                self.inline_size = Some((columns, rows));
                self.last_inline_resize = Instant::now();
                true
            }
            Err(_) => {
                // The old terminal was cleared above and remains owned here;
                // request its redraw and retry the resize on a later tick.
                self.requeue_resize(size);
                true
            }
        }
    }

    pub(crate) fn resize_pending(&self) -> bool {
        self.pump.resize_size.load(Ordering::SeqCst) != 0
    }

    /// Consumes the pump's mouse-activity signal, set by every handled
    /// Down/Drag/Up arm. A tick's `changed` computation must fold this in —
    /// mouse input wakes the tick (`notify_input` bumps the input
    /// generation) exactly like a key or resize does, but unlike those it
    /// left no trace `changed` recognised, so a drag rendered no highlight
    /// and a mouseup performed no copy until an unrelated key/focus/resize
    /// forced the next frame.
    pub(crate) fn take_mouse_changed(&self) -> bool {
        self.pump.mouse_dirty.swap(false, Ordering::SeqCst)
    }

    /// Whether the terminal window currently has focus, as last reported by
    /// the pump's `FocusGained`/`FocusLost` events. [`Self::draw`] reads this
    /// to dim an unfocused pane; callers read it to notice that the dim needs
    /// repainting at all, since a focus change alters what a frame should
    /// look like without changing any of its content.
    pub(crate) fn focused(&self) -> bool {
        self.pump.focused.load(Ordering::SeqCst)
    }

    fn requeue_resize(&self, size: u32) {
        // A newer event wins over a failed/debounced older size.
        let _ = self
            .pump
            .resize_size
            .compare_exchange(0, size, Ordering::SeqCst, Ordering::SeqCst);
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

    /// Startup owns the same inline terminal before a live panel exists. It
    /// needs to observe Ctrl-C without tearing the pane down itself so its
    /// owner can first commit the final startup rows to scrollback.
    pub(crate) fn poll_startup_interrupt(&mut self) -> bool {
        if self.detached() {
            return false;
        }
        while let Ok(key) = self.keys.try_recv() {
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                crate::app::interrupt::request_kill();
                return true;
            }
        }
        false
    }

    /// General-purpose draw entry point for the P423 dashboard's screen
    /// hierarchy: callers build arbitrary ratatui widgets against the given
    /// frame instead of being limited to [`Self::render`]'s single styled-line
    /// paragraph. Gated on `detached()` exactly like every other draw path so
    /// a panic caught elsewhere can never cause a later frame onto the
    /// restored normal screen.
    ///
    /// Task 0023: also the single hook every `Alt` surface (run view,
    /// dashboard, startup view, demo, trait editor) shares for mouse
    /// selection — mirroring how the unfocused-DIM overlay above already
    /// post-processes the frame buffer generically instead of every renderer
    /// doing its own thing. While a drag is live, the selected linear region
    /// is painted `REVERSED` after the widget closure runs. Once a mouseup
    /// lands, the frame after it (this same call, since drawing wakes on
    /// mouse input) extracts the selected text from the buffer this exact
    /// frame just drew, expands any ellipsized row through the truncation
    /// ledger, writes it to the clipboard via OSC 52, and clears the
    /// selection — so the highlight never lingers past the release that
    /// produced the copy.
    pub(crate) fn draw(
        &mut self,
        widget: impl FnOnce(&mut ratatui::Frame<'_>),
    ) -> std::io::Result<()> {
        if self.detached() {
            return Ok(());
        }
        let screen = self.screen;
        let Some(terminal) = self.terminal.as_mut() else {
            return Ok(());
        };
        let focused = &self.pump.focused;
        let pump = &self.pump;
        // Peek the pending mouseup here, before `terminal.draw` runs, rather
        // than only clearing `mouse_selecting` after it: the mouseup is
        // processed inside THIS call (below), so if the highlight were still
        // painted from `mouse_selecting` alone, this exact release frame
        // would render with the REVERSED overlay still on and only the NEXT
        // frame would clear it visually — leaving a stale highlight on
        // screen after the copy until an unrelated key/resize/focus event
        // forced a repaint.
        let selecting = screen == PaneScreen::Alt
            && pump.mouse_selecting.load(Ordering::SeqCst)
            && !pump.mouse_up_pending.load(Ordering::SeqCst);
        terminal.draw(|frame| {
            widget(frame);
            if !focused.load(Ordering::SeqCst) {
                let area = frame.area();
                // BOLD must come OFF, not merely gain DIM: terminals resolve
                // bold+faint in their own favor (bold usually wins), so the
                // focused pane's BOLD title and the active tab would hold
                // full lightness while everything around them dimmed. With
                // BOLD stripped the whole frame — borders, titles, content —
                // dims uniformly, and no renderer needs to know about window
                // focus; the next focused frame redraws from the widgets and
                // restores full styling.
                frame.buffer_mut().set_style(
                    area,
                    Style::default()
                        .add_modifier(Modifier::DIM)
                        .remove_modifier(Modifier::BOLD),
                );
            }
            if selecting {
                let area = frame.area();
                let anchor = unpack_terminal_size(pump.mouse_anchor.load(Ordering::SeqCst));
                let current = unpack_terminal_size(pump.mouse_current.load(Ordering::SeqCst));
                let buffer = frame.buffer_mut();
                for span in tui_select::linear_region(anchor, current, area) {
                    let rect =
                        Rect::new(span.start_col, span.row, span.end_col - span.start_col, 1);
                    buffer.set_style(rect, Style::default().add_modifier(Modifier::REVERSED));
                }
            }
        })?;
        if screen == PaneScreen::Alt && pump.mouse_up_pending.swap(false, Ordering::SeqCst) {
            pump.mouse_selecting.store(false, Ordering::SeqCst);
            let anchor = unpack_terminal_size(pump.mouse_anchor.load(Ordering::SeqCst));
            let current = unpack_terminal_size(pump.mouse_current.load(Ordering::SeqCst));
            let buffer = terminal.current_buffer_mut();
            let spans = tui_select::linear_region(anchor, current, buffer.area);
            let text = tui_select::substitute_ledger(&tui_select::extract_text(buffer, &spans));
            if !text.is_empty() {
                // Fire-and-forget, same stream the backend already owns: a
                // terminal that ignores OSC 52 (Terminal.app by default) or
                // a tmux without `set-clipboard on` just silently no-ops.
                let _ = write!(std::io::stderr(), "{}", tui_select::osc52_sequence(&text));
                let _ = std::io::stderr().flush();
            }
        }
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
                let focus_ok = raw_ok && execute!(std::io::stderr(), EnableFocusChange).is_ok();
                let screen_ok = focus_ok
                    && match self.screen {
                        PaneScreen::Alt => {
                            execute!(std::io::stderr(), EnterAlternateScreen, EnableMouseCapture)
                                .is_ok()
                        }
                        // Inline never left the alternate screen (it was never
                        // in one) — only raw mode needs re-entering.
                        PaneScreen::Inline => true,
                    };
                let reentry_ok = raw_ok && focus_ok && screen_ok;
                if !reentry_ok {
                    restore_terminal(self.screen);
                }
                self.reentry_ok.set(reentry_ok);
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
        let _ = execute!(std::io::stderr(), DisableFocusChange);
        disable_raw_mode()?;
        match screen {
            PaneScreen::Alt => {
                execute!(
                    std::io::stderr(),
                    DisableMouseCapture,
                    LeaveAlternateScreen,
                    Show
                )?;
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

fn set_focus(pump: &PumpControl, focused: bool) {
    if pump.focused.swap(focused, Ordering::SeqCst) != focused {
        notify_input(pump);
    }
}

/// The pump thread's `Event::Mouse` handling, extracted so it is callable
/// (and testable) without a real terminal. Returns `false` only when the key
/// channel has been dropped by the receiver — the same "give up the pump"
/// signal the resize/key arms already return on — so the caller can
/// propagate it into its own `return`.
fn handle_mouse_event(
    pump: &PumpControl,
    sender: &mpsc::Sender<KeyEvent>,
    mouse: crossterm::event::MouseEvent,
) -> bool {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let packed = pack_terminal_size(mouse.column, mouse.row);
            pump.mouse_anchor.store(packed, Ordering::SeqCst);
            pump.mouse_current.store(packed, Ordering::SeqCst);
            pump.mouse_up_pending.store(false, Ordering::SeqCst);
            pump.mouse_selecting.store(true, Ordering::SeqCst);
            pump.mouse_dirty.store(true, Ordering::SeqCst);
            notify_input(pump);
            true
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if pump.mouse_selecting.load(Ordering::SeqCst) {
                pump.mouse_current.store(
                    pack_terminal_size(mouse.column, mouse.row),
                    Ordering::SeqCst,
                );
                pump.mouse_dirty.store(true, Ordering::SeqCst);
                notify_input(pump);
            }
            true
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if pump.mouse_selecting.load(Ordering::SeqCst) {
                pump.mouse_current.store(
                    pack_terminal_size(mouse.column, mouse.row),
                    Ordering::SeqCst,
                );
                pump.mouse_up_pending.store(true, Ordering::SeqCst);
                pump.mouse_dirty.store(true, Ordering::SeqCst);
                notify_input(pump);
            }
            true
        }
        // Capture stops the terminal from emulating arrow keys for the
        // wheel in the alternate screen, so translate the wheel onto the
        // same key channel every scrollable surface already reads —
        // otherwise this feature regresses scrolling on terminals that
        // relied on that emulation.
        MouseEventKind::ScrollUp => {
            if sender
                .send(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE))
                .is_err()
            {
                return false;
            }
            notify_input(pump);
            true
        }
        MouseEventKind::ScrollDown => {
            if sender
                .send(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
                .is_err()
            {
                return false;
            }
            notify_input(pump);
            true
        }
        _ => true,
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
    fn full_restore_escape_disables_every_mode_the_pane_enables() {
        // Lockstep invariant with `new_with_options`/`restore_terminal`: for
        // every mode a pane can enable, the signal-safe rescue const must
        // carry the matching disable escape. Not a snapshot of the whole
        // byte string — a per-mode membership check, so appending an
        // unrelated escape (e.g. the trailing CR/LF) never breaks the test.
        let escape = std::str::from_utf8(FULL_RESTORE_ESCAPE).expect("ascii escapes");
        for mode in [
            "?1000l", "?1002l", "?1003l", "?1006l", "?1004l", "?1049l", "?25h",
        ] {
            assert!(
                escape.contains(mode),
                "FULL_RESTORE_ESCAPE missing {mode}: {escape:?}"
            );
        }
    }

    #[test]
    fn input_notification_advances_generation_and_wakes_after_queueing() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let control = PumpControl {
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            focused: AtomicBool::new(true),
            resize_size: AtomicU32::new(0),
            mouse_anchor: AtomicU32::new(0),
            mouse_current: AtomicU32::new(0),
            mouse_selecting: AtomicBool::new(false),
            mouse_up_pending: AtomicBool::new(false),
            mouse_dirty: AtomicBool::new(false),
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

    #[test]
    fn focus_transitions_default_to_focused_and_notify_only_on_change() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let control = PumpControl {
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            focused: AtomicBool::new(true),
            resize_size: AtomicU32::new(0),
            mouse_anchor: AtomicU32::new(0),
            mouse_current: AtomicU32::new(0),
            mouse_selecting: AtomicBool::new(false),
            mouse_up_pending: AtomicBool::new(false),
            mouse_dirty: AtomicBool::new(false),
            input_generation: Arc::new(AtomicU64::new(0)),
            wake: Mutex::new(Some({
                let wakes = Arc::clone(&wakes);
                Arc::new(move || {
                    wakes.fetch_add(1, Ordering::SeqCst);
                })
            })),
            ctrl_c_policy: CtrlCPolicy::RequestStop,
        };

        assert!(control.focused.load(Ordering::SeqCst));
        set_focus(&control, false);
        assert!(!control.focused.load(Ordering::SeqCst));
        set_focus(&control, false);
        set_focus(&control, true);
        assert!(control.focused.load(Ordering::SeqCst));
        assert_eq!(control.input_generation.load(Ordering::SeqCst), 2);
        assert_eq!(wakes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn inline_viewport_height_uses_every_terminal_row() {
        assert_eq!(inline_viewport_height(24), 24);
        assert_eq!(inline_viewport_height(6), 6);
        assert_eq!(inline_viewport_height(1), 1);
    }

    #[test]
    fn inline_viewport_height_handles_tiny_terminals() {
        assert_eq!(inline_viewport_height(0), 0);
        assert_eq!(inline_viewport_height(1), 1);
    }

    #[test]
    fn mouse_down_drag_up_sets_dirty_and_take_consumes_it_once() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let control = PumpControl {
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            focused: AtomicBool::new(true),
            resize_size: AtomicU32::new(0),
            mouse_anchor: AtomicU32::new(0),
            mouse_current: AtomicU32::new(0),
            mouse_selecting: AtomicBool::new(false),
            mouse_up_pending: AtomicBool::new(false),
            mouse_dirty: AtomicBool::new(false),
            input_generation: Arc::new(AtomicU64::new(0)),
            wake: Mutex::new(Some({
                let wakes = Arc::clone(&wakes);
                Arc::new(move || {
                    wakes.fetch_add(1, Ordering::SeqCst);
                })
            })),
            ctrl_c_policy: CtrlCPolicy::RequestStop,
        };
        let (sender, _keys) = mpsc::channel();

        // `take_mouse_changed` is not exposed on a bare `PumpControl` (it is
        // a `RatatuiPane` method), so this reads the same atomic it swaps.
        assert!(!control.mouse_dirty.swap(false, Ordering::SeqCst));

        let down = crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 2,
            modifiers: KeyModifiers::NONE,
        };
        assert!(handle_mouse_event(&control, &sender, down));
        assert!(control.mouse_selecting.load(Ordering::SeqCst));
        assert_eq!(
            control.mouse_anchor.load(Ordering::SeqCst),
            pack_terminal_size(5, 2)
        );
        // Consuming here proves the signal survives exactly the one Down
        // event that set it, then is gone until the next handled arm.
        assert!(control.mouse_dirty.swap(false, Ordering::SeqCst));
        assert!(!control.mouse_dirty.swap(false, Ordering::SeqCst));

        let drag = crossterm::event::MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 9,
            row: 4,
            modifiers: KeyModifiers::NONE,
        };
        assert!(handle_mouse_event(&control, &sender, drag));
        assert_eq!(
            control.mouse_current.load(Ordering::SeqCst),
            pack_terminal_size(9, 4)
        );
        assert!(control.mouse_dirty.swap(false, Ordering::SeqCst));

        let up = crossterm::event::MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 9,
            row: 4,
            modifiers: KeyModifiers::NONE,
        };
        assert!(handle_mouse_event(&control, &sender, up));
        // `draw()`, not the pump, clears `mouse_selecting` once it has
        // consumed the pending mouseup — still true here.
        assert!(control.mouse_selecting.load(Ordering::SeqCst));
        assert!(control.mouse_up_pending.load(Ordering::SeqCst));
        assert!(control.mouse_dirty.swap(false, Ordering::SeqCst));
        assert!(!control.mouse_dirty.swap(false, Ordering::SeqCst));

        assert_eq!(control.input_generation.load(Ordering::SeqCst), 3);
        assert_eq!(wakes.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn packed_resize_tracks_width_and_height_independently() {
        let current = pack_terminal_size(120, 24);
        assert_ne!(current, pack_terminal_size(80, 24));
        assert_ne!(current, pack_terminal_size(120, 40));
        assert_eq!(unpack_terminal_size(current), (120, 24));
    }
}
