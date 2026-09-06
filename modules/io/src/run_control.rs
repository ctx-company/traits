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
//! answers "is a driver attached, and if so, ask it to stop." The authenticated
//! control byte selects either interruption or pause; cooperative interruption reuses the existing `SIGINT` semantics that
//! `crate::app::interrupt`/`drive` already honor (see P402) — this module
//! never sends `SIGKILL` and never introduces a permanent canceled status.
//!
//! Ownership vs. display evidence is never conflated: a *contended* `flock`
//! always classifies as [`DriverProbe::Held`], even when the holder's
//! metadata is missing, malformed, or stale (e.g. read during a new
//! holder's acquire-to-write window) — only a genuinely *uncontended* lock
//! is `Unheld`. Holder metadata (pid/session/run id/start time) is display
//! evidence ONLY; callers may compare its session id with the action they are
//! about to take, but never authorize an OS signal from its pid. A pid read
//! from a file can be stale, forged, or reused by an unrelated process.
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

use crate::answer::{AnswerDeliveryVerdict, AnswerEnvelope};
use camino::{Utf8Path, Utf8PathBuf};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::time::Duration;

/// Cooperative command selected by the authenticated control socket byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCommand {
    Interrupt,
    Pause,
}

impl ControlCommand {
    pub fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            b'i' => Some(Self::Interrupt),
            b'p' => Some(Self::Pause),
            _ => None,
        }
    }

    pub fn wire(self) -> u8 {
        match self {
            Self::Interrupt => b'i',
            Self::Pause => b'p',
        }
    }
}

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
/// its interrupt or pause callback. Control requests treat anything else — including a
/// partial or missing read — as "not confirmed".
const CONTROL_ACK: &[u8] = b"ok";

/// Largest answer payload accepted from the local control socket.
const MAX_ANSWER_PAYLOAD_BYTES: usize = 1024 * 1024;

/// How long the listener waits for the drive thread to validate a delivered
/// answer. A disconnected or stalled drive must not retain the accept thread.
const ANSWER_DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);

pub type AnswerHandler = Arc<dyn Fn(AnswerEnvelope) -> AnswerDeliveryVerdict + Send + Sync>;

/// Callbacks owned by a driver while it holds the control socket.
pub struct ControlHandlers {
    on_command: Arc<dyn Fn(ControlCommand) + Send + Sync>,
    on_answer: Option<AnswerHandler>,
}

impl ControlHandlers {
    /// Build the handlers used by drivers that do not accept answer delivery.
    pub fn command_only(on_command: Arc<dyn Fn(ControlCommand) + Send + Sync>) -> Self {
        Self {
            on_command,
            on_answer: None,
        }
    }

    /// Add an answer callback to the ordinary command handler.
    pub fn with_answer(mut self, on_answer: AnswerHandler) -> Self {
        self.on_answer = Some(on_answer);
        self
    }
}

/// An answer sent by the control listener for the drive thread to process.
pub struct AnswerDelivery {
    pub envelope: AnswerEnvelope,
    reply: SyncSender<AnswerDeliveryVerdict>,
}

impl AnswerDelivery {
    /// Return the verdict to the listener that delivered this answer.
    pub fn reply(self, verdict: AnswerDeliveryVerdict) {
        let _ = self.reply.send(verdict);
    }
}

/// Handoff between the socket listener and its single ledger-writing drive
/// thread. The listener only queues delivery and waits for a verdict.
pub struct AnswerMailbox {
    accepting: Arc<AtomicBool>,
    sender: SyncSender<AnswerDelivery>,
    receiver: Receiver<AnswerDelivery>,
}

impl Default for AnswerMailbox {
    fn default() -> Self {
        Self::new()
    }
}

