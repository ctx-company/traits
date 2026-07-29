//! Per-ledger driver control lock (P423).
//!
//! Every `drive` invocation that touches a run-session ledger holds a
//! sibling `flock(2)` lock file for the duration of that drive, following
//! exactly the [`crate::file_lock`]/[`crate::merge_lock`] pattern: the
//! kernel lock is the sole authority on "is a driver currently attached to
//! this ledger", and holder metadata (pid/session/run id/start time) is
//! best-effort operational evidence for display only. [`probe`] is strictly
//! read-only: a crashed holder's leftover metadata is reported as
//! stale but never cleared by a probe — only the next genuine [`try_acquire`]
//! overwrites it, via the truncate-and-write [`crate::file_lock::write_lock_metadata`]
//! already does on every acquire, so nothing is lost and nothing is newly
//! trusted by leaving it in place.
//!
//! This module never inspects or mutates ledger/session content; it only
//! answers "is a driver attached, and if so, ask it to stop." Cooperative
//! interruption reuses the existing `SIGINT` semantics that
//! `crate::app::interrupt`/`drive` already honor (see P402) — this module
//! never sends `SIGKILL` and never introduces a permanent canceled status.
//!
//! Ownership vs. display evidence is never conflated: a *contended* `flock`
//! always classifies as [`DriverProbe::Held`], even when the holder's
//! metadata is missing, malformed, or stale (e.g. read during a new
//! holder's acquire-to-write window) — only a genuinely *uncontended* lock
//! is `Unheld`. Holder metadata (pid/session/run id/start time) is display
//! evidence ONLY and is never used to authorize [`request_interrupt`]: a pid
//! read from a file can be stale, forged, or reused by an unrelated process
//! between the moment it is read and the moment it would be signaled, and no
//! number of re-checks around that read closes the window structurally.
//!
//! Interruption is instead authenticated by a Unix domain control socket
//! bound by [`try_acquire`] only while the caller genuinely holds the driver
//! `flock`, and removed (its listener thread stopped) before the flock is
//! released. [`request_interrupt`] never reads or signals a pid at all: it
//! only attempts to connect to that socket. A successful connect-and-ack
//! round trip is itself the proof that a live process — the process that is
//! the current flock holder at the moment it accepted the connection — is
//! listening, because only that holder ever binds the socket, and it is
//! removed before the holder's lock releases. A missing or refused socket
//! (crashed holder, handed-off lock, no holder at all) can never be
//! connected to, so it can never be mistaken for a live target.

use camino::{Utf8Path, Utf8PathBuf};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How often the control-socket accept loop wakes to check whether it has
/// been asked to stop (on [`DriverLockGuard`] drop). Bounds both the delay
/// before a released lock's socket disappears and the CPU cost of the idle
/// poll.
const CONTROL_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Bound on how long the accept thread will block reading/writing a single
/// accepted connection. An idle or malicious client that never sends its
/// byte, or never reads the ack, must never be able to wedge the listener
/// thread (and, transitively, [`DriverLockGuard`] teardown) forever.
const CONTROL_STREAM_TIMEOUT: Duration = Duration::from_secs(2);

/// Bound on how long [`request_interrupt`]'s connect attempt is allowed to
/// take before giving up on a socket that accepted the connection but never
/// makes progress.
const CONTROL_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Exact acknowledgement bytes a live holder writes back once it has invoked
/// `on_interrupt`. [`request_interrupt`] treats anything else — including a
/// partial or missing read — as "not confirmed".
const CONTROL_ACK: &[u8] = b"ok";

/// Best-effort holder metadata written while a driver lock is held. Never
/// canonical/ledger state — only a display/probe aid. `control_token` also
/// doubles as the unguessable component of the control-socket path (see
/// [`control_socket_path`]) — a co-located process needs read access to this
/// metadata (the same access level already required to read the ledger
/// itself) to learn where to connect, rather than being able to compute the
/// path from the ledger path alone.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DriverHolder {
    pub pid: u32,
    pub session_id: String,
    pub run_id: String,
    pub started_at_epoch_secs: u64,
    #[serde(default)]
    pub control_token: String,
}

/// RAII guard for a held driver lock: stops and removes the control-socket
/// listener, removes this session's liveness-index row, then clears
/// holder metadata (still under lock) on drop, covering every exit path —
/// success, error, panic unwind — since `Drop` always runs. The lock file
/// itself is left in place so every future contender flocks the same stable
/// inode.
pub struct DriverLockGuard {
    file: std::fs::File,
    control: Option<ControlListener>,
    /// `None` when this acquisition never had liveness facts to index (not
    /// expected in production — every `try_acquire` caller supplies them —
    /// but tests and any future bare caller must still drop cleanly).
    session_id: Option<String>,
}

