//! Process-group registry backing P551's instant ctrl-c kill.
//!
//! Harness and worktree-setup children are spawned into their own process
//! group (`process_group(0)`, Unix-only) so [`kill_active_process_group`]
//! can signal the whole group — not just the immediate child, which would
//! leave any of its own children (a shell wrapper, a package-manager
//! subprocess) running after the parent is gone. Only one such child is
//! ever active on the drive thread at a time, so a single slot is enough;
//! [`register`]/[`clear`] are keyed on the pgid itself so a stale `clear`
//! racing a newer `register` can never clobber the live registration.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

static ACTIVE_PGID: AtomicI32 = AtomicI32::new(0);
/// Set by [`request_kill`], observed by the harness wait loops in this crate
/// so a `SIGKILL`ed child is classified as deliberately killed rather than a
/// generic harness failure. Cleared by [`reset`].
static KILLED: AtomicBool = AtomicBool::new(false);

/// Registers `pgid` (the leader pid of a child spawned with
/// `process_group(0)`) as the group to signal on a kill request.
pub fn register(pgid: i32) {
    ACTIVE_PGID.store(pgid, Ordering::SeqCst);
}

/// Clears `pgid`'s registration once its child has been waited on. A no-op
/// if a different pgid is currently registered (a newer child already
/// replaced this one).
pub fn clear(pgid: i32) {
    let _ = ACTIVE_PGID.compare_exchange(pgid, 0, Ordering::SeqCst, Ordering::SeqCst);
}

/// Sends `SIGKILL` to the currently registered process group, if any.
/// Called from the pump thread on the ctrl-c key path, and — as of 0024 —
/// also from the CLI's `SIGTERM`/`SIGHUP` signal handlers
/// (`interrupt::handle_sigterm`). Both call sites are safe: the body is
/// nothing but an atomic load and a single `kill(2)`, no allocation and no
/// locks, so it is async-signal-safe as written and needs no change to be
/// called from a handler.
#[cfg(unix)]
pub fn kill_active_process_group() {
    let pgid = ACTIVE_PGID.load(Ordering::SeqCst);
    if pgid > 0 {
        // SAFETY: `kill(2)` with a negative pid signals the process group;
        // a group that has already exited simply yields ESRCH, ignored here.
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
pub fn kill_active_process_group() {}

/// Marks the kill flag and signals the active process group. The single
/// entry point the CLI's `interrupt::request_kill()` calls.
pub fn request_kill() {
    KILLED.store(true, Ordering::SeqCst);
    kill_active_process_group();
}

/// `true` once [`request_kill`] has fired since the last [`reset`].
pub fn was_killed() -> bool {
    KILLED.load(Ordering::SeqCst)
}

/// Clears the killed flag (and the pgid registration) for a fresh drive.
pub fn reset() {
    KILLED.store(false, Ordering::SeqCst);
    ACTIVE_PGID.store(0, Ordering::SeqCst);
}