impl AnswerMailbox {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::sync_channel(1);
        Self {
            accepting: Arc::new(AtomicBool::new(false)),
            sender,
            receiver,
        }
    }

    /// Toggle delivery only for the interval where the drive is parked at an
    /// Ask frame. A request outside that interval is refused without waking it.
    pub fn set_accepting(&self, accepting: bool) {
        self.accepting.store(accepting, Ordering::SeqCst);
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<AnswerDelivery, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }

    pub fn handler(&self) -> AnswerHandler {
        let accepting = Arc::clone(&self.accepting);
        let sender = self.sender.clone();
        Arc::new(move |envelope| {
            if !accepting.load(Ordering::SeqCst) {
                return AnswerDeliveryVerdict::NotWaiting;
            }
            let (reply, response) = mpsc::sync_channel(1);
            // A prior delivery may be waiting for the drive thread. Do not
            // let that full one-slot queue bypass the listener's time bound.
            if sender.try_send(AnswerDelivery { envelope, reply }).is_err() {
                return AnswerDeliveryVerdict::NotWaiting;
            }
            response
                .recv_timeout(ANSWER_DELIVERY_TIMEOUT)
                .unwrap_or(AnswerDeliveryVerdict::NotWaiting)
        })
    }
}

/// Result of asking a live driver to deliver an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnswerDeliveryResult {
    Delivered(AnswerDeliveryVerdict),
    Undelivered,
}

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
    title_claim_owner: String,
    holder: DriverHolder,
    /// `None` when this acquisition never had liveness facts to index (not
    /// expected in production — every `try_acquire` caller supplies them —
    /// but tests and any future bare caller must still drop cleanly).
    session_id: Option<String>,
}

impl DriverLockGuard {
    /// Unique for this authoritative lock acquisition and therefore suitable
    /// for durable title-attempt ownership.
    pub fn title_claim_owner(&self) -> &str {
        &self.title_claim_owner
    }