/// Exclusive maintenance ownership of a driver's stable lock inode. Unlike a
/// driver guard this writes no holder metadata and exposes no control socket:
/// callers use it only to prove that no driver can appear while they repair or
/// remove the ledger's associated artifacts.
pub struct MaintenanceLockGuard {
    file: std::fs::File,
}

impl MaintenanceLockGuard {
    /// Clear stale display metadata while this guard still excludes a driver.
    pub fn clear_stale_metadata(&mut self) -> crate::Result<()> {
        crate::file_lock::clear_lock_metadata(&mut self.file).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: "driver lock metadata".to_string(),
                source,
            }
            .into()
        })
    }
}

impl Drop for DriverLockGuard {
    fn drop(&mut self) {
        // Dropped explicitly (and first) so the socket is gone and the
        // accept thread has exited before the flock itself is released
        // below — a request racing this teardown sees a refused/missing
        // socket rather than a connection into a listener whose driver lock
        // is about to disappear out from under it.
        self.control = None;
        // Best-effort, like every other liveness-index write: a removal
        // failure must never fail (or even warn on) a clean driver exit —
        // the row it would leave behind is exactly the orphan case the
        // index already has to handle.
        if let Some(session_id) = self.session_id.as_deref() {
            let _ = crate::run_liveness::remove_row(&runtime_root(), session_id);
        }
        let _ = crate::file_lock::clear_lock_metadata(&mut self.file);
    }
}

/// A control-socket listener bound only while its owning [`DriverLockGuard`]
/// holds the driver `flock`. Its accept loop runs on a dedicated thread and
/// invokes the caller-supplied `on_interrupt` callback for every connection
/// (each connection is itself already an authenticated interrupt request —
/// see the module doc — so no message content needs parsing).
struct ControlListener {
    socket_path: Utf8PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for ControlListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(self.socket_path.as_std_path());
    }
}

/// Ensure the per-user runtime root exists, is a real directory (not a
/// symlink an unrelated party could have planted), and is owned by the
/// current user with no group/other access — refusing to reuse anything
/// else rather than binding sockets (or writing the liveness index)
/// under an attacker-controlled directory.
pub fn ensure_runtime_root(root: &Utf8Path) -> crate::Result<()> {
    match std::fs::symlink_metadata(root.as_std_path()) {
        Ok(meta) => {
            let current_uid = unsafe { libc::getuid() };
            if meta.file_type().is_symlink()
                || !meta.is_dir()
                || meta.uid() != current_uid
                || meta.mode() & 0o777 != 0o700
            {
                return Err(crate::environment::Error::Filesystem {
                    path: root.to_string(),
                    source: std::io::Error::other(
                        "control root exists but is not a user-owned 0700 directory",
                    ),
                }
                .into());
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(root.as_std_path())
                .map_err(|e| crate::environment::Error::Filesystem {
                    path: root.to_string(),
                    source: e,
                })?;
            Ok(())
        }
        Err(e) => Err(crate::environment::Error::Filesystem {
            path: root.to_string(),
            source: e,
        }
        .into()),
    }
}

fn bind_control_listener(
    socket_path: Utf8PathBuf,
    on_interrupt: Arc<dyn Fn() + Send + Sync>,
) -> crate::Result<ControlListener> {
    if let Some(parent) = socket_path.parent() {
        ensure_runtime_root(parent)?;
    }
    // The socket filename embeds this acquisition's unguessable token (see
    // `DriverHolder::control_token`), so a collision here would require
    // another process to have independently produced the same random token
    // for the same ledger — astronomically unlikely, not a race this code
    // needs to resolve by unlinking a peer's live socket.
    let listener = UnixListener::bind(socket_path.as_std_path()).map_err(|e| {
        crate::environment::Error::Filesystem {
            path: socket_path.to_string(),
            source: e,
        }
    })?;
    listener
        .set_nonblocking(true)
        .map_err(|e| crate::environment::Error::Filesystem {
            path: socket_path.to_string(),
            source: e,
        })?;
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread = std::thread::spawn(move || {
        loop {
            if thread_stop.load(Ordering::SeqCst) {
                return;
            }
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(CONTROL_STREAM_TIMEOUT));
                    let _ = stream.set_write_timeout(Some(CONTROL_STREAM_TIMEOUT));
                    let mut byte = [0u8; 1];
                    // Content is irrelevant: the connection itself is the
                    // authenticated request (see module doc). A read/write
                    // that never completes is bounded by the timeouts above
                    // rather than blocking this thread indefinitely; an idle
                    // or malicious client can only cost this thread up to
                    // `CONTROL_STREAM_TIMEOUT`, never wedge it forever.
                    let _ = stream.read(&mut byte);
                    on_interrupt();
                    let _ = stream.write_all(CONTROL_ACK);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(CONTROL_POLL_INTERVAL);
                }
                Err(_) => {
                    std::thread::sleep(CONTROL_POLL_INTERVAL);
                }
            }
        }
    });
    Ok(ControlListener {
        socket_path,
        stop,
        thread: Some(thread),
    })
}

