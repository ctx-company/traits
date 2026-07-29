//! Process-wide graceful-`SIGINT` flag for the P402 durable drive loop.
//!
//! A first `SIGINT` sets this flag; `drive()`'s wave loop checks it between
//! (never inside) bounded operations — it never interrupts an in-flight
//! harness call, it only stops the loop from *starting* a new reservation or
//! wave once the flag is observed. This is the only interruption path this
//! module makes any guarantee about: a hard kill (`SIGKILL`) or crash mid
//! provider-call is explicitly out of scope (see `run_branch::BranchSidecar`
//! status `Interrupted` vs an unresolvable `Running` sidecar left behind by a
//! hard kill — the latter is never silently treated as complete or safe to
//! redispatch).
//!
//! Uses the `libc` dependency already present in this workspace (no new
//! crate): a signal handler may only touch async-signal-safe state, so it
//! does nothing but store `true` into an `AtomicBool`.

use std::sync::Once;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
/// `SIGINT` deliveries observed since install (or the last [`reset`]) —
/// drives the escalation ladder in [`handle_sigint`]. Only actual signals
/// count; [`request_stop`] stays a pure graceful request.
static SIGINT_COUNT: AtomicU64 = AtomicU64::new(0);
static INSTALL_ONCE: Once = Once::new();

/// Cooked-mode terminal attributes captured once in [`install`], BEFORE the
/// handler is registered (so the handler never races the write). Used by the
/// escalation ladder to hand the user a working terminal even when the exit
/// happens while a ratatui pane holds raw mode: `tcsetattr` and `write` are
/// both async-signal-safe, unlike anything crossterm offers.
static mut SAVED_TERMIOS: std::mem::MaybeUninit<libc::termios> = std::mem::MaybeUninit::uninit();
static TERMIOS_SAVED: AtomicBool = AtomicBool::new(false);

/// Install the process-wide `SIGINT` handler, if not already installed.
/// Idempotent and cheap to call at the top of every drive invocation.
pub fn install() {
    INSTALL_ONCE.call_once(|| {
        // Capture the pre-TUI cooked-mode attributes before the handler can
        // ever run. Best-effort: headless invocations (proofs, CI) have no
        // tty and simply skip the restore half of escalation.
        // SAFETY: single write to the static, strictly before the handler
        // that reads it is registered.
        unsafe {
            let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
            if libc::tcgetattr(libc::STDIN_FILENO, termios.as_mut_ptr()) == 0 {
                SAVED_TERMIOS = termios;
                TERMIOS_SAVED.store(true, Ordering::SeqCst);
            }
        }
        // SAFETY: `signal(2)` with a plain `extern "C" fn(i32)` handler that
        // only performs async-signal-safe work (atomic stores, `write`,
        // `tcsetattr`, `_exit`) is a standard, sound use of this API.
        unsafe {
            libc::signal(
                libc::SIGINT,
                handle_sigint as *const () as libc::sighandler_t,
            );
        }
    });
}

/// Best-effort terminal rescue from inside the signal handler: leave the
/// alternate screen, show the cursor, and restore the cooked-mode attributes
/// captured at install. Every step is async-signal-safe; every step
/// tolerates a terminal that was never in raw/alternate mode.
fn rescue_terminal_signal_safe() {
    const LEAVE: &[u8] = b"\x1b[?1049l\x1b[?25h\r\n";
    // SAFETY: `write` on a borrowed fd with a static buffer, and `tcsetattr`
    // with attributes fully initialized before the handler was registered
    // (guarded by TERMIOS_SAVED).
    unsafe {
        let _ = libc::write(libc::STDERR_FILENO, LEAVE.as_ptr().cast(), LEAVE.len());
        if TERMIOS_SAVED.load(Ordering::SeqCst) {
            let saved = std::ptr::addr_of!(SAVED_TERMIOS);
            let _ = libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, (*saved).as_ptr());
        }
    }
}

/// Escalation ladder: the 1st `SIGINT` requests the graceful between-frames
/// stop (unchanged P402 semantics); the 2nd additionally rescues the
/// terminal out of raw/alternate mode so the user is never stranded behind a
/// pane while the drain finishes; the 3rd gives up on draining and exits
/// with the conventional 130, after the same terminal rescue.
extern "C" fn handle_sigint(_signal: i32) {
    INTERRUPTED.store(true, Ordering::SeqCst);
    let count = SIGINT_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    if count >= 2 {
        rescue_terminal_signal_safe();
    }
    if count >= 3 {
        // SAFETY: `_exit` is async-signal-safe; 130 = 128 + SIGINT.
        unsafe { libc::_exit(130) };
    }
}

/// `true` once a `SIGINT` has been observed since the process started (or
/// since the last [`reset`]).
pub fn is_interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// Clear the flag (and the escalation count). Only used by the drive loop
/// after it has finished recording an interrupted outcome and returned
/// control to the caller, so a long-lived host process (tests, a future
/// daemon mode) can drive again without carrying a stale interrupt forward.
pub fn reset() {
    INTERRUPTED.store(false, Ordering::SeqCst);
    SIGINT_COUNT.store(0, Ordering::SeqCst);
    ctx_traits_io::run_kill::reset();
}

/// Set the same flag [`handle_sigint`] sets, but from a same-process
/// callback rather than an actual signal — the target of
/// `ctx_traits_io::run_control::try_acquire`'s `on_interrupt` callback for
/// every drive registered under the P423 driver control lock. A request
/// arriving through the authenticated control socket and a real `SIGINT` are
/// treated identically by the drive loop from this point on.
pub fn request_stop() {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// P551: the live TUI's ctrl-c meaning — an instant kill of the active
/// harness/setup child, not the cooperative between-frames stop a real
/// `SIGINT` requests. Sets [`INTERRUPTED`] too (every existing checkpoint
/// still trips) and additionally marks `ctx_traits_io::run_kill`'s killed
/// flag and signals the registered process group with `SIGKILL`. Only the
/// raw-mode key path (ratatui pump / `poll_detach`) ever calls this —
/// headless drives keep the cooperative-then-escalate ladder untouched.
pub fn request_kill() {
    INTERRUPTED.store(true, Ordering::SeqCst);
    ctx_traits_io::run_kill::request_kill();
}