    /// The metadata atomically written for this lock acquisition. Consumers
    /// must reuse it rather than manufacturing a second description of the
    /// live driver.
    pub fn holder(&self) -> &DriverHolder {
        &self.holder
    }
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
/// invokes the caller-supplied callback for every recognized command. The
/// connection authenticates the requester; its byte selects the cooperative
/// stop to make.
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
    on_command: Arc<dyn Fn(ControlCommand) + Send + Sync>,
    on_answer: Option<AnswerHandler>,
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
                    // Authentication comes from the socket; the byte selects
                    // the command. A read/write that never completes is bounded by the timeouts above
                    // rather than blocking this thread indefinitely; an idle
                    // or malicious client can only cost this thread up to
                    // `CONTROL_STREAM_TIMEOUT`, never wedge it forever.
                    if stream
                        .read(&mut byte)
                        .ok()
                        .filter(|count| *count == 1)
                        .is_some()
                    {
                        if let Some(command) = ControlCommand::from_wire(byte[0]) {
                            on_command(command);
                            let _ = stream.write_all(CONTROL_ACK);
                        } else if byte[0] == b'a'
                            && let Some(on_answer) = on_answer.as_ref()
                            && let Some(envelope) = read_answer_envelope(&mut stream)
                            && let Ok(body) = serde_json::to_vec(&on_answer(envelope))
                        {
                            let Ok(length) = u32::try_from(body.len()) else {
                                return;
                            };
                            let _ = stream.write_all(&length.to_be_bytes());
                            let _ = stream.write_all(&body);
                        }
                    }
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

fn read_answer_envelope(stream: &mut UnixStream) -> Option<AnswerEnvelope> {
    let mut length = [0u8; 4];
    stream.read_exact(&mut length).ok()?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_ANSWER_PAYLOAD_BYTES {
        return None;
    }
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
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
/// treating a miss as fatal). `on_command` is invoked (on a background
/// thread) once per authenticated control connection for as
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
    handlers: ControlHandlers,
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
        handlers.on_command,
        handlers.on_answer,
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
        title_claim_owner: control_token,
        holder,
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

/// Cooperatively interrupt the driver described by an already-probed holder's
/// `control_token` (see [`DriverHolder`]) by connecting to its control socket — the same graceful-stop path
/// `drive` already treats a `SIGINT` as requesting (P402), now requested
/// through an authenticated handshake instead of a raw pid signal (see the
/// module doc). Returns `true` only once a connection was accepted AND the
/// exact [`CONTROL_ACK`] bytes were read back — proof a live holder actually
/// ran `on_interrupt` — `false` for every other outcome: no driver holds the
/// lock, the holder's metadata does not carry a token (pre-token or
/// mid-write), the holder crashed leaving a stale socket file, the lock
/// changed hands, or the connection
/// was accepted but the acknowledgement round trip did not complete within
/// [`CONTROL_CONNECT_TIMEOUT`]/[`CONTROL_STREAM_TIMEOUT`]. Never removes the
/// socket file itself — only the owning [`ControlListener`]'s `Drop` ever
/// unlinks its own socket, so a request racing a live holder's teardown can
/// never delete a *different*, freshly bound listener's socket out from
/// under it. Never escalates to `SIGKILL`: an unresponsive driver simply
/// stays `stopping` until it exits on its own.
pub fn request_interrupt(ledger_path: &Utf8Path, holder: &DriverHolder) -> crate::Result<bool> {
    request_control(ledger_path, holder, ControlCommand::Interrupt)
}

/// Cooperatively pause the holder at its next frame boundary.
pub fn request_pause(ledger_path: &Utf8Path, holder: &DriverHolder) -> crate::Result<bool> {
    request_control(ledger_path, holder, ControlCommand::Pause)
}

/// Deliver an answer to the driver identified by an already-probed holder.
/// A missing socket, a pre-answer driver, or any incomplete round trip is
/// `Undelivered`; only a parsed typed verdict counts as a delivery.
pub fn request_answer(
    ledger_path: &Utf8Path,
    holder: &DriverHolder,
    envelope: &AnswerEnvelope,
) -> crate::Result<AnswerDeliveryResult> {
    let Some(mut stream) = connect_control(ledger_path, holder)? else {
        return Ok(AnswerDeliveryResult::Undelivered);
    };
    let payload = serde_json::to_vec(envelope).expect("answer envelope serializes");
    let Ok(length) = u32::try_from(payload.len()) else {
        return Ok(AnswerDeliveryResult::Undelivered);
    };
    if stream
        .write_all(b"a")
        .and_then(|_| stream.write_all(&length.to_be_bytes()))
        .and_then(|_| stream.write_all(&payload))
        .is_err()
    {
        return Ok(AnswerDeliveryResult::Undelivered);
    }
    let Some(verdict) = read_answer_verdict(&mut stream) else {
        return Ok(AnswerDeliveryResult::Undelivered);
    };
    Ok(AnswerDeliveryResult::Delivered(verdict))
}

fn request_control(
    ledger_path: &Utf8Path,
    holder: &DriverHolder,
    command: ControlCommand,
) -> crate::Result<bool> {
    let Some(mut stream) = connect_control(ledger_path, holder)? else {
        return Ok(false);
    };
    if stream.write_all(&[command.wire()]).is_err() {
        return Ok(false);
    }
    let mut ack = [0u8; CONTROL_ACK.len()];
    if stream.read_exact(&mut ack).is_err() {
        return Ok(false);
    }
    Ok(ack == CONTROL_ACK)
}

fn connect_control(
    ledger_path: &Utf8Path,
    holder: &DriverHolder,
) -> crate::Result<Option<UnixStream>> {
    if holder.control_token.is_empty() {
        return Ok(None);
    }
    let socket_path = control_socket_path(ledger_path, &holder.control_token);
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
            return Ok(None);
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
        Err(_) => return Ok(None),
    };
    let _ = stream.set_read_timeout(Some(CONTROL_STREAM_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CONTROL_STREAM_TIMEOUT));
    Ok(Some(stream))
}

fn read_answer_verdict(stream: &mut UnixStream) -> Option<AnswerDeliveryVerdict> {
    let mut length = [0u8; 4];
    stream.read_exact(&mut length).ok()?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_ANSWER_PAYLOAD_BYTES {
        return None;
    }
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_interrupt_uses_the_validated_holders_token_without_reprobing() {
        let ledger_path = Utf8Path::new("/tmp/ctx-run-control-token-test.json");
        let validated = DriverHolder {
            pid: 1,
            session_id: "selected".to_string(),
            run_id: "run-selected".to_string(),
            started_at_epoch_secs: 0,
            control_token: "validated-token".to_string(),
        };
        let replacement = DriverHolder {
            control_token: "replacement-token".to_string(),
            ..validated.clone()
        };
        let interrupted = Arc::new(AtomicBool::new(false));
        let observed = interrupted.clone();
        let replacement_socket = control_socket_path(ledger_path, &replacement.control_token);
        let listener = bind_control_listener(
            replacement_socket,
            Arc::new(move |_| {
                observed.store(true, Ordering::SeqCst);
            }),
            None,
        )
        .expect("bind replacement listener");

        assert!(
            !request_interrupt(ledger_path, &validated).expect("request interrupt"),
            "a replacement token must not be selected after validation"
        );
        assert!(
            !interrupted.load(Ordering::SeqCst),
            "the replacement listener must not receive the selected action"
        );
        drop(listener);
    }

    #[test]
    fn control_command_parses_only_the_known_wire_bytes() {
        assert_eq!(
            ControlCommand::from_wire(b'i'),
            Some(ControlCommand::Interrupt)
        );
        assert_eq!(ControlCommand::from_wire(b'p'), Some(ControlCommand::Pause));
        assert_eq!(ControlCommand::from_wire(b'x'), None);
        assert_eq!(ControlCommand::Interrupt.wire(), b'i');
        assert_eq!(ControlCommand::Pause.wire(), b'p');
    }

    #[test]
    fn unrecognized_control_byte_invokes_no_callback_and_is_not_acknowledged() {
        let path = control_socket_path(
            Utf8Path::new("/tmp/ctx-control-invalid.json"),
            &format!("invalid-{}", std::process::id()),
        );
        let called = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&called);
        let listener = bind_control_listener(
            path.clone(),
            Arc::new(move |_| {
                observed.store(true, Ordering::SeqCst);
            }),
            None,
        )
        .expect("bind listener");
        std::thread::sleep(CONTROL_POLL_INTERVAL);
        let mut stream = UnixStream::connect(path.as_std_path()).expect("connect listener");
        stream.write_all(b"x").expect("write invalid command");
        let _ = stream.set_read_timeout(Some(CONTROL_STREAM_TIMEOUT));
        let mut ack = [0; 2];
        assert!(stream.read_exact(&mut ack).is_err());
        assert!(!called.load(Ordering::SeqCst));
        drop(listener);
    }