/// Outcome of probing a ledger's driver lock without attempting to hold it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverProbe {
    /// No driver currently holds the lock (the kernel `flock` itself was
    /// acquired uncontended). `stale_metadata` carries any leftover holder
    /// metadata from a crashed driver that never cleared it — reported for
    /// display, never treated as a live attachment.
    Unheld {
        stale_metadata: Option<DriverHolder>,
    },
    /// A driver currently holds the lock (the kernel `flock` is contended).
    /// `metadata` is `None` when the holder's record is missing, malformed,
    /// or was read mid-write — the lock is still authoritatively held even
    /// though nothing readable identifies who holds it.
    Held(Option<DriverHolder>),
}

/// Path of the driver lock sibling to a run-session ledger file, e.g.
/// `<session>.json` -> `<session>.json.driver-lock`. Kept next to the
/// ledger (rather than under the global runs root) so it travels with
/// whichever store — global default or legacy fallback — actually holds
/// the ledger.
pub fn driver_lock_path(ledger_path: &Utf8Path) -> Utf8PathBuf {
    let mut path = ledger_path.as_str().to_string();
    path.push_str(".driver-lock");
    Utf8PathBuf::from(path)
}

/// Per-user root for control sockets and the local liveness index
/// (`live-index.toml`, see [`crate::run_liveness`]). `AF_UNIX` paths are
/// hard-capped (`SUN_LEN`, ~104-108 bytes), so this stays under the
/// always-short `/tmp` rather than `$TMPDIR` (unbounded, and deliberately
/// redirected under a deep hermetic-test home directory by some callers).
/// Scoped per-uid (rather than one shared directory) and locked to `0700` in
/// [`ensure_runtime_root`] so an unrelated local user can neither list nor
/// plant a file at a path this user's driver would bind or write. This
/// directory is on LOCAL disk and is cleared by a reboot — the defining
/// property the liveness index depends on: `flock` semantics are reliable
/// here (unlike NFS), and a reboot clearing it means liveness can never
/// survive past the processes it described.
pub fn runtime_root() -> Utf8PathBuf {
    let uid = unsafe { libc::getuid() };
    Utf8PathBuf::from(format!("/tmp/ctx-driver-control-{uid}"))
}

/// Path of the driver control socket for one specific acquisition, keyed by
/// both the ledger path and that acquisition's unguessable `control_token`
/// (see [`DriverHolder`]). Deliberately NOT derivable from the ledger path
/// alone: a socket path predictable from public information (the ledger
/// path) would let any co-located process — not just one with legitimate
/// read access to this ledger's lock metadata — discover and connect to it.
pub fn control_socket_path(ledger_path: &Utf8Path, control_token: &str) -> Utf8PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    ledger_path.as_str().hash(&mut hasher);
    let digest = hasher.finish();
    runtime_root().join(format!("{digest:016x}-{control_token}.sock"))
}

/// A process-local, best-effort-unguessable token for one lock acquisition.
/// Not cryptographic key material — it only needs to be infeasible for an
/// unrelated process to blindly guess, since actually reading it requires
/// the same filesystem read access already needed to read the ledger/lock
/// metadata it travels alongside.
fn random_token() -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::process::id().hash(&mut hasher);
    if let Ok(duration) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        duration.as_nanos().hash(&mut hasher);
    }
    // A stack address is ASLR-randomized per process and unrelated to the
    // hasher's own internal state, so folding it in adds entropy the
    // pid+timestamp alone would not have.
    let stack_addr = &hasher as *const _ as usize;
    stack_addr.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Acquire the driver lock for a ledger about to be driven. `Ok(Some(guard))`
