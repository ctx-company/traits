//! Stable-inode `flock(2)` primitives shared by [`crate::merge_lock`]
//! (bounded-wait, contention-visible cross-process serialization for
//! `ctx traits merge`) and [`crate::builtin_store`] (guarding built-in trait
//! store publication). Extracted so both keep exactly one flock/pid-liveness
//! implementation instead of drifting copies.

use camino::Utf8Path;
use std::io::{self, Read, Seek, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;

/// Open (creating if absent) a lock file for `flock`-based locking, refusing
/// to follow a symlink at the leaf: the kernel `O_NOFOLLOW` flag rejects the
/// open atomically if the path is a symlink, so there is no separate
/// check-then-open race window. Shared by [`crate::builtin_store`]'s
/// publish lock and [`crate::merge_lock`]'s merge lock so both open their
/// lock file the same safe way.
pub fn open_lock_file_no_follow(path: &Utf8Path) -> io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path.as_std_path())
}

/// Nonblocking exclusive `flock`. `Ok(true)` acquired, `Ok(false)` contended.
pub fn try_lock_exclusive(file: &std::fs::File) -> io::Result<bool> {
    // SAFETY: `file`'s raw fd is valid for the duration of this call; `flock`
    // only mutates kernel lock state associated with the fd, no memory is
    // touched through the pointer-free syscall.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::WouldBlock {
        return Ok(false);
    }
    Err(error)
}

/// Blocking exclusive `flock`: waits in-kernel until acquired, with no
/// userspace poll loop. Used where the expected hold time is short and a
/// caller has no bounded-wait/contention-reporting requirement of its own
/// (built-in trait store publication) — contrast [`crate::merge_lock::acquire`],
/// which needs [`try_lock_exclusive`] directly so it can poll, report queued
/// holders, and honor a caller-supplied timeout.
pub fn lock_exclusive_blocking(file: &std::fs::File) -> io::Result<()> {
    // SAFETY: same as `try_lock_exclusive`; blocking `flock` without `LOCK_NB`
    // only differs in that the kernel parks the calling thread instead of
    // returning `EWOULDBLOCK` immediately.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if result == 0 {
        return Ok(());
    }
    Err(io::Error::last_os_error())
}

/// Try to acquire the single conductor lease for a P402 durable
/// CLI/IO-supervised run (one lease file per parent run, under that run's
/// `.branches` sidecar directory). `Ok(Some(file))` means this process is now
/// the sole parent-ledger writer for that run: hold the returned file for as
/// long as the lease must be held — the kernel `flock` it carries releases
/// automatically when the file (and with it, the last dup'd fd) is dropped or
/// the process exits, so a crashed conductor never leaves a stale held lease
/// behind. `Ok(None)` means a live conductor already holds it (a caller that
/// opted into `--wait` polls this on a bounded interval instead of treating a
/// miss as fatal). Creates parent directories as needed so callers don't have
/// to pre-create the `.branches` tree before the first lease attempt.
pub fn try_acquire_conductor_lease(path: &Utf8Path) -> io::Result<Option<std::fs::File>> {
    if let Some(parent) = path.parent() {
        if !parent.as_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let file = open_lock_file_no_follow(path)?;
    if try_lock_exclusive(&file)? {
        Ok(Some(file))
    } else {
        Ok(None)
    }
}

/// `true` if a process with `pid` currently exists, via `kill(pid, 0)`
/// liveness semantics (no signal delivered). Only a confirmed `ESRCH` ("no
/// such process") is treated as dead; Rust's portable `ErrorKind` mapping
/// loses this distinction (`ESRCH` maps to `Uncategorized`, not `NotFound`,
/// on this platform), so the raw errno is checked directly. Every other
/// outcome — success, `EPERM`, or any other error — is treated as alive, so
/// a permission-denied or transient error never misclassifies a live holder
/// as stale.
pub fn pid_is_alive(pid: u32) -> bool {
    // SAFETY: signal 0 delivers no signal and only probes process existence;
    // `pid` is a plain value with no aliasing/memory-safety implications.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if result == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// Read and deserialize best-effort JSON holder metadata previously written
/// by [`write_lock_metadata`]. Returns `None` on any I/O error, empty
/// content, or malformed JSON — callers must never treat a `None` here as
/// proof the lock is unheld; only the kernel `flock` state (contended vs.
/// acquired) is ownership authority. Shared by every lock module that keeps
/// a small serialized holder record beside its `flock`.
pub fn read_lock_metadata<T: serde::de::DeserializeOwned>(file: &mut std::fs::File) -> Option<T> {
    let mut text = String::new();
    file.rewind().ok()?;
    file.read_to_string(&mut text).ok()?;
    if text.trim().is_empty() {
        return None;
    }
    serde_json::from_str(&text).ok()
}

/// Serialize and write best-effort JSON holder metadata while the caller
/// holds the file's `flock`. The metadata is display/probe evidence only —
/// never the ownership mechanism.
pub fn write_lock_metadata<T: serde::Serialize>(
    file: &mut std::fs::File,
    metadata: &T,
) -> io::Result<()> {
    let text = serde_json::to_string(metadata).unwrap_or_default();
    file.rewind()?;
    file.set_len(0)?;
    file.write_all(text.as_bytes())?;
    file.flush()
}

/// Clear any holder metadata while the caller still holds the file's
/// `flock`, leaving the lock file itself in place so every future contender
/// flocks the same stable inode.
pub fn clear_lock_metadata(file: &mut std::fs::File) -> io::Result<()> {
    file.rewind()?;
    file.set_len(0)
}