    #[test]
    fn pause_request_delivers_the_pause_command_to_the_lock_holder() {
        let ledger_path = Utf8PathBuf::from(format!(
            "/tmp/ctx-control-pause-{}.json",
            std::process::id()
        ));
        let holder = DriverHolder {
            pid: std::process::id(),
            session_id: "session".to_string(),
            run_id: "run".to_string(),
            started_at_epoch_secs: 0,
            control_token: format!("pause-{}", std::process::id()),
        };
        let seen = Arc::new(std::sync::Mutex::new(None));
        let observed = Arc::clone(&seen);
        let listener = bind_control_listener(
            control_socket_path(&ledger_path, &holder.control_token),
            Arc::new(move |command| *observed.lock().expect("lock") = Some(command)),
            None,
        )
        .expect("bind listener");
        let deadline = std::time::Instant::now() + CONTROL_STREAM_TIMEOUT;
        let mut accepted = false;
        while std::time::Instant::now() < deadline {
            if request_pause(&ledger_path, &holder).expect("request pause") {
                accepted = true;
                break;
            }
            std::thread::sleep(CONTROL_POLL_INTERVAL);
        }
        assert!(
            accepted,
            "listener did not acknowledge pause before its deadline"
        );
        assert_eq!(*seen.lock().expect("lock"), Some(ControlCommand::Pause));
        drop(listener);
    }