/// means this process is now the sole driver of that ledger for as long as
/// the guard is held; `Ok(None)` means another driver already holds it (a
/// caller sizing `--wait` polls this on its own bounded interval rather than
/// treating a miss as fatal). `on_interrupt` is invoked (on a background
/// thread) once per authenticated [`request_interrupt`] connection for as
/// long as the guard lives; callers pass a closure that sets their own
/// cooperative-stop flag (e.g. `crate::app::interrupt::request_stop` in the
/// CLI crate — this module never depends on that flag directly).
///
/// `facts` bundles every liveness-index fact this acquisition carries
/// (see [`crate::run_liveness::LiveRunFacts`]) — the same bundling precedent
/// as `DriveTerminalEvidence`, taken here because this parameter list was
/// already at four. Once the lock is genuinely acquired, `facts` is upserted
/// into the local liveness index immediately, before the guard is returned,
/// so a row and a held lock can never diverge; a write failure is logged
/// into `warnings` by the caller (this module never fails a drive over an
/// index write) rather than surfaced as an error here.
pub fn try_acquire(
    facts: &crate::run_liveness::LiveRunFacts,
    on_interrupt: Arc<dyn Fn() + Send + Sync>,
) -> crate::Result<Option<DriverLockGuard>> {
    let ledger_path = facts.ledger_path.as_path();
    let session_id = facts.session_id.as_str();
    let run_id = facts.run_id.as_str();
    let lock_path = driver_lock_path(ledger_path);
    if let Some(parent) = lock_path.parent()
        && !parent.as_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| crate::environment::Error::Filesystem {
            path: parent.to_string(),
            source: e,
        })?;
    }
    let mut file = crate::file_lock::open_lock_file_no_follow(&lock_path).map_err(|e| {
        crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source: e,
        }
    })?;
    if !crate::file_lock::try_lock_exclusive(&file).map_err(|e| {
        crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source: e,
        }
    })? {
        return Ok(None);
    }
    let control_token = random_token();
    let holder = DriverHolder {
        pid: std::process::id(),
        session_id: session_id.to_string(),
        run_id: run_id.to_string(),
        started_at_epoch_secs: epoch_secs(),
        control_token: control_token.clone(),
    };
    crate::file_lock::write_lock_metadata(&mut file, &holder).map_err(|e| {
        crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source: e,
        }
    })?;
    // The ledger flock remains authoritative even when the disposable local
    // control root cannot be used. A driver must keep progressing in that
    // case; only machine-local interrupt and liveness visibility are absent.
    let control = bind_control_listener(
        control_socket_path(ledger_path, &control_token),
        on_interrupt,
    )
    .ok();
    let pid = std::process::id();
    let started_at_epoch = epoch_secs();
    // Best-effort, matching every other liveness-index write policy: an
    // index write failure must never fail this drive. The caller sees no
    // signal from this — the same as `bind_control_listener` above would if
    // it, too, were made best-effort — because a driver that cannot be
    // indexed can still correctly drive; it is only machine-wide visibility
    // that degrades.
    let _ = crate::run_liveness::upsert_row(&runtime_root(), facts, pid, started_at_epoch);
    Ok(Some(DriverLockGuard {
        file,
        control,
        session_id: Some(session_id.to_string()),
    }))
}

/// Acquire exclusive maintenance ownership of `ledger_path`'s driver lock.
/// `Ok(None)` is authoritative evidence that a driver holds it; errors remain
/// distinct so destructive callers can fail closed on an unprobeable lock.
pub fn try_acquire_maintenance(
    ledger_path: &Utf8Path,
) -> crate::Result<Option<MaintenanceLockGuard>> {
    let lock_path = driver_lock_path(ledger_path);
    if let Some(parent) = lock_path.parent()
        && !parent.as_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: parent.to_string(),
                source,
            }
        })?;
    }
    let file = crate::file_lock::open_lock_file_no_follow(&lock_path).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source,
        }
    })?;
    if !crate::file_lock::try_lock_exclusive(&file).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source,
        }
    })? {
        return Ok(None);
    }
    Ok(Some(MaintenanceLockGuard { file }))
}