    #[test]
    fn answer_wire_delivers_envelope_and_returns_length_prefixed_verdict() {
        let path = control_socket_path(
            Utf8Path::new("/tmp/ctx-control-answer.json"),
            &format!("answer-{}", std::process::id()),
        );
        let observed = Arc::new(std::sync::Mutex::new(None));
        let received = Arc::clone(&observed);
        let listener = bind_control_listener(
            path.clone(),
            Arc::new(|_| panic!("answer must not invoke command handler")),
            Some(Arc::new(move |envelope| {
                *received.lock().expect("lock") = Some(envelope);
                AnswerDeliveryVerdict::Accepted
            })),
        )
        .expect("bind listener");
        std::thread::sleep(CONTROL_POLL_INTERVAL);

        let envelope = AnswerEnvelope {
            target: "slot:ask-owner".to_string(),
            schema_ref: Some("schema:text".to_string()),
            expected_state_digest: "sha256:digest".to_string(),
            value: serde_json::Value::String("answer".to_string()),
        };
        let payload = serde_json::to_vec(&envelope).expect("serialize envelope");
        let mut stream = UnixStream::connect(path.as_std_path()).expect("connect listener");
        stream.write_all(b"a").expect("write answer discriminator");
        stream
            .write_all(&(payload.len() as u32).to_be_bytes())
            .expect("write answer length");
        stream.write_all(&payload).expect("write answer payload");
        let mut length = [0u8; 4];
        stream.read_exact(&mut length).expect("read verdict length");
        let mut response = vec![0; u32::from_be_bytes(length) as usize];
        stream.read_exact(&mut response).expect("read verdict");
        assert_eq!(
            serde_json::from_slice::<AnswerDeliveryVerdict>(&response).expect("parse verdict"),
            AnswerDeliveryVerdict::Accepted
        );
        assert_eq!(*observed.lock().expect("lock"), Some(envelope));
        drop(listener);
    }

    #[test]
    fn request_answer_round_trips_each_delivery_verdict() {
        let ledger_path = Utf8PathBuf::from(format!(
            "/tmp/ctx-control-answer-verdicts-{}.json",
            std::process::id()
        ));
        let envelope = AnswerEnvelope {
            target: "slot:ask-owner".to_string(),
            schema_ref: None,
            expected_state_digest: "sha256:digest".to_string(),
            value: serde_json::Value::Null,
        };
        let verdicts = [
            AnswerDeliveryVerdict::Accepted,
            AnswerDeliveryVerdict::Stale,
            AnswerDeliveryVerdict::Cancelled,
            AnswerDeliveryVerdict::NotWaiting,
            AnswerDeliveryVerdict::RejectedCorrection {
                detail: "correction required".to_string(),
            },
            AnswerDeliveryVerdict::NotRouted,
        ];
        for (index, verdict) in verdicts.into_iter().enumerate() {
            let holder = DriverHolder {
                pid: std::process::id(),
                session_id: "session".to_string(),
                run_id: "run".to_string(),
                started_at_epoch_secs: 0,
                control_token: format!("answer-verdict-{}-{index}", std::process::id()),
            };
            let callback_verdict = verdict.clone();
            let listener = bind_control_listener(
                control_socket_path(&ledger_path, &holder.control_token),
                Arc::new(|_| panic!("answer must not invoke command handler")),
                Some(Arc::new(move |_| callback_verdict.clone())),
            )
            .expect("bind listener");
            std::thread::sleep(CONTROL_POLL_INTERVAL);
            assert_eq!(
                request_answer(&ledger_path, &holder, &envelope).expect("request answer"),
                AnswerDeliveryResult::Delivered(verdict)
            );
            drop(listener);
        }
    }