/// Probe a ledger's driver lock without holding it: used by the dashboard's
/// SESSIONS scan and [`crate::run_liveness`]'s liveness sweep to classify a
/// run as actively driven, stopped/resumable, or terminal, without
/// disturbing an active driver's lock. Strictly read-only: never
/// writes to the lock file, so probing 260 ledgers on a 2s tick costs 260
/// reads, never 260 writes.
pub fn probe(ledger_path: &Utf8Path) -> crate::Result<DriverProbe> {
    let lock_path = driver_lock_path(ledger_path);
    if !lock_path.is_file() {
        return Ok(DriverProbe::Unheld {
            stale_metadata: None,
        });
    }
    let mut file = crate::file_lock::open_lock_file_no_follow(&lock_path).map_err(|e| {
        crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source: e,
        }
    })?;
    let acquired = crate::file_lock::try_lock_exclusive(&file).map_err(|e| {
        crate::environment::Error::Filesystem {
            path: lock_path.to_string(),
            source: e,
        }
    })?;
    if !acquired {
        // Contended: some other process holds the lock right now. The kernel
        // `flock` state alone is ownership authority here — whatever the
        // metadata read turns up (complete, missing, malformed, or a
        // mid-write partial write from a holder that just acquired) never
        // downgrades this to `Unheld`.
        let holder = crate::file_lock::read_lock_metadata(&mut file);
        return Ok(DriverProbe::Held(holder));
    }
    // We just acquired it ourselves, so nobody else holds it. Any metadata
    // present is leftover from a crashed driver that never cleared it on
    // drop — report it as stale evidence and release immediately: probing
    // must never hold the lock past this call. Probing is
    // strictly read-only and never mutates the lock file — a crashed
    // holder's stale metadata is left in place for `try_acquire`'s
    // truncate-and-write to overwrite the next time this ledger is driven,
    // exactly like `write_lock_metadata` already does on every acquire.
    // Clearing it here made an uncontended probe indistinguishable from a
    // write, and the dashboard's every-2s SESSIONS scan probed (and thus
    // flock+truncated) every ledger's lock file just to draw a list.
    let stale_metadata = crate::file_lock::read_lock_metadata(&mut file);
    drop(file);
    Ok(DriverProbe::Unheld { stale_metadata })
}

/// Cooperatively interrupt the driver currently holding a ledger's lock by
/// probing for the current holder's `control_token` (see [`DriverHolder`])
/// and connecting to its control socket — the same graceful-stop path
/// `drive` already treats a `SIGINT` as requesting (P402), now requested
/// through an authenticated handshake instead of a raw pid signal (see the
/// module doc). Returns `true` only once a connection was accepted AND the
/// exact [`CONTROL_ACK`] bytes were read back — proof a live holder actually
/// ran `on_interrupt` — `false` for every other outcome: no driver holds the
/// lock, the holder's metadata does not carry a token (pre-token or
/// mid-write), the holder crashed leaving a stale socket file, the lock
/// changed hands before the new holder finished binding, or the connection
/// was accepted but the acknowledgement round trip did not complete within
/// [`CONTROL_CONNECT_TIMEOUT`]/[`CONTROL_STREAM_TIMEOUT`]. Never removes the
/// socket file itself — only the owning [`ControlListener`]'s `Drop` ever
/// unlinks its own socket, so a request racing a live holder's teardown can
/// never delete a *different*, freshly bound listener's socket out from
/// under it. Never escalates to `SIGKILL`: an unresponsive driver simply
/// stays `stopping` until it exits on its own.
pub fn request_interrupt(ledger_path: &Utf8Path) -> crate::Result<bool> {
    let control_token = match probe(ledger_path)? {
        DriverProbe::Held(Some(holder)) if !holder.control_token.is_empty() => holder.control_token,
        _ => return Ok(false),
    };
    let socket_path = control_socket_path(ledger_path, &control_token);
    let (tx, rx) = std::sync::mpsc::channel();
    let connect_path = socket_path.clone();
    std::thread::spawn(move || {
        let _ = tx.send(UnixStream::connect(connect_path.as_std_path()));
    });
    let stream = match rx.recv_timeout(CONTROL_CONNECT_TIMEOUT) {
        Ok(Ok(stream)) => stream,
        Ok(Err(e))
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(false);
        }
        Ok(Err(e)) => {
            return Err(crate::environment::Error::Filesystem {
                path: socket_path.to_string(),
                source: e,
            }
            .into());
        }
        // Timed out waiting for connect, or the connect thread's sender
        // dropped without sending (should not happen, but never block
        // forever either way): treat as "nothing confirmed interrupted".
        Err(_) => return Ok(false),
    };
    let mut stream = stream;
    let _ = stream.set_read_timeout(Some(CONTROL_STREAM_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CONTROL_STREAM_TIMEOUT));
    // The connection itself, plus this exact byte, is the authenticated
    // request; the listener discards its content after reading it.
    if stream.write_all(b"i").is_err() {
        return Ok(false);
    }
    let mut ack = [0u8; CONTROL_ACK.len()];
    if stream.read_exact(&mut ack).is_err() {
        return Ok(false);
    }
    Ok(ack == CONTROL_ACK)
}

fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