    #[test]
    fn mailbox_refuses_when_not_accepting_and_hands_accepted_delivery_to_drive() {
        let ledger_path = Utf8PathBuf::from(format!(
            "/tmp/ctx-control-mailbox-{}.json",
            std::process::id()
        ));
        let holder = DriverHolder {
            pid: std::process::id(),
            session_id: "session".to_string(),
            run_id: "run".to_string(),
            started_at_epoch_secs: 0,
            control_token: format!("mailbox-{}", std::process::id()),
        };
        let mailbox = AnswerMailbox::new();
        let listener = bind_control_listener(
            control_socket_path(&ledger_path, &holder.control_token),
            Arc::new(|_| panic!("answer must not invoke command handler")),
            Some(mailbox.handler()),
        )
        .expect("bind listener");
        std::thread::sleep(CONTROL_POLL_INTERVAL);
        let envelope = AnswerEnvelope {
            target: "slot:ask-owner".to_string(),
            schema_ref: None,
            expected_state_digest: "sha256:digest".to_string(),
            value: serde_json::Value::Bool(true),
        };
        assert_eq!(
            request_answer(&ledger_path, &holder, &envelope).expect("request while idle"),
            AnswerDeliveryResult::Delivered(AnswerDeliveryVerdict::NotWaiting)
        );

        mailbox.set_accepting(true);
        let request_ledger = ledger_path.clone();
        let request_holder = holder.clone();
        let request_envelope = envelope.clone();
        let request = std::thread::spawn(move || {
            request_answer(&request_ledger, &request_holder, &request_envelope)
                .expect("request while accepting")
        });
        let received = mailbox
            .recv_timeout(CONTROL_STREAM_TIMEOUT)
            .expect("drive receives answer");
        assert_eq!(received.envelope, envelope);
        received.reply(AnswerDeliveryVerdict::Accepted);
        assert_eq!(
            request.join().expect("join requester"),
            AnswerDeliveryResult::Delivered(AnswerDeliveryVerdict::Accepted)
        );
        drop(listener);
    }

    #[test]
    fn request_answer_to_a_legacy_listener_is_undelivered() {
        let ledger_path = Utf8PathBuf::from(format!(
            "/tmp/ctx-control-answer-legacy-{}.json",
            std::process::id()
        ));
        let holder = DriverHolder {
            pid: std::process::id(),
            session_id: "session".to_string(),
            run_id: "run".to_string(),
            started_at_epoch_secs: 0,
            control_token: format!("answer-legacy-{}", std::process::id()),
        };
        let listener = bind_control_listener(
            control_socket_path(&ledger_path, &holder.control_token),
            Arc::new(|_| {}),
            None,
        )
        .expect("bind listener");
        std::thread::sleep(CONTROL_POLL_INTERVAL);
        assert_eq!(
            request_answer(
                &ledger_path,
                &holder,
                &AnswerEnvelope {
                    target: "slot:ask-owner".to_string(),
                    schema_ref: None,
                    expected_state_digest: "sha256:digest".to_string(),
                    value: serde_json::Value::Null,
                },
            )
            .expect("request answer"),
            AnswerDeliveryResult::Undelivered
        );
        let missing_holder = DriverHolder {
            control_token: format!("missing-answer-{}", std::process::id()),
            ..holder.clone()
        };
        assert_eq!(
            request_answer(
                &ledger_path,
                &missing_holder,
                &AnswerEnvelope {
                    target: "slot:ask-owner".to_string(),
                    schema_ref: None,
                    expected_state_digest: "sha256:digest".to_string(),
                    value: serde_json::Value::Null,
                },
            )
            .expect("request missing answer socket"),
            AnswerDeliveryResult::Undelivered
        );
        drop(listener);
    }

    #[test]
    fn malformed_answer_payload_invokes_no_callback_and_returns_no_verdict() {
        let path = control_socket_path(
            Utf8Path::new("/tmp/ctx-control-malformed-answer.json"),
            &format!("malformed-answer-{}", std::process::id()),
        );
        let called = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&called);
        let listener = bind_control_listener(
            path.clone(),
            Arc::new(|_| panic!("malformed answer must not invoke command handler")),
            Some(Arc::new(move |_| {
                observed.store(true, Ordering::SeqCst);
                AnswerDeliveryVerdict::Accepted
            })),
        )
        .expect("bind listener");
        std::thread::sleep(CONTROL_POLL_INTERVAL);

        let mut stream = UnixStream::connect(path.as_std_path()).expect("connect listener");
        stream.write_all(b"a").expect("write answer discriminator");
        stream
            .write_all(&4u32.to_be_bytes())
            .expect("write malformed length");
        stream.write_all(b"nope").expect("write malformed JSON");
        stream
            .set_read_timeout(Some(CONTROL_STREAM_TIMEOUT))
            .expect("set timeout");
        let mut verdict = [0u8; 1];
        assert!(stream.read_exact(&mut verdict).is_err());
        assert!(!called.load(Ordering::SeqCst));
        drop(listener);
    }
}
