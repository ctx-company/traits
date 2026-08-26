//! Machine-wide run center: a small, disposable index over run ledgers.
//!
//! The SQLite database is only a restart cache. Ledgers and their driver
//! flocks remain authoritative, so a deleted database is rebuilt on the next
//! scan and a crashed center never affects a driver.
//! Unlike `run_session::InventoryCache`, which intentionally dies with each
//! process and makes cold dashboard processes parse again, this cache is a
//! disposable machine-wide derived index for the long-lived center.

use camino::{Utf8Path, Utf8PathBuf};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(test)]
thread_local! {
    static LEDGER_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    // Test-only checkpoint for the stat-to-parse race. Keeping it thread-local
    // lets the test replace the atomic ledger at the exact read boundary.
    static AFTER_REFRESH_STAT: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

fn read_session(ledger: &Utf8Path) -> crate::Result<ctx_traits_core::procedure::session::Session> {
    #[cfg(test)]
    LEDGER_READS.with(|reads| reads.set(reads.get() + 1));
    crate::run_session::read_run_session(ledger)
}

#[cfg(test)]
fn run_after_refresh_stat() {
    AFTER_REFRESH_STAT.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(not(test))]
fn run_after_refresh_stat() {}

pub const CENTER_PROCESS_SENTINEL: &str = "__ctx-center";
// Sessions are returned only by `get`; permit a representative full ledger
// while still bounding memory consumed by any peer line.
const MAX_LINE_BYTES: usize = 1024 * 1024;
const STREAM_TIMEOUT: Duration = Duration::from_secs(2);
// This covers the complete bounded spawn-readiness lease as well as normal
// connection retries. A caller that lost arbitration must not give up while
// the winner is still exclusively bringing its child online.
const CONNECT_RETRIES: usize = 220;
const RETRY_DELAY: Duration = Duration::from_millis(50);
// Opening the shared index can wait for another version's short SQLite
// transaction. Keep the spawn lease through that bounded startup window.
const SPAWN_READY_TIMEOUT: Duration = Duration::from_secs(10);
const SPAWN_ABORT_TIMEOUT: Duration = Duration::from_secs(10);
const SCAN_INTERVAL: Duration = Duration::from_secs(2);
const IDLE_TIMEOUT: Duration = Duration::from_secs(15);
const SQLITE_BUSY_TIMEOUT_MS: u64 = 2_000;
const NOTIFIER_QUEUE: usize = 32;
const SUBSCRIBER_QUEUE: usize = 64;
const MODEL_QUEUE: usize = 128;
const NOTIFIER_BACKOFF_MIN: Duration = Duration::from_millis(25);
const NOTIFIER_BACKOFF_MAX: Duration = Duration::from_secs(2);
// Keep this marker at the version understood by the already-shipped center.
// Installed binaries share this SQLite file, and v1 rejects any other value.
const CENTER_SCHEMA_VERSION: i64 = 1;
// RunSummary is a separately versioned JSON projection. Newer centers can
// rebuild it without making a concurrent v1 center reject the shared index.
const CENTER_PROJECTION_VERSION: i64 = 2;
static NEXT_SUBSCRIBER: AtomicU64 = AtomicU64::new(1);

/// One bounded, correlated JSON-lines request.  Keeping the protocol types at
/// this boundary prevents a malformed peer from becoming an untyped command on
/// the model-owner channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum Request {
    Register {
        id: String,
        #[serde(flatten)]
        registration: DriverRegistration,
    },
    FrameDone {
        id: String,
        ledger_path: String,
    },
    ActivityLine {
        id: String,
        ledger_path: String,
        activity: crate::activity_sidecar::ActivityRecord,
    },
    Ended {
        id: String,
        ledger_path: String,
    },
    Subscribe {
        id: String,
        repo_key: Option<String>,
    },
    List {
        id: String,
        repo_key: Option<String>,
    },
    Get {
        id: String,
        session_id: String,
        repo_key: Option<String>,
    },
    Resolve {
        id: String,
        session_id: String,
        repo_key: Option<String>,
    },
    FindByRunId {
        id: String,
        run_id: String,
        repo_key: Option<String>,
    },
    Stats {
        id: String,
        since_epoch: Option<u64>,
        trait_id: Option<String>,
        repo_key: Option<String>,
    },
    StandingWall {
        id: String,
        wall_id: String,
        dispatched_task: String,
        repo_key: Option<String>,
    },
}

impl Request {
    fn id(&self) -> &str {
        match self {
            Self::Register { id, .. }
            | Self::FrameDone { id, .. }
            | Self::ActivityLine { id, .. }
            | Self::Ended { id, .. }
            | Self::Subscribe { id, .. }
            | Self::List { id, .. }
            | Self::Get { id, .. }
            | Self::Resolve { id, .. }
            | Self::FindByRunId { id, .. }
            | Self::Stats { id, .. }
            | Self::StandingWall { id, .. } => id,
        }
    }

    fn notification_ledger(&self) -> Option<&str> {
        match self {
            Self::FrameDone { ledger_path, .. }
            | Self::ActivityLine { ledger_path, .. }
            | Self::Ended { ledger_path, .. } => Some(ledger_path),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum WireMessage {
    Hello { id: String },
    Ready { id: String },
    Response { id: String, result: ResponseResult },
    SnapshotStart { id: String },
    SnapshotRow { row: Box<CenterPublicRow> },
    SnapshotEnd,
    Delta { delta: CenterDelta },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "kebab-case")]
enum ResponseResult {
    Ok,
    List(Vec<CenterPublicRow>),
    Get(GetWireResult),
    Resolve(ResolveWireResult),
    FindByRunId(Vec<CenterPublicRow>),
    Stats(Box<ctx_traits_core::procedure::stats::StatsReport>),
    StandingWall(Option<crate::dispatch_preflight::StandingWall>),
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "kebab-case")]
enum GetWireResult {
    Missing,
    Session(Box<ctx_traits_core::procedure::session::Session>),
    Ambiguous(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "kebab-case")]
enum ResolveWireResult {
    Missing,
    Row(Box<CenterPublicRow>),
    Ambiguous(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum CenterDelta {
    Appeared {
        row: Box<CenterPublicRow>,
    },
    RowChanged {
        row: Box<CenterPublicRow>,
    },
    Ended {
        row: Box<CenterPublicRow>,
    },
    ActivityLine {
        row: Box<CenterPublicRow>,
        activity: crate::activity_sidecar::ActivityRecord,
    },
}

/// Canonical driver identity supplied by the held driver lock.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriverRegistration {
    pub ledger_path: String,
    pub holder: crate::run_control::DriverHolder,
}

#[derive(Debug, Clone)]
enum DriverEvent {
    Register,
    FrameDone,
    ActivityLine(crate::activity_sidecar::ActivityRecord),
    Ended,
}

/// Best-effort, non-blocking notification handle for a driving process.
///
/// Every producer operation is `try_send`; failure only loses center freshness
/// and never changes the ledger write or drive result that preceded it.
#[derive(Clone)]
pub struct DriverNotifier {
    sender: mpsc::SyncSender<DriverEvent>,
}

impl DriverNotifier {
    pub fn new(registration: DriverRegistration) -> Self {
        let (sender, receiver) = mpsc::sync_channel(notifier_queue_capacity());
        // This debug-only seam lets process proofs compare notification loss to
        // an otherwise identical drive with transport disabled. It is not a
        // runtime feature of release builds.
        #[cfg(debug_assertions)]
        if std::env::var_os("CTX_CENTER_DISABLE_NOTIFICATIONS").is_some() {
            drop(receiver);
            return Self { sender };
        }
        // Notification delivery is strictly best effort. In particular, an OS
        // refusal to create this helper thread must not panic into a drive.
        if std::thread::Builder::new()
            .name("ctx-center-notifier".to_string())
            .spawn(move || notifier_worker(registration, receiver))
            .is_err()
        {
            // Drop the receiver so producers retain their non-blocking,
            // failure-isolated behavior through `try_send` below.
        }
        let notifier = Self { sender };
        notifier.register();
        notifier
    }

    pub fn register(&self) {
        let _ = self.sender.try_send(DriverEvent::Register);
    }
    pub fn frame_done(&self) {
        let _ = self.sender.try_send(DriverEvent::FrameDone);
    }
    pub fn activity_line(&self, record: crate::activity_sidecar::ActivityRecord) {
        let _ = self.sender.try_send(DriverEvent::ActivityLine(record));
    }
    pub fn ended(&self) {
        let _ = self.sender.try_send(DriverEvent::Ended);
    }
}

fn notifier_queue_capacity() -> usize {
    #[cfg(debug_assertions)]
    if let Some(capacity) = std::env::var("CTX_CENTER_NOTIFIER_QUEUE")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|capacity| *capacity > 0)
    {
        return capacity;
    }
    NOTIFIER_QUEUE
}

fn notifier_worker(registration: DriverRegistration, receiver: mpsc::Receiver<DriverEvent>) {
    notifier_worker_with(registration, receiver, notifier_connect)
}

fn notifier_worker_with(
    registration: DriverRegistration,
    receiver: mpsc::Receiver<DriverEvent>,
    connect: impl FnMut() -> crate::Result<UnixStream>,
) {
    notifier_worker_with_sleep(registration, receiver, connect, std::thread::sleep)
}

fn notifier_worker_with_sleep(
    registration: DriverRegistration,
    receiver: mpsc::Receiver<DriverEvent>,
    mut connect: impl FnMut() -> crate::Result<UnixStream>,
    mut sleep: impl FnMut(Duration),
) {
    let mut backoff = NOTIFIER_BACKOFF_MIN;
    let mut stream: Option<UnixStream> = None;
    // Retain the event until it reaches an acknowledged center. A failed write
    // identifies a stale connection, not an undurable frame that can be lost.
    let mut pending = None;
    loop {
        let event = match pending.take() {
            Some(event) => event,
            None => match receiver.recv() {
                Ok(event) => event,
                Err(_) => return,
            },
        };
        let wire_event = match &event {
            // Registration is sent as the first line for every connection.
            // Do not duplicate it merely because this is the initial event.
            DriverEvent::Register => None,
            DriverEvent::FrameDone => Some(("frame-done", None)),
            DriverEvent::ActivityLine(record) => Some(("activity-line", Some(record.clone()))),
            DriverEvent::Ended => Some(("ended", None)),
        };
        if stream.is_none() {
            stream = connect()
                .and_then(|mut candidate| {
                    notifier_send(
                        &mut candidate,
                        Request::Register {
                            id: next_id("notify"),
                            registration: registration.clone(),
                        },
                    )?;
                    Ok(candidate)
                })
                .ok();
        }
        let sent = stream.is_some()
            && wire_event.is_none_or(|(kind, activity)| {
                stream.as_mut().is_some_and(|stream| {
                    notifier_send(
                        stream,
                        match kind {
                            "frame-done" => Request::FrameDone {
                                id: next_id("notify"),
                                ledger_path: registration.ledger_path.clone(),
                            },
                            "activity-line" => Request::ActivityLine {
                                id: next_id("notify"),
                                ledger_path: registration.ledger_path.clone(),
                                activity: activity.expect("activity event has a record"),
                            },
                            "ended" => Request::Ended {
                                id: next_id("notify"),
                                ledger_path: registration.ledger_path.clone(),
                            },
                            _ => unreachable!("driver event maps to a known request"),
                        },
                    )
                    .is_ok()
                })
            });
        if sent {
            backoff = NOTIFIER_BACKOFF_MIN;
        } else {
            stream = None;
            pending = Some(event);
            // Retry the retained event even if its final producer handle was
            // dropped immediately after enqueueing it. The worker exits on
            // the next empty receive once that event is acknowledged.
            sleep(backoff);
            backoff = next_notifier_backoff(backoff);
        }
    }
}

fn next_notifier_backoff(backoff: Duration) -> Duration {
    (backoff * 2).min(NOTIFIER_BACKOFF_MAX)
}

/// Notifications are acknowledged so a dead peer is detected before the
/// notifier considers a durable frame fresh. Producers still only enqueue via
/// `try_send`; all acknowledgement I/O remains on this worker thread.
fn notifier_send(stream: &mut UnixStream, request: Request) -> crate::Result<()> {
    let id = request.id().to_owned();
    write_line(stream, &request)?;
    let reply: WireMessage = serde_json::from_slice(&read_line(stream)?).map_err(|source| {
        crate::parse::Error::JsonDeserialize {
            context: "decode center notification acknowledgement".to_string(),
            source,
        }
    })?;
    match reply {
        WireMessage::Response {
            id: reply_id,
            result: ResponseResult::Ok,
        } if reply_id == id => Ok(()),
        _ => Err(protocol_error("invalid notification acknowledgement")),
    }
}

/// Notifiers must not invoke `ensure_connected`: its readiness/spawn retry is
/// for interactive clients and could turn a best-effort drive notification into
/// a long-lived retry worker. A notifier only attempts the already configured
/// endpoint once and lets its capped backoff govern later attempts.
fn notifier_connect() -> crate::Result<UnixStream> {
    let socket = center_paths()?.socket;
    let mut stream =
        UnixStream::connect(socket.as_std_path()).map_err(|source| io_error(&socket, source))?;
    handshake(&mut stream)?;
    Ok(stream)
}

/// Start the center if needed without waiting for listener readiness. This is
/// for drivers only; interactive query clients should use [`ensure_connected`].
pub fn start_for_driver() {
    let Ok(paths) = center_paths() else { return };
    let Ok(executable) = center_executable() else {
        return;
    };
    // `try_spawn_detached` owns the same under-flock socket and launch-marker
    // arbitration as query startup. In particular, its short handshake treats
    // an already-connected but slow listener as inconclusive, rather than
    // replacing a live center during its fork-to-bind window.
    let _ = try_spawn_detached(&paths, &executable);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CenterPaths {
    pub socket: Utf8PathBuf,
    pub spawn_lock: Utf8PathBuf,
    pub runs_root: Utf8PathBuf,
    pub index: Utf8PathBuf,
}

/// Resolve the one private configuration tuple shared by every client and the
/// sentinel. Partial tuples are always rejected rather than silently mixing a
/// test endpoint with production state.
fn center_paths() -> crate::Result<CenterPaths> {
    let configured = |name: &str| std::env::var(name).ok().map(Utf8PathBuf::from);
    match (
        configured("CTX_CENTER_SOCKET"),
        configured("CTX_CENTER_SPAWN_LOCK"),
        configured("CTX_CENTER_RUNS_ROOT"),
        configured("CTX_CENTER_INDEX"),
    ) {
        (Some(socket), Some(spawn_lock), Some(runs_root), Some(index)) => Ok(CenterPaths {
            socket,
            spawn_lock,
            runs_root,
            index,
        }),
        (None, None, None, None) => production_paths(),
        _ => Err(protocol_error("incomplete private center configuration")),
    }
}

fn center_executable() -> crate::Result<std::path::PathBuf> {
    match std::env::var_os("CTX_CENTER_EXECUTABLE") {
        Some(executable) => Ok(std::path::PathBuf::from(executable)),
        None => std::env::current_exe()
            .map_err(|source| io_error(&Utf8PathBuf::from("current executable"), source)),
    }
}

/// Version-scoped socket paths prevent two installed versions from speaking an
/// incompatible protocol. The durable index is deliberately shared.
pub fn production_paths() -> crate::Result<CenterPaths> {
    let uid = unsafe { libc::getuid() };
    let runs_root = crate::state::global_runs_family_root()?;
    Ok(versioned_paths(uid, env!("CARGO_PKG_VERSION"), runs_root))
}

fn versioned_paths(uid: u32, version: &str, runs_root: Utf8PathBuf) -> CenterPaths {
    let stem = format!("/tmp/ctx-{uid}-{version}");
    CenterPaths {
        socket: Utf8PathBuf::from(format!("{stem}.sock")),
        spawn_lock: Utf8PathBuf::from(format!("{stem}.spawn.lock")),
        index: runs_root.join("index.sqlite3"),
        runs_root,
    }
}

fn next_id(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        NEXT_SUBSCRIBER.fetch_add(1, Ordering::Relaxed)
    )
}

fn io_error(path: &Utf8Path, source: std::io::Error) -> crate::Error {
    crate::environment::Error::Filesystem {
        path: path.to_string(),
        source,
    }
    .into()
}

fn protocol_error(message: impl Into<String>) -> crate::Error {
    crate::Error::Usage {
        message: format!("center protocol: {}", message.into()),
    }
}

fn write_line(stream: &mut UnixStream, value: &impl Serialize) -> crate::Result<()> {
    let mut line =
        serde_json::to_vec(value).map_err(|source| crate::parse::Error::JsonSerialize {
            context: "serialize center protocol line".to_string(),
            source,
        })?;
    line.push(b'\n');
    if line.len() > MAX_LINE_BYTES {
        return Err(protocol_error("line exceeds limit"));
    }
    stream
        .write_all(&line)
        .map_err(|source| io_error(&Utf8PathBuf::from("center socket"), source))
}

fn read_line(stream: &mut UnixStream) -> crate::Result<Vec<u8>> {
    read_line_bounded(stream, false, STREAM_TIMEOUT)
}

/// Registered streams may legitimately be silent between frames. Once a peer
/// starts a line it still has the same absolute deadline, so a trickling peer
/// cannot retain a worker indefinitely.
fn read_line_with_idle(stream: &mut UnixStream, allow_idle: bool) -> crate::Result<Vec<u8>> {
    read_line_bounded(stream, allow_idle, STREAM_TIMEOUT)
}

/// Read one bounded JSON line. `allow_idle` permits established persistent
/// streams to remain quiet; any started line always has an absolute deadline.
fn read_line_bounded(
    stream: &mut UnixStream,
    allow_idle: bool,
    timeout: Duration,
) -> crate::Result<Vec<u8>> {
    stream
        .set_nonblocking(true)
        .map_err(|source| io_error(&Utf8PathBuf::from("center socket"), source))?;
    let result = (|| {
        let mut line = Vec::new();
        let mut deadline = None;
        loop {
            if line.len() == MAX_LINE_BYTES {
                return Err(protocol_error("line exceeds limit"));
            }
            let remaining = deadline
                .map(|deadline: Instant| deadline.saturating_duration_since(Instant::now()))
                .unwrap_or(Duration::from_millis(5));
            if deadline.is_some() && remaining.is_zero() {
                return Err(protocol_error("read timed out"));
            }
            let mut byte = [0u8; 1];
            match stream.read(&mut byte) {
                Ok(0) if line.is_empty() => return Err(protocol_error("unexpected EOF")),
                Ok(0) => return Err(protocol_error("unterminated line")),
                Ok(_) if byte[0] == b'\n' => return Ok(line),
                Ok(_) => {
                    deadline.get_or_insert_with(|| Instant::now() + timeout);
                    line.push(byte[0]);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if !allow_idle && deadline.is_none() {
                        deadline = Some(Instant::now() + timeout);
                    }
                    std::thread::sleep(remaining.min(Duration::from_millis(5)));
                }
                Err(source) => return Err(io_error(&Utf8PathBuf::from("center socket"), source)),
            }
        }
    })();
    // A peer can close while this read is unwinding; Darwin then rejects the
    // mode reset with EINVAL. The completed protocol result is authoritative.
    let _ = stream.set_nonblocking(false);
    result
}

fn handshake(stream: &mut UnixStream) -> crate::Result<()> {
    handshake_with_timeout(stream, STREAM_TIMEOUT)
}

fn handshake_with_timeout(stream: &mut UnixStream, timeout: Duration) -> crate::Result<()> {
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|source| io_error(&Utf8PathBuf::from("center socket"), source))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|source| io_error(&Utf8PathBuf::from("center socket"), source))?;
    let id = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    write_line(stream, &WireMessage::Hello { id: id.clone() })?;
    let ready: WireMessage = serde_json::from_slice(&read_line_with_deadline(
        stream, false, timeout,
    )?)
    .map_err(|source| crate::parse::Error::JsonDeserialize {
        context: "decode center ready line".to_string(),
        source,
    })?;
    match ready {
        WireMessage::Ready { id: ready_id } if ready_id == id => Ok(()),
        _ => Err(protocol_error("unexpected ready response")),
    }
}

fn read_line_with_deadline(
    stream: &mut UnixStream,
    allow_idle: bool,
    timeout: Duration,
) -> crate::Result<Vec<u8>> {
    read_line_bounded(stream, allow_idle, timeout)
}

/// Connect to the center, launching at most one detached center across all
/// simultaneous callers. A complete handshake retries once after EOF.
pub fn ensure_connected() -> crate::Result<UnixStream> {
    let paths = center_paths()?;
    let executable = center_executable()?;
    ensure_connected_at(&paths, &executable)
}

/// Send one correlated request to the center. Queries use this path; it never
/// opens a ledger in the calling process.
fn request(request: Request) -> crate::Result<ResponseResult> {
    let id = request.id().to_owned();
    let mut stream = ensure_connected()?;
    write_line(&mut stream, &request)?;
    let reply: WireMessage =
        serde_json::from_slice(&read_line(&mut stream)?).map_err(|source| {
            crate::parse::Error::JsonDeserialize {
                context: "decode center response".to_string(),
                source,
            }
        })?;
    match reply {
        WireMessage::Response {
            id: reply_id,
            result,
        } if reply_id == id => match result {
            ResponseResult::Error { message } => Err(protocol_error(message)),
            result => Ok(result),
        },
        _ => Err(protocol_error("uncorrelated response")),
    }
}

/// Model-backed query helpers. They deliberately only encode/decode protocol
/// values: ledger reconstruction belongs exclusively to the center owner.
pub fn list(repo_key: Option<&str>) -> crate::Result<Vec<CenterPublicRow>> {
    match request(Request::List {
        id: next_id("query"),
        repo_key: repo_key.map(str::to_owned),
    })? {
        ResponseResult::List(rows) => Ok(rows),
        _ => Err(protocol_error("unexpected list response")),
    }
}

pub fn get(session_id: &str, repo_key: Option<&str>) -> crate::Result<GetResult> {
    match request(Request::Get {
        id: next_id("query"),
        session_id: session_id.to_owned(),
        repo_key: repo_key.map(str::to_owned),
    })? {
        ResponseResult::Get(GetWireResult::Missing) => Ok(GetResult::Missing),
        ResponseResult::Get(GetWireResult::Session(session)) => Ok(GetResult::Session(session)),
        ResponseResult::Get(GetWireResult::Ambiguous(ids)) => Ok(GetResult::Ambiguous(ids)),
        _ => Err(protocol_error("unexpected get response")),
    }
}

/// Deterministic result of a cached full-session lookup.
#[derive(Debug, Clone, PartialEq)]
pub enum GetResult {
    Missing,
    Session(Box<ctx_traits_core::procedure::session::Session>),
    Ambiguous(Vec<String>),
}

/// Deterministic result of exact-before-prefix public-row resolution.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolveResult {
    Missing,
    Row(Box<CenterPublicRow>),
    Ambiguous(Vec<String>),
}

pub fn resolve(session_id: &str, repo_key: Option<&str>) -> crate::Result<ResolveResult> {
    match request(Request::Resolve {
        id: next_id("query"),
        session_id: session_id.to_owned(),
        repo_key: repo_key.map(str::to_owned),
    })? {
        ResponseResult::Resolve(ResolveWireResult::Missing) => Ok(ResolveResult::Missing),
        ResponseResult::Resolve(ResolveWireResult::Row(row)) => Ok(ResolveResult::Row(row)),
        ResponseResult::Resolve(ResolveWireResult::Ambiguous(ids)) => {
            Ok(ResolveResult::Ambiguous(ids))
        }
        _ => Err(protocol_error("unexpected resolve response")),
    }
}

pub fn find_by_run_id(run_id: &str, repo_key: Option<&str>) -> crate::Result<Vec<CenterPublicRow>> {
    match request(Request::FindByRunId {
        id: next_id("query"),
        run_id: run_id.to_owned(),
        repo_key: repo_key.map(str::to_owned),
    })? {
        ResponseResult::FindByRunId(rows) => Ok(rows),
        _ => Err(protocol_error("unexpected find-by-run-id response")),
    }
}

pub fn stats(
    since_epoch: Option<u64>,
    trait_id: Option<&str>,
    repo_key: Option<&str>,
) -> crate::Result<ctx_traits_core::procedure::stats::StatsReport> {
    match request(Request::Stats {
        id: next_id("query"),
        since_epoch,
        trait_id: trait_id.map(str::to_owned),
        repo_key: repo_key.map(str::to_owned),
    })? {
        ResponseResult::Stats(report) => Ok(*report),
        _ => Err(protocol_error("unexpected stats response")),
    }
}

/// Answer the dispatch wall preflight from sessions already reconstructed by
/// the model owner; clients never scan or reopen ledgers for this query.
pub fn find_standing_wall(
    wall_id: &str,
    dispatched_task: &str,
    repo_key: Option<&str>,
) -> crate::Result<Option<crate::dispatch_preflight::StandingWall>> {
    match request(Request::StandingWall {
        id: next_id("query"),
        wall_id: wall_id.to_owned(),
        dispatched_task: dispatched_task.to_owned(),
        repo_key: repo_key.map(str::to_owned),
    })? {
        ResponseResult::StandingWall(wall) => Ok(wall),
        _ => Err(protocol_error("unexpected standing-wall response")),
    }
}

/// Events from one snapshot-plus-delta center subscription. Snapshot rows are
/// compact projections; callers fetch a complete session only through `get`.
#[derive(Debug, Clone, PartialEq)]
pub enum CenterEvent {
    SnapshotStart,
    SnapshotRow(Box<CenterPublicRow>),
    SnapshotEnd,
    Delta(CenterDelta),
}

pub struct CenterSubscription {
    receiver: mpsc::Receiver<CenterEvent>,
    // Closing this clone wakes the server's EOF watcher immediately instead of
    // retaining an otherwise idle subscription until the next model change.
    closer: UnixStream,
}

impl CenterSubscription {
    pub fn recv(&self) -> Result<CenterEvent, mpsc::RecvError> {
        self.receiver.recv()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<CenterEvent, mpsc::RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }
}

impl Drop for CenterSubscription {
    fn drop(&mut self) {
        let _ = self.closer.shutdown(Shutdown::Both);
    }
}

/// Establish a persistent, repository-scoped snapshot stream. The reader owns
/// socket I/O, leaving consumers with a typed channel rather than a protocol
/// stream. Dropping the handle shuts down its reader socket so the server's
/// EOF watcher removes the subscriber without waiting for a later delta.
pub fn subscribe(repo_key: Option<&str>) -> crate::Result<CenterSubscription> {
    let mut stream = ensure_connected()?;
    let id = format!(
        "subscription-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let expected_start = id.clone();
    write_line(
        &mut stream,
        &Request::Subscribe {
            id: id.clone(),
            repo_key: repo_key.map(str::to_owned),
        },
    )?;
    let closer = stream
        .try_clone()
        .map_err(|source| io_error(&Utf8PathBuf::from("center socket"), source))?;
    let (sender, receiver) = mpsc::sync_channel(SUBSCRIBER_QUEUE);
    std::thread::spawn(move || {
        loop {
            let Ok(line) = read_line_with_idle(&mut stream, true) else {
                return;
            };
            let Ok(value) = serde_json::from_slice::<WireMessage>(&line) else {
                return;
            };
            let event = match value {
                WireMessage::SnapshotStart { id } if id == expected_start => {
                    Some(CenterEvent::SnapshotStart)
                }
                WireMessage::SnapshotRow { row } => Some(CenterEvent::SnapshotRow(row)),
                WireMessage::SnapshotEnd => Some(CenterEvent::SnapshotEnd),
                WireMessage::Delta { delta } => Some(CenterEvent::Delta(delta)),
                _ => None,
            };
            let Some(event) = event else { return };
            if sender.send(event).is_err() {
                return;
            }
        }
    });
    Ok(CenterSubscription { receiver, closer })
}

fn ensure_connected_at(
    paths: &CenterPaths,
    executable: &std::path::Path,
) -> crate::Result<UnixStream> {
    let mut retried_eof = false;
    for _ in 0..CONNECT_RETRIES {
        match UnixStream::connect(paths.socket.as_std_path()) {
            Ok(mut stream) => match handshake(&mut stream) {
                Ok(()) => return Ok(stream),
                // One peer may have accepted then exited during a stale-socket
                // handoff. Retry that complete handshake exactly once.
                Err(error) if !retried_eof && is_handshake_eof(&error) => {
                    retried_eof = true;
                    continue;
                }
                // A listener that accepts but fails our typed handshake is not
                // a center. Re-enter the same under-flock stale-listener
                // arbitration used for an absent socket.
                Err(_) => {
                    try_spawn(paths, executable)?;
                }
            },
            Err(error) if unavailable_socket(&error) => {
                // Re-enter arbitration while waiting: a fresh marker can
                // represent a child that died before binding. The marker and
                // flock make this cheap for all losers and recover it once
                // its PID is no longer live.
                try_spawn(paths, executable)?;
            }
            Err(error) => return Err(io_error(&paths.socket, error)),
        }
        std::thread::sleep(RETRY_DELAY);
    }
    Err(protocol_error(format!(
        "could not connect to {} within bounded retry",
        paths.socket
    )))
}

// A peer can close before sending any bytes or after a partial JSON line. Both
// are one failed handshake, not two different protocol failures.
fn is_handshake_eof(error: &crate::Error) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("unexpected eof")
        || message.contains("unterminated line")
        || message.contains("connection reset")
        || message.contains("broken pipe")
        || message.contains("not connected")
        // Darwin can report EINVAL when a UnixStream peer closes while the
        // caller is changing socket mode for the bounded handshake read.
        // At this boundary it has the same one-retry meaning as a reset.
        || message.contains("invalid argument")
}

fn is_handshake_timeout(error: &crate::Error) -> bool {
    error.to_string().to_ascii_lowercase().contains("timed out")
}

fn try_spawn(paths: &CenterPaths, executable: &std::path::Path) -> crate::Result<()> {
    let lock = crate::file_lock::open_lock_file_no_follow(&paths.spawn_lock)
        .map_err(|source| io_error(&paths.spawn_lock, source))?;
    if !crate::file_lock::try_lock_exclusive(&lock)
        .map_err(|source| io_error(&paths.spawn_lock, source))?
    {
        return Ok(());
    }
    if prepare_spawn(paths, STREAM_TIMEOUT)? {
        return Ok(());
    }
    let mut child = spawn_center(paths, executable)?;
    // Keep the arbitration lock through listener readiness. Otherwise a second
    // caller can acquire it between spawn_detached returning and the child
    // binding its socket, then launch a duplicate center.
    let deadline = Instant::now() + SPAWN_READY_TIMEOUT;
    while Instant::now() < deadline {
        let child_status = match child.try_wait() {
            Ok(status) => status,
            // Do not relinquish arbitration merely because observing the child
            // failed. It may still bind after another contender starts.
            Err(source) => {
                return abort_spawn(child, lock, io_error(&paths.socket, source));
            }
        };
        if let Some(status) = child_status {
            let log = std::fs::read_to_string(paths.socket.with_extension("log").as_std_path())
                .unwrap_or_default();
            return Err(protocol_error(format!(
                "spawned center exited before readiness with {status}: {log}"
            )));
        }
        match UnixStream::connect(paths.socket.as_std_path()) {
            Ok(mut stream) => match handshake(&mut stream) {
                Ok(()) => return Ok(()),
                Err(_) => {
                    return abort_spawn(
                        child,
                        lock,
                        protocol_error("spawned center did not complete the handshake"),
                    );
                }
            },
            Err(error) if unavailable_socket(&error) => std::thread::sleep(RETRY_DELAY),
            Err(error) => return abort_spawn(child, lock, io_error(&paths.socket, error)),
        }
    }
    // Never drop the arbitration lock while a timed-out detached child is
    // still alive: that would permit a later caller to spawn a second center
    // before the slow child binds. Terminate and reap it before releasing the
    // lock; its possible socket pathname is then recovered by the next winner.
    abort_spawn(
        child,
        lock,
        protocol_error(format!(
            "spawned center did not become ready at {} within {:?}",
            paths.socket, SPAWN_READY_TIMEOUT
        )),
    )
}

fn try_spawn_detached(paths: &CenterPaths, executable: &std::path::Path) -> crate::Result<()> {
    let lock = crate::file_lock::open_lock_file_no_follow(&paths.spawn_lock)
        .map_err(|source| io_error(&paths.spawn_lock, source))?;
    if !crate::file_lock::try_lock_exclusive(&lock)
        .map_err(|source| io_error(&paths.spawn_lock, source))?
    {
        return Ok(());
    }
    if prepare_spawn(paths, Duration::from_millis(25))? {
        return Ok(());
    }
    let marker = spawn_marker_path(paths);
    let child = spawn_center(paths, executable)?;
    // Retain launch ownership after releasing this flock. The child removes
    // the short lease only after publishing its listener; contenders then see
    // either this live marker or a usable socket, never a fork-to-bind gap.
    if let Err(source) = std::fs::write(
        marker.as_std_path(),
        format!("{} {}\n", child.id(), epoch_millis()),
    ) {
        // Do not release spawn arbitration with an unmarked child still able
        // to bind. A later caller would otherwise launch a second center in
        // the fork-to-bind interval.
        return abort_spawn(child, lock, io_error(&marker, source));
    }
    Ok(())
}

/// Check every condition shared by blocking and driver startup while holding
/// spawn arbitration. An accepting but unverified owned socket is stale, not a
/// center; recovering it here prevents the two launch paths from diverging.
fn prepare_spawn(paths: &CenterPaths, handshake_timeout: Duration) -> crate::Result<bool> {
    match UnixStream::connect(paths.socket.as_std_path()) {
        Ok(mut stream) => {
            match handshake_with_timeout(&mut stream, handshake_timeout) {
                Ok(()) => return Ok(true),
                // Driver startup uses a deliberately short handshake deadline
                // so it never waits for readiness. A timeout from an already
                // connected socket is inconclusive: a real center may simply
                // be scheduled late. Never unlink that live pathname or fork
                // a competing center while its owner is unknown.
                Err(error)
                    if handshake_timeout < STREAM_TIMEOUT && is_handshake_timeout(&error) =>
                {
                    return Ok(true);
                }
                Err(_) => {}
            }
        }
        Err(error) if !unavailable_socket(&error) => return Err(io_error(&paths.socket, error)),
        Err(_) => {}
    }
    let marker = spawn_marker_path(paths);
    if fresh_live_spawn_marker(&marker) {
        return Ok(true);
    }
    let _ = std::fs::remove_file(marker.as_std_path());
    if let Ok(metadata) = std::fs::symlink_metadata(paths.socket.as_std_path()) {
        if metadata.file_type().is_socket() && metadata.uid() == unsafe { libc::getuid() } {
            std::fs::remove_file(paths.socket.as_std_path())
                .map_err(|source| io_error(&paths.socket, source))?;
        } else {
            return Err(protocol_error(format!(
                "refusing to remove unowned non-socket {}",
                paths.socket
            )));
        }
    }
    Ok(false)
}

fn spawn_center(
    paths: &CenterPaths,
    executable: &std::path::Path,
) -> crate::Result<std::process::Child> {
    let exe = Utf8PathBuf::from_path_buf(executable.to_path_buf())
        .map_err(|path| protocol_error(format!("non-UTF-8 executable {}", path.display())))?;
    let cwd = Utf8PathBuf::from_path_buf(
        std::env::current_dir().map_err(|source| io_error(&paths.socket, source))?,
    )
    .map_err(|path| protocol_error(format!("non-UTF-8 current directory {}", path.display())))?;
    let log = paths.socket.with_extension("log");
    crate::process::spawn_detached(
        &exe,
        &[CENTER_PROCESS_SENTINEL.to_string()],
        &cwd,
        &log,
        &[
            ("CTX_CENTER_SOCKET", paths.socket.as_str()),
            ("CTX_CENTER_SPAWN_LOCK", paths.spawn_lock.as_str()),
            ("CTX_CENTER_RUNS_ROOT", paths.runs_root.as_str()),
            ("CTX_CENTER_INDEX", paths.index.as_str()),
        ],
    )
}

fn spawn_marker_path(paths: &CenterPaths) -> Utf8PathBuf {
    paths.socket.with_extension("spawn")
}

fn epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn fresh_live_spawn_marker(marker: &Utf8Path) -> bool {
    let Ok(text) = std::fs::read_to_string(marker.as_std_path()) else {
        return false;
    };
    let mut fields = text.split_whitespace();
    let (Some(pid), Some(then)) = (fields.next(), fields.next()) else {
        return false;
    };
    let (Ok(pid), Ok(then)) = (pid.parse::<i32>(), then.parse::<u128>()) else {
        return false;
    };
    epoch_millis().saturating_sub(then) < SPAWN_READY_TIMEOUT.as_millis()
        && unsafe { libc::kill(pid, 0) == 0 }
}

fn unavailable_socket(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound
            | std::io::ErrorKind::ConnectionRefused
            // Darwin reports this while a just-accepted AF_UNIX listener is
            // unlinked, before the pathname lookup observes NotFound.
            | std::io::ErrorKind::InvalidInput
            | std::io::ErrorKind::AddrNotAvailable
    )
}

trait SpawnControl: Send + 'static {
    fn kill(&mut self) -> std::io::Result<()>;
    fn is_exited(&mut self) -> std::io::Result<bool>;
}

impl SpawnControl for std::process::Child {
    fn kill(&mut self) -> std::io::Result<()> {
        std::process::Child::kill(self)
    }

    fn is_exited(&mut self) -> std::io::Result<bool> {
        std::process::Child::try_wait(self).map(|status| status.is_some())
    }
}

/// Reap a child after a successfully delivered SIGKILL without extending the
/// spawn lease. Signal delivery means it can no longer publish the listener;
/// reaping is only resource cleanup and must not make the caller wait.
fn reap_killed_child<C: SpawnControl>(mut child: C) {
    let deadline = Instant::now() + SPAWN_ABORT_TIMEOUT;
    while Instant::now() < deadline {
        match child.is_exited() {
            Ok(true) | Err(_) => return,
            Ok(false) => std::thread::sleep(RETRY_DELAY),
        }
    }
}

/// Do not release spawn arbitration while a child whose termination could not
/// be confirmed might still publish this attempt's listener. This is the one
/// intentionally fail-closed case; the process exit closes the descriptor.
fn retain_unconfirmed_spawn<C: SpawnControl>(child: C, lock: std::fs::File) {
    std::mem::forget((child, lock));
}

/// Abort a spawn attempt without allowing a possibly surviving child to race a
/// later launcher. A successful SIGKILL releases the lease immediately: the
/// child cannot publish after accepting that signal, even if bounded reaping
/// subsequently fails. If termination cannot be confirmed, retain the lease
/// fail-closed for this process lifetime.
fn abort_spawn<C: SpawnControl>(
    mut child: C,
    lock: std::fs::File,
    failure: crate::Error,
) -> crate::Result<()> {
    if child.kill().is_ok() {
        drop(lock);
        std::thread::spawn(move || reap_killed_child(child));
    } else if child.is_exited().unwrap_or(false) {
        drop(lock);
    } else {
        retain_unconfirmed_spawn(child, lock);
    }
    Err(failure)
}

#[derive(Debug, Clone)]
struct CenterRow {
    summary: crate::run_summary::RunSummary,
    /// Parsed only during reconciliation and retained so serving queries never
    /// reopens the authoritative ledger.
    session: Option<ctx_traits_core::procedure::session::Session>,
    repo_key: String,
    repo_path: String,
    ledger_path: Utf8PathBuf,
    modified: SystemTime,
    size: u64,
    live_holder: Option<crate::run_control::DriverHolder>,
    live: bool,
}

struct CenterModel {
    rows: HashMap<Utf8PathBuf, CenterRow>,
    db: Connection,
    subscribers: HashMap<u64, Subscriber>,
    // An unverified row is deliberately treated as live for idle purposes:
    // cache data is derived, while an unprobeable driver lock is authoritative.
    uncertain: bool,
}

struct Subscriber {
    repo_key: Option<String>,
    outbound: mpsc::SyncSender<Outbound>,
    // The writer pulls the next row only after it has written the preceding
    // message, so a large snapshot cannot occupy one queue allocation.
    snapshot: Option<VecDeque<CenterPublicRow>>,
    // Deltas racing a streamed snapshot must follow SnapshotEnd. This remains
    // bounded so a peer that cannot consume the snapshot cannot retain model
    // state indefinitely.
    pending_deltas: VecDeque<CenterDelta>,
}

#[derive(Debug)]
enum Outbound {
    Delta(CenterDelta),
    SnapshotStart(String),
    SnapshotRow(Box<CenterPublicRow>),
    SnapshotEnd,
}

impl CenterModel {
    fn cache_error(paths: &CenterPaths, source: impl std::fmt::Display) -> crate::Error {
        protocol_error(format!(
            "center index {} is invalid: {source}; remove {} to rebuild",
            paths.index, paths.index
        ))
    }

    fn open(paths: &CenterPaths) -> crate::Result<Self> {
        std::fs::create_dir_all(paths.runs_root.as_std_path())
            .map_err(|source| io_error(&paths.runs_root, source))?;
        let db = Connection::open(paths.index.as_std_path())
            .map_err(|source| Self::cache_error(paths, format!("open failed: {source}")))?;
        db.busy_timeout(Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))
            .map_err(|source| {
                Self::cache_error(paths, format!("configure busy timeout: {source}"))
            })?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS center_meta (version INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS center_projection_meta (version INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS center_rows (ledger TEXT PRIMARY KEY, repo_key TEXT NOT NULL, repo_path TEXT NOT NULL, mtime_secs INTEGER NOT NULL, mtime_nanos INTEGER NOT NULL, size INTEGER NOT NULL, summary TEXT NOT NULL); CREATE TABLE IF NOT EXISTS center_sessions (ledger TEXT PRIMARY KEY, mtime_secs INTEGER NOT NULL, mtime_nanos INTEGER NOT NULL, size INTEGER NOT NULL, session TEXT NOT NULL);")
            .map_err(|source| Self::cache_error(paths, format!("initialize schema: {source}")))?;
        let version: Option<i64> = db
            .query_row("SELECT version FROM center_meta LIMIT 1", [], |r| r.get(0))
            .optional()
            .map_err(|source| Self::cache_error(paths, format!("read schema: {source}")))?;
        match version {
            None => {
                db.execute(
                    "INSERT INTO center_meta(version) VALUES (?1)",
                    [CENTER_SCHEMA_VERSION],
                )
                .map_err(|source| Self::cache_error(paths, format!("write schema: {source}")))?;
            }
            Some(CENTER_SCHEMA_VERSION) => {}
            // An earlier 0243.4 build used this shared marker for the widened
            // JSON projection. Downgrade that marker in place: table layout is
            // unchanged and projection_version below retains the v2 meaning.
            Some(CENTER_PROJECTION_VERSION) => {
                db.execute(
                    "UPDATE center_meta SET version = ?1",
                    [CENTER_SCHEMA_VERSION],
                )
                .map_err(|source| {
                    Self::cache_error(paths, format!("restore legacy schema marker: {source}"))
                })?;
            }
            Some(_) => return Err(Self::cache_error(paths, "unsupported legacy schema")),
        }
        let projection_version: Option<i64> = db
            .query_row(
                "SELECT version FROM center_projection_meta LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|source| {
                Self::cache_error(paths, format!("read projection schema: {source}"))
            })?;
        match projection_version {
            None => {
                // A v1 index has no projection marker. Its cached full session
                // rows are enough to reproject below, without changing the
                // marker that a concurrently running v1 center requires.
                db.execute(
                    "INSERT INTO center_projection_meta(version) VALUES (?1)",
                    [CENTER_PROJECTION_VERSION],
                )
                .map_err(|source| {
                    Self::cache_error(paths, format!("write projection schema: {source}"))
                })?;
            }
            Some(CENTER_PROJECTION_VERSION) => {}
            Some(_) => {
                // Projection rows are derived. This marker is deliberately
                // separate from center_meta so this rebuild never bricks v1.
                db.execute_batch("DELETE FROM center_sessions; DELETE FROM center_rows; DELETE FROM center_projection_meta;")
                    .map_err(|source| Self::cache_error(paths, format!("rebuild projection: {source}")))?;
                db.execute(
                    "INSERT INTO center_projection_meta(version) VALUES (?1)",
                    [CENTER_PROJECTION_VERSION],
                )
                .map_err(|source| {
                    Self::cache_error(paths, format!("write rebuilt projection schema: {source}"))
                })?;
            }
        }
        let mut rows = HashMap::new();
        let mut statement = db.prepare("SELECT ledger, repo_key, repo_path, mtime_secs, mtime_nanos, size, summary FROM center_rows").map_err(|source| Self::cache_error(paths, format!("prepare rows: {source}")))?;
        let cached = statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, String>(6)?,
                ))
            })
            .map_err(|source| Self::cache_error(paths, format!("read rows: {source}")))?;
        for item in cached {
            let (ledger, repo_key, repo_path, secs, nanos, size, summary) =
                item.map_err(|source| Self::cache_error(paths, format!("read row: {source}")))?;
            let mut summary = serde_json::from_str(&summary)
                .map_err(|source| Self::cache_error(paths, format!("decode summary: {source}")))?;
            let secs = u64::try_from(secs)
                .map_err(|_| Self::cache_error(paths, "negative mtime seconds"))?;
            let nanos = u32::try_from(nanos)
                .map_err(|_| Self::cache_error(paths, "invalid mtime nanoseconds"))?;
            if nanos >= 1_000_000_000 {
                return Err(Self::cache_error(paths, "invalid mtime nanoseconds"));
            }
            let size = u64::try_from(size)
                .map_err(|_| Self::cache_error(paths, "negative ledger size"))?;
            let modified = UNIX_EPOCH
                .checked_add(Duration::new(secs, nanos))
                .ok_or_else(|| Self::cache_error(paths, "overflowing mtime"))?;
            let session: Option<String> = db.query_row("SELECT session FROM center_sessions WHERE ledger = ?1 AND mtime_secs = ?2 AND mtime_nanos = ?3 AND size = ?4", params![ledger, secs as i64, nanos as i64, size as i64], |row| row.get(0)).optional().map_err(|source| Self::cache_error(paths, format!("read session: {source}")))?;
            let session = session.and_then(|text| serde_json::from_str(&text).ok());
            // Summary fields grow independently of the shared SQLite schema.
            // A preceding center may have written a valid, defaulted summary;
            // when its matching cached ledger is available, rebuild the current
            // projection rather than serving those defaults until a file changes.
            if let Some(session) = &session {
                summary = crate::run_summary::RunSummary::from_session(session);
                if summary.title.is_none() {
                    summary.title =
                        crate::activity_sidecar::read_session_title(Utf8Path::new(&ledger));
                }
            }
            rows.insert(
                Utf8PathBuf::from(ledger.clone()),
                CenterRow {
                    summary,
                    session,
                    repo_key,
                    repo_path,
                    ledger_path: Utf8PathBuf::from(ledger),
                    modified,
                    size,
                    live_holder: None,
                    live: false,
                },
            );
        }
        drop(statement);
        Ok(Self {
            rows,
            db,
            subscribers: HashMap::new(),
            uncertain: false,
        })
    }

    fn discover(&mut self, paths: &CenterPaths) -> crate::Result<()> {
        let before: HashMap<_, _> = self
            .rows
            .iter()
            .map(|(ledger, row)| (ledger.clone(), public_row(row)))
            .collect();
        self.discover_inner(paths)?;
        self.broadcast_row_changes(before);
        Ok(())
    }

    /// Refresh one authoritative ledger after a driver notification. Periodic
    /// discovery still repairs dropped notifications, but a healthy driver must
    /// not make every frame enumerate every repository on the machine.
    fn refresh_ledger(&mut self, paths: &CenterPaths, ledger: &Utf8Path) -> crate::Result<()> {
        let repo_paths = match crate::state::read_repo_index() {
            Ok(repos) => Some(repo_paths(repos)),
            Err(_) => {
                self.uncertain = true;
                None
            }
        };
        self.refresh_ledger_inner(paths, ledger, true, repo_paths.as_ref())
    }

    /// A final outcome refresh changes a retained row. `Ended` is reserved for
    /// a ledger that has actually disappeared, so subscribers can distinguish
    /// completion from deletion without consulting the store.
    fn refresh_ledger_ended(
        &mut self,
        paths: &CenterPaths,
        ledger: &Utf8Path,
    ) -> crate::Result<()> {
        let before = self.rows.get(ledger).map(public_row);
        let repo_paths = match crate::state::read_repo_index() {
            Ok(repos) => Some(repo_paths(repos)),
            Err(_) => {
                self.uncertain = true;
                None
            }
        };
        self.refresh_ledger_inner(paths, ledger, false, repo_paths.as_ref())?;
        let after = self.rows.get(ledger).map(public_row);
        if before != after {
            match (before, after) {
                (_, Some(row)) => self.broadcast(CenterDelta::RowChanged { row: Box::new(row) }),
                (Some(row), None) => self.broadcast(CenterDelta::Ended { row: Box::new(row) }),
                (None, None) => {}
            }
        }
        Ok(())
    }

    /// Reconcile one ledger through the same parse, liveness, and persistence
    /// path used by driver notifications. Discovery suppresses the per-ledger
    /// emission and publishes one complete before/after diff after its scan.
    fn refresh_ledger_inner(
        &mut self,
        paths: &CenterPaths,
        ledger: &Utf8Path,
        emit: bool,
        repo_paths: Option<&HashMap<String, String>>,
    ) -> crate::Result<()> {
        if !ledger.starts_with(&paths.runs_root) {
            return Err(protocol_error(
                "driver ledger is outside the center runs root",
            ));
        }
        let before = self.rows.get(ledger).map(public_row);
        // A parsed fingerprint is only a candidate until its driver lock has
        // been verified. Retain the complete old entry so a failed probe never
        // becomes queryable or persistable.
        let previous = self.rows.get(ledger).cloned();
        let metadata = match std::fs::metadata(ledger.as_std_path()) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Some(previous) = self.rows.remove(ledger) {
                    if let Err(error) = self.persist() {
                        self.rows.insert(ledger.to_path_buf(), previous);
                        return Err(error);
                    }
                    if emit {
                        self.broadcast(CenterDelta::Ended {
                            row: Box::new(public_row(&previous)),
                        });
                    }
                }
                return Ok(());
            }
            Err(error) => return Err(io_error(ledger, error)),
        };
        let modified = metadata
            .modified()
            .map_err(|source| io_error(ledger, source))?;
        let size = metadata.len();
        run_after_refresh_stat();
        let unchanged = self.rows.get(ledger).is_some_and(|row| {
            row.modified == modified
                && row.size == size
                // A parse error is an accepted cache entry just like a parsed
                // session. Rows with neither are legacy/incomplete and must be
                // rehydrated from the authoritative ledger.
                && (row.session.is_some() || row.summary.parse_error.is_some())
        });
        let repo_key = ledger
            .parent()
            .and_then(Utf8Path::file_name)
            .unwrap_or_default()
            .to_string();
        // Repository discovery is independent of ledger bytes. A scan supplies
        // one index snapshot for all ledgers, while an unavailable index retains
        // the last verified path rather than publishing a false empty value.
        let repo_path = repo_paths
            .and_then(|paths| paths.get(&repo_key).cloned())
            .or_else(|| previous.as_ref().map(|row| row.repo_path.clone()))
            .unwrap_or_default();
        if !unchanged {
            let (summary, session) = match read_session(ledger) {
                Ok(session) => {
                    let mut summary = crate::run_summary::RunSummary::from_session(&session);
                    if summary.title.is_none() {
                        summary.title = crate::activity_sidecar::read_session_title(ledger);
                    }
                    (summary, Some(session))
                }
                Err(error) => {
                    let session_id = ledger
                        .file_stem()
                        .map(str::to_owned)
                        .unwrap_or_else(|| ledger.to_string());
                    (
                        crate::run_summary::RunSummary::unreadable(session_id, error.to_string()),
                        None,
                    )
                }
            };
            // A live driver may replace its atomic ledger between the initial
            // fingerprint and parse, including a parse that fails. Never attach
            // either candidate to an older fingerprint; the next refresh retries it.
            let verified = std::fs::metadata(ledger.as_std_path())
                .map_err(|source| io_error(ledger, source))?;
            let verified_modified = verified
                .modified()
                .map_err(|source| io_error(ledger, source))?;
            if verified_modified != modified || verified.len() != size {
                return Err(protocol_error(
                    "ledger changed while center parsed its refresh candidate",
                ));
            }
            self.rows.insert(
                ledger.to_path_buf(),
                CenterRow {
                    summary,
                    session,
                    repo_key,
                    repo_path,
                    ledger_path: ledger.to_path_buf(),
                    modified,
                    size,
                    live_holder: None,
                    live: false,
                },
            );
        } else if let Some(row) = self.rows.get_mut(ledger) {
            row.repo_key = repo_key;
            row.repo_path = repo_path;
        }
        if let Err(error) = self.refresh_liveness(ledger, unchanged) {
            self.restore_verified_row(ledger, previous);
            return Err(error);
        }
        if let Err(error) = self.persist() {
            self.restore_verified_row(ledger, previous);
            return Err(error);
        }
        let after = self.rows.get(ledger).map(public_row);
        if !emit {
            return Ok(());
        }
        match (before, after) {
            (None, Some(row)) => self.broadcast(CenterDelta::Appeared { row: Box::new(row) }),
            (Some(previous), Some(row)) if previous != row => {
                self.broadcast(CenterDelta::RowChanged { row: Box::new(row) })
            }
            (Some(previous), None) => self.broadcast(CenterDelta::Ended {
                row: Box::new(previous),
            }),
            _ => {}
        }
        Ok(())
    }

    fn restore_verified_row(&mut self, ledger: &Utf8Path, previous: Option<CenterRow>) {
        match previous {
            Some(row) => {
                self.rows.insert(ledger.to_path_buf(), row);
            }
            None => {
                self.rows.remove(ledger);
            }
        }
    }

    fn discover_inner(&mut self, paths: &CenterPaths) -> crate::Result<()> {
        let mut present = HashSet::new();
        let mut unscanned_roots = HashSet::new();
        let mut incomplete_directory_listing = false;
        self.uncertain = false;
        let repo_paths = match crate::state::read_repo_index() {
            Ok(repos) => Some(repo_paths(repos)),
            Err(_) => {
                // The ledger scan remains useful, but repository labels are
                // stale until the index becomes readable again.
                self.uncertain = true;
                None
            }
        };
        let directories = match std::fs::read_dir(paths.runs_root.as_std_path()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.rows.clear();
                return self.persist();
            }
            Err(error) => return Err(io_error(&paths.runs_root, error)),
        };
        for entry in directories {
            let Ok(entry) = entry else {
                self.uncertain = true;
                incomplete_directory_listing = true;
                continue;
            };
            let Ok(kind) = entry.file_type() else {
                self.uncertain = true;
                incomplete_directory_listing = true;
                continue;
            };
            if !kind.is_dir() {
                continue;
            }
            let Some(repo_key) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let root = paths.runs_root.join(&repo_key);
            let ledgers = match crate::run_session::session_store_paths(Some(root.as_str())) {
                Ok(ledgers) => ledgers,
                Err(_) => {
                    self.uncertain = true;
                    unscanned_roots.insert(repo_key);
                    continue;
                }
            };
            for ledger in ledgers {
                // Metadata can fail transiently after enumeration. It is still
                // present and must not be deleted from the last-good model.
                present.insert(ledger.clone());
                // All ledger parsing, lock probing, cache persistence, and
                // row reconstruction is centralized here. A failed refresh
                // leaves the last verified projection in place for retry.
                if self
                    .refresh_ledger_inner(paths, &ledger, false, repo_paths.as_ref())
                    .is_err()
                {
                    self.uncertain = true;
                }
            }
        }
        if !incomplete_directory_listing {
            self.rows.retain(|path, row| {
                present.contains(path) || unscanned_roots.contains(&row.repo_key)
            });
        }
        self.persist()?;
        Ok(())
    }

    fn subscribe(
        &mut self,
        id: u64,
        request_id: String,
        repo_key: Option<String>,
        outbound: mpsc::SyncSender<Outbound>,
    ) -> crate::Result<()> {
        // Snapshot rows are streamed one at a time. Racing deltas are retained
        // in the subscriber's bounded pending queue and released only after
        // the writer confirms SnapshotEnd reached the peer.
        outbound
            .try_send(Outbound::SnapshotStart(request_id.clone()))
            .map_err(|_| protocol_error("subscriber outbound queue is full"))?;
        self.subscribers.insert(
            id,
            Subscriber {
                repo_key: repo_key.clone(),
                outbound,
                snapshot: Some(public_rows(self, repo_key.as_deref()).into()),
                pending_deltas: VecDeque::new(),
            },
        );
        Ok(())
    }

    fn advance_snapshot(&mut self, id: u64) {
        let mut remove = false;
        if let Some(subscriber) = self.subscribers.get_mut(&id) {
            if let Some(snapshot) = subscriber.snapshot.as_mut() {
                let next = match snapshot.pop_front() {
                    Some(row) => Outbound::SnapshotRow(Box::new(row)),
                    None => Outbound::SnapshotEnd,
                };
                let completed = matches!(&next, Outbound::SnapshotEnd);
                if subscriber.outbound.try_send(next).is_err() {
                    remove = true;
                } else if completed {
                    // This credit is sent by the writer only after SnapshotEnd
                    // reaches the peer, so a queued delta follows it on wire.
                    subscriber.snapshot = None;
                    if let Some(delta) = subscriber.pending_deltas.pop_front()
                        && subscriber
                            .outbound
                            .try_send(Outbound::Delta(delta))
                            .is_err()
                    {
                        remove = true;
                    }
                }
            } else if let Some(delta) = subscriber.pending_deltas.pop_front()
                && subscriber
                    .outbound
                    .try_send(Outbound::Delta(delta))
                    .is_err()
            {
                remove = true;
            }
        }
        if remove {
            self.subscribers.remove(&id);
        }
    }

    fn broadcast_row_changes(&mut self, before: HashMap<Utf8PathBuf, CenterPublicRow>) {
        let mut changes = Vec::new();
        for (ledger, row) in &self.rows {
            let public = public_row(row);
            match before.get(ledger) {
                None => changes.push(CenterDelta::Appeared {
                    row: Box::new(public),
                }),
                Some(previous) if previous != &public => changes.push(CenterDelta::RowChanged {
                    row: Box::new(public),
                }),
                _ => {}
            }
        }
        for (ledger, row) in before {
            if !self.rows.contains_key(&ledger) {
                changes.push(CenterDelta::Ended { row: Box::new(row) });
            }
        }
        for message in changes {
            self.broadcast(message);
        }
    }

    fn broadcast_activity(
        &mut self,
        ledger_path: &str,
        activity: crate::activity_sidecar::ActivityRecord,
    ) {
        let ledger_path = Utf8PathBuf::from(ledger_path);
        if let crate::activity_sidecar::ActivityRecord::SessionTitle { title, .. } = &activity
            && let Some(row) = self.rows.get_mut(&ledger_path)
        {
            // Title generation is durable in the activity stream before the next
            // ledger frame. Keep the projection current without reopening it.
            row.summary.title = Some(title.clone());
            let public = public_row(row);
            // Keep activity-stream consumers compatible while center-row
            // consumers observe the title through the following row update.
            self.broadcast(CenterDelta::ActivityLine {
                row: Box::new(public.clone()),
                activity,
            });
            self.broadcast(CenterDelta::RowChanged {
                row: Box::new(public),
            });
            return;
        }
        if let Some(row) = self.rows.get(&ledger_path) {
            self.broadcast(CenterDelta::ActivityLine {
                row: Box::new(public_row(row)),
                activity,
            });
        }
    }

    fn broadcast(&mut self, message: CenterDelta) {
        let repo_key = match &message {
            CenterDelta::Appeared { row }
            | CenterDelta::RowChanged { row }
            | CenterDelta::Ended { row }
            | CenterDelta::ActivityLine { row, .. } => row.repo_key.as_str(),
        };
        self.subscribers.retain(|_, subscriber| {
            // A repository-scoped subscriber stays registered when an update
            // belongs to another repository; it simply does not receive it.
            // Removing it here made unrelated activity silently terminate a
            // persistent subscription.
            if !subscriber
                .repo_key
                .as_deref()
                .is_none_or(|scope| scope == repo_key)
            {
                return true;
            }
            if subscriber.snapshot.is_some() {
                if subscriber.pending_deltas.len() == SUBSCRIBER_QUEUE {
                    return false;
                }
                subscriber.pending_deltas.push_back(message.clone());
                return true;
            }
            subscriber
                .outbound
                .try_send(Outbound::Delta(message.clone()))
                .is_ok()
        });
    }

    fn refresh_liveness(&mut self, ledger: &Utf8Path, unchanged: bool) -> crate::Result<()> {
        let probe = crate::run_control::probe(ledger)?;
        self.refresh_after_probe(ledger, unchanged, probe)
    }

    fn refresh_after_probe(
        &mut self,
        ledger: &Utf8Path,
        unchanged: bool,
        probe: crate::run_control::DriverProbe,
    ) -> crate::Result<()> {
        let Some(row) = self.rows.get_mut(ledger) else {
            return Ok(());
        };
        match probe {
            crate::run_control::DriverProbe::Held(holder) => {
                // The kernel lock remains authoritative even if the ledger has
                // already reached a terminal state. A finishing driver must
                // prevent idle exit until it has released the lock.
                row.live = true;
                row.live_holder = holder.clone();
                // Discovery is also the adoption path for drivers that began
                // before registration. Publish the same liveness row a driver
                // publishes at start, so removing dashboard sweeps does not
                // make `internal running` forget a held ledger.
                if let (Some(holder), Some(session)) = (holder.as_ref(), row.session.as_ref()) {
                    let facts = crate::run_liveness::LiveRunFacts {
                        session_id: row.summary.session_id.clone(),
                        run_id: row.summary.run_id.clone(),
                        repo_key: row.repo_key.clone(),
                        repo_path: row.repo_path.clone(),
                        ledger_path: row.ledger_path.clone(),
                        worktree_path: session
                            .provenance
                            .worktree
                            .as_ref()
                            .and_then(|worktree| worktree.path.clone()),
                        branch: session
                            .provenance
                            .worktree
                            .as_ref()
                            .map(|worktree| worktree.branch.clone()),
                        log_path: None,
                    };
                    let _ = crate::run_liveness::upsert_row(
                        &crate::run_control::runtime_root(),
                        &facts,
                        holder.pid,
                        holder.started_at_epoch_secs,
                    );
                }
            }
            crate::run_control::DriverProbe::Unheld { .. } => {
                match crate::run_control::try_acquire_maintenance(ledger)? {
                    None => {
                        row.live = true;
                        row.live_holder = None;
                    }
                    Some(mut maintenance) => {
                        // A terminal summary with an unchanged fingerprint is
                        // already a complete ledger projection. It still needs
                        // maintenance ownership to clear stale holder metadata.
                        // Re-stat under that ownership before trusting it: a driver
                        // may have rewritten the ledger after discovery's first
                        // stat and before releasing its lock.
                        let fingerprint_matches_under_maintenance =
                            std::fs::metadata(ledger.as_std_path())
                                .and_then(|metadata| {
                                    metadata.modified().map(|modified| {
                                        modified == row.modified && metadata.len() == row.size
                                    })
                                })
                                .map_err(|source| io_error(ledger, source))?;
                        let unchanged_under_maintenance =
                            unchanged && fingerprint_matches_under_maintenance;
                        if unchanged_under_maintenance && terminal(&row.summary) {
                            maintenance.clear_stale_metadata()?;
                            row.live = false;
                            row.live_holder = None;
                            return Ok(());
                        }
                        // The candidate was parsed once before probing. If the
                        // fingerprint changed while acquiring maintenance,
                        // retain the last verified model and retry later rather
                        // than parsing multiple racing ledger versions.
                        if !fingerprint_matches_under_maintenance {
                            return Err(protocol_error(
                                "ledger changed while acquiring center maintenance lock",
                            ));
                        }
                        // A corrupt ledger remains a visible, non-live row. It cannot
                        // be repaired without parsed session data, but must still flow
                        // through the same probe/persist/delta transaction as rows that
                        // can be parsed.
                        let Some(mut session) = row.session.clone() else {
                            row.live = false;
                            row.live_holder = None;
                            return Ok(());
                        };
                        let summary = crate::run_summary::RunSummary::from_session(&session);
                        if !terminal(&summary) {
                            crate::run_session::record_interrupted_outcome_in_session(
                                ledger,
                                &mut session,
                            )?;
                        }
                        // Clear stale display metadata for both terminal and repaired
                        // ledgers while maintenance ownership still excludes a driver.
                        maintenance.clear_stale_metadata()?;
                        let summary = crate::run_summary::RunSummary::from_session(&session);
                        let metadata = std::fs::metadata(ledger.as_std_path())
                            .map_err(|source| io_error(ledger, source))?;
                        let modified = metadata
                            .modified()
                            .map_err(|source| io_error(ledger, source))?;
                        row.summary = summary;
                        row.session = Some(session);
                        row.modified = modified;
                        row.size = metadata.len();
                        row.live = false;
                        row.live_holder = None;
                    }
                }
            }
        }
        Ok(())
    }

    fn persist(&mut self) -> crate::Result<()> {
        let transaction = self
            .db
            .transaction()
            .map_err(|source| protocol_error(source.to_string()))?;
        transaction
            .execute("DELETE FROM center_rows", [])
            .map_err(|source| protocol_error(source.to_string()))?;
        for row in self.rows.values() {
            let elapsed = row.modified.duration_since(UNIX_EPOCH).map_err(|_| {
                protocol_error(format!(
                    "center row {} predates UNIX_EPOCH",
                    row.ledger_path
                ))
            })?;
            let summary = serde_json::to_string(&row.summary).map_err(|source| {
                crate::parse::Error::JsonSerialize {
                    context: "serialize center summary".to_string(),
                    source,
                }
            })?;
            transaction
                .execute(
                    "INSERT INTO center_rows VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        row.ledger_path.as_str(),
                        row.repo_key,
                        row.repo_path,
                        i64::try_from(elapsed.as_secs())
                            .map_err(|_| protocol_error("mtime overflows SQLite"))?,
                        elapsed.subsec_nanos() as i64,
                        i64::try_from(row.size)
                            .map_err(|_| protocol_error("ledger size overflows SQLite"))?,
                        summary
                    ],
                )
                .map_err(|source| protocol_error(source.to_string()))?;
            if let Some(session) = &row.session {
                let serialized = serde_json::to_string(session).map_err(|source| {
                    crate::parse::Error::JsonSerialize {
                        context: "serialize cached center session".to_string(),
                        source,
                    }
                })?;
                transaction
                    .execute(
                        "INSERT OR REPLACE INTO center_sessions VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![
                            row.ledger_path.as_str(),
                            i64::try_from(elapsed.as_secs())
                                .map_err(|_| protocol_error("mtime overflows SQLite"))?,
                            elapsed.subsec_nanos() as i64,
                            i64::try_from(row.size)
                                .map_err(|_| protocol_error("ledger size overflows SQLite"))?,
                            serialized
                        ],
                    )
                    .map_err(|source| protocol_error(source.to_string()))?;
            }
        }
        // `center_sessions` is an additive cache table, so older center
        // versions do not depend on it. Keep it bounded to the ledgers that
        // survived this model-owned rebuild rather than retaining stale full
        // sessions forever after retention deletes their ledger rows.
        transaction
            .execute(
                "DELETE FROM center_sessions WHERE NOT EXISTS (SELECT 1 FROM center_rows WHERE center_rows.ledger = center_sessions.ledger)",
                [],
            )
            .map_err(|source| protocol_error(source.to_string()))?;
        transaction
            .commit()
            .map_err(|source| protocol_error(source.to_string()))?;
        Ok(())
    }

    fn has_live(&self) -> bool {
        self.uncertain || !self.subscribers.is_empty() || self.rows.values().any(|row| row.live)
    }
}

fn terminal(summary: &crate::run_summary::RunSummary) -> bool {
    summary
        .session_state
        .is_some_and(ctx_traits_core::procedure::activity::SessionState::is_terminal)
        || matches!(
            summary.status,
            ctx_traits_core::procedure::session::Status::Completed
                | ctx_traits_core::procedure::session::Status::Failed
        )
}

struct SocketGuard {
    path: Utf8PathBuf,
    device: u64,
    inode: u64,
    uid: u32,
}

impl SocketGuard {
    fn for_listener(path: Utf8PathBuf) -> crate::Result<Self> {
        let metadata = std::fs::symlink_metadata(path.as_std_path())
            .map_err(|source| io_error(&path, source))?;
        Ok(Self {
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
            uid: metadata.uid(),
        })
    }
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let Ok(metadata) = std::fs::symlink_metadata(self.path.as_std_path()) else {
            return;
        };
        if metadata.file_type().is_socket()
            && metadata.uid() == self.uid
            && metadata.dev() == self.device
            && metadata.ino() == self.inode
        {
            let _ = std::fs::remove_file(self.path.as_std_path());
        }
    }
}

/// Entry point for the private sentinel. It intentionally has no command API:
/// accepted connections only prove that the shared center is alive.
pub fn run_server() -> crate::Result<()> {
    let configured_duration = |name: &str, default: Duration| -> crate::Result<Duration> {
        match std::env::var(name) {
            Ok(value) => value
                .parse::<u64>()
                .map(Duration::from_millis)
                .map_err(|_| protocol_error(format!("invalid private {name}: {value}"))),
            Err(std::env::VarError::NotPresent) => Ok(default),
            Err(error) => Err(protocol_error(format!("read private {name}: {error}"))),
        }
    };
    let idle = configured_duration("CTX_CENTER_IDLE_MS", IDLE_TIMEOUT)?;
    let scan_interval = configured_duration("CTX_CENTER_SCAN_MS", SCAN_INTERVAL)?;
    run_server_at(center_paths()?, idle, scan_interval)
}

fn run_server_at(paths: CenterPaths, idle: Duration, scan_interval: Duration) -> crate::Result<()> {
    if let Ok(marker) = std::env::var("CTX_CENTER_LAUNCH_MARKER") {
        // Private test instrumentation: each sentinel process records exactly
        // one launch before it can publish a listener.
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(marker)
            .and_then(|mut file| file.write_all(b"center\n"))
            .map_err(|source| io_error(&paths.socket, source))?;
    }
    if paths.socket.exists() {
        return Err(protocol_error(format!(
            "center socket already exists: {}",
            paths.socket
        )));
    }
    // Do the potentially slow reconstruction before publishing the listener.
    // Spawn arbitration uses a successful handshake as its readiness signal,
    // so a bound socket must always be able to service that handshake.
    let mut model = CenterModel::open(&paths)?;
    if model.discover(&paths).is_err() {
        // The disk cache is derived, but a failed scan cannot prove that a
        // driver disappeared. Keep the process alive and retry on cadence.
        model.uncertain = true;
    }
    let listener = UnixListener::bind(paths.socket.as_std_path())
        .map_err(|source| io_error(&paths.socket, source))?;
    let _guard = SocketGuard::for_listener(paths.socket.clone())?;
    let _ = std::fs::remove_file(spawn_marker_path(&paths).as_std_path());
    listener
        .set_nonblocking(true)
        .map_err(|source| io_error(&paths.socket, source))?;
    let mut last_work = Instant::now();
    let mut last_scan = Instant::now();
    // Connection workers own every potentially slow socket operation. Jobs are
    // executed here, on the sole owner of both the model and SQLite handle.
    let (jobs, job_receiver) = mpsc::sync_channel::<ModelCommand>(MODEL_QUEUE);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let jobs = jobs.clone();
                std::thread::spawn(move || serve_connection_worker(stream, jobs));
                last_work = Instant::now();
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(io_error(&paths.socket, error)),
        }
        while let Ok(job) = job_receiver.try_recv() {
            match job {
                ModelCommand::Request { request, reply } => {
                    // Queries take a center-owned freshness barrier before
                    // reading the model, including unnotified store changes.
                    if snapshot_request(&request)
                        && let Err(error) = model.discover(&paths)
                    {
                        let _ = reply.send(Err(error));
                        continue;
                    }
                    let _ = reply.send(handle_request(&mut model, &paths, request));
                }
                ModelCommand::Subscribe {
                    id,
                    request_id,
                    repo_key,
                    outbound,
                    reply,
                } => {
                    let _ = reply.send(model.subscribe(id, request_id, repo_key, outbound));
                }
                ModelCommand::Unsubscribe { id } => {
                    model.subscribers.remove(&id);
                }
                ModelCommand::SnapshotNext { id } => model.advance_snapshot(id),
            }
            last_work = Instant::now();
        }
        if last_scan.elapsed() >= scan_interval {
            if model.discover(&paths).is_err() {
                model.uncertain = true;
            }
            last_scan = Instant::now();
        }
        if last_work.elapsed() >= idle {
            if model.discover(&paths).is_err() {
                model.uncertain = true;
            }
            if !model.has_live() {
                return Ok(());
            }
            last_work = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

enum ModelCommand {
    Request {
        request: Request,
        reply: mpsc::SyncSender<crate::Result<ResponseResult>>,
    },
    Subscribe {
        id: u64,
        request_id: String,
        repo_key: Option<String>,
        outbound: mpsc::SyncSender<Outbound>,
        reply: mpsc::SyncSender<crate::Result<()>>,
    },
    Unsubscribe {
        id: u64,
    },
    SnapshotNext {
        id: u64,
    },
}

fn serve_handshake(stream: &mut UnixStream) -> crate::Result<()> {
    stream
        .set_read_timeout(Some(STREAM_TIMEOUT))
        .map_err(|source| io_error(&Utf8PathBuf::from("center socket"), source))?;
    stream
        .set_write_timeout(Some(STREAM_TIMEOUT))
        .map_err(|source| io_error(&Utf8PathBuf::from("center socket"), source))?;
    let hello: WireMessage = serde_json::from_slice(&read_line(stream)?).map_err(|source| {
        crate::parse::Error::JsonDeserialize {
            context: "decode center hello line".to_string(),
            source,
        }
    })?;
    match hello {
        WireMessage::Hello { id } => write_line(stream, &WireMessage::Ready { id }),
        _ => Err(protocol_error("expected hello")),
    }
}

fn response(stream: &mut UnixStream, id: String, result: ResponseResult) -> crate::Result<()> {
    write_line(stream, &WireMessage::Response { id, result })
}

/// Serve a bounded request stream. All state changes use the existing single
/// model refresh path (`discover`), so a dropped driver event is repaired by
/// the periodic scan rather than creating a second interpretation of ledgers.
fn serve_connection_worker(mut stream: UnixStream, jobs: mpsc::SyncSender<ModelCommand>) {
    if serve_handshake(&mut stream).is_err() {
        return;
    }
    let mut persistent = false;
    // Registration binds this connection to the metadata atomically written by
    // its DriverLockGuard. Events cannot switch ledgers after that point.
    let mut registered_ledger: Option<String> = None;
    loop {
        // A handshaken peer has not earned an unbounded idle connection until
        // it installs a notifier or subscription. This bounds abandoned query
        // workers without imposing an idle deadline on real persistent peers.
        let line = match read_line_with_idle(&mut stream, persistent) {
            Ok(line) => line,
            Err(_) => return,
        };
        let request: Request = match serde_json::from_slice(&line) {
            Ok(request) => request,
            Err(_) => return,
        };
        let id = request.id().to_owned();
        let notification = request.notification_ledger().is_some();
        if notification && registered_ledger.is_none() {
            let _ = response(
                &mut stream,
                id,
                ResponseResult::Error {
                    message: "notification requires register".to_string(),
                },
            );
            return;
        }
        if notification && request.notification_ledger() != registered_ledger.as_deref() {
            let _ = response(
                &mut stream,
                id,
                ResponseResult::Error {
                    message: "notification ledger differs from register".to_string(),
                },
            );
            return;
        }
        if let Request::Register { registration, .. } = &request
            && registered_ledger
                .as_deref()
                .is_some_and(|ledger| ledger != registration.ledger_path)
        {
            let _ = response(
                &mut stream,
                id,
                ResponseResult::Error {
                    message: "connection is already registered".to_string(),
                },
            );
            return;
        }
        if let Request::Subscribe {
            id: request_id,
            repo_key,
        } = &request
        {
            let (reply_sender, reply_receiver) = mpsc::sync_channel::<crate::Result<()>>(1);
            let (outbound, receiver) = mpsc::sync_channel(SUBSCRIBER_QUEUE);
            let id = NEXT_SUBSCRIBER.fetch_add(1, Ordering::Relaxed);
            // Start draining before the owner is asked to publish the
            // snapshot. The cloned descriptor is write-only from this point;
            // the original remains available for the initial request read.
            let mut writer_stream = match stream.try_clone() {
                Ok(writer) => writer,
                Err(_) => return,
            };
            // A peer that stops reading must not retain the writer forever in
            // one blocking write after the model has evicted its queue.
            if writer_stream
                .set_write_timeout(Some(STREAM_TIMEOUT))
                .is_err()
            {
                return;
            }
            let cleanup = jobs.clone();
            let snapshot_credit = jobs.clone();
            let writer = std::thread::spawn(move || {
                while let Ok(message) = receiver.recv() {
                    let write = match message {
                        Outbound::Delta(delta) => {
                            write_line(&mut writer_stream, &WireMessage::Delta { delta })
                        }
                        Outbound::SnapshotStart(id) => {
                            write_line(&mut writer_stream, &WireMessage::SnapshotStart { id })
                        }
                        Outbound::SnapshotRow(row) => {
                            write_line(&mut writer_stream, &WireMessage::SnapshotRow { row })
                        }
                        Outbound::SnapshotEnd => {
                            write_line(&mut writer_stream, &WireMessage::SnapshotEnd)
                        }
                    };
                    if write.is_err() {
                        break;
                    }
                    // Only the worker waits for model capacity. The owner
                    // never waits for this socket and emits at most one next
                    // snapshot message for each completed write.
                    if snapshot_credit
                        .send(ModelCommand::SnapshotNext { id })
                        .is_err()
                    {
                        break;
                    }
                }
                // The EOF watcher owns a clone of this descriptor. Explicitly
                // shutting down the socket makes a backpressure eviction
                // observable to the client instead of leaving it on a stale,
                // silent subscription.
                let _ = writer_stream.shutdown(Shutdown::Both);
                // This worker may wait for a congested model queue, but model
                // mutation never waits on this peer's socket I/O.
                let _ = cleanup.send(ModelCommand::Unsubscribe { id });
            });
            if jobs
                .try_send(ModelCommand::Subscribe {
                    id,
                    request_id: request_id.clone(),
                    repo_key: repo_key.clone(),
                    outbound,
                    reply: reply_sender,
                })
                .is_err()
            {
                return;
            }
            if reply_receiver
                .recv_timeout(STREAM_TIMEOUT)
                .ok()
                .and_then(Result::ok)
                .is_none()
            {
                let _ = jobs.send(ModelCommand::Unsubscribe { id });
                return;
            }
            // The writer owns the persistent outbound stream. A separate
            // reader clone reports EOF so an idle closed subscriber is removed
            // even when no subsequent delta is broadcast.
            let mut watcher_stream = match stream.try_clone() {
                Ok(watcher) => watcher,
                Err(_) => return,
            };
            let cleanup = jobs.clone();
            std::thread::spawn(move || {
                let _ = read_line_with_idle(&mut watcher_stream, true);
                let _ = cleanup.send(ModelCommand::Unsubscribe { id });
            });
            drop(writer);
            return;
        }
        let (reply_sender, reply_receiver) = mpsc::sync_channel::<crate::Result<ResponseResult>>(1);
        let registered_path = match &request {
            Request::Register { registration, .. } => Some(registration.ledger_path.clone()),
            _ => None,
        };
        if jobs
            .try_send(ModelCommand::Request {
                request,
                reply: reply_sender,
            })
            .is_err()
        {
            return;
        }
        let result = match reply_receiver.recv_timeout(STREAM_TIMEOUT) {
            Ok(result) => result,
            Err(_) => return,
        };
        let succeeded = result.is_ok();
        let value = result.unwrap_or_else(|error| ResponseResult::Error {
            message: error.to_string(),
        });
        if response(&mut stream, id, value).is_err() {
            return;
        }
        // A notifier is long-lived and may be silent between frames. Keep its
        // connection usable while retaining the absolute deadline for any
        // partial line, so a trickling peer still cannot pin this worker.
        if registered_path.is_some() && succeeded {
            registered_ledger = registered_path;
            persistent = true;
        }
    }
}

fn snapshot_request(request: &Request) -> bool {
    matches!(
        request,
        Request::List { .. }
            | Request::Get { .. }
            | Request::Resolve { .. }
            | Request::FindByRunId { .. }
            | Request::Stats { .. }
            | Request::StandingWall { .. }
    )
}

fn handle_request(
    model: &mut CenterModel,
    paths: &CenterPaths,
    request: Request,
) -> crate::Result<ResponseResult> {
    match request {
        Request::Register { registration, .. } => {
            let ledger = registration.ledger_path;
            model.refresh_ledger(paths, Utf8Path::new(&ledger))?;
            if let Some(row) = model.rows.get_mut(Utf8Path::new(&ledger)) {
                row.live = true;
                row.live_holder = Some(registration.holder);
            }
            Ok(ResponseResult::Ok)
        }
        Request::FrameDone { ledger_path, .. } => {
            model.refresh_ledger(paths, Utf8Path::new(&ledger_path))?;
            record_frame_notification();
            Ok(ResponseResult::Ok)
        }
        Request::ActivityLine {
            ledger_path,
            activity,
            ..
        } => {
            // Activity is already durable in its sidecar. It only needs
            // fan-out against a registration-established row; reopening and
            // persisting the ledger for every activity line would turn stream
            // output into a reconciliation scan.
            model.broadcast_activity(&ledger_path, activity);
            Ok(ResponseResult::Ok)
        }
        Request::Ended { ledger_path, .. } => {
            model.refresh_ledger_ended(paths, Utf8Path::new(&ledger_path))?;
            Ok(ResponseResult::Ok)
        }
        Request::List { repo_key, .. } => Ok(ResponseResult::List(public_rows(
            model,
            repo_key.as_deref(),
        ))),
        Request::Get {
            session_id: wanted,
            repo_key,
            ..
        } => match select_row(model, &wanted, repo_key.as_deref()) {
            RowSelection::One(row) => Ok(ResponseResult::Get(match &row.session {
                Some(session) => GetWireResult::Session(Box::new(session.clone())),
                None => GetWireResult::Missing,
            })),
            RowSelection::Missing => Ok(ResponseResult::Get(GetWireResult::Missing)),
            RowSelection::Ambiguous(rows) => Ok(ResponseResult::Get(GetWireResult::Ambiguous(
                rows.into_iter()
                    .map(|row| row.summary.session_id.clone())
                    .collect(),
            ))),
        },
        Request::Resolve {
            session_id: wanted,
            repo_key,
            ..
        } => {
            let result = match select_row(model, &wanted, repo_key.as_deref()) {
                RowSelection::One(row) => ResolveWireResult::Row(Box::new(public_row(row))),
                RowSelection::Missing => ResolveWireResult::Missing,
                RowSelection::Ambiguous(rows) => ResolveWireResult::Ambiguous(
                    rows.into_iter()
                        .map(|row| row.summary.session_id.clone())
                        .collect(),
                ),
            };
            Ok(ResponseResult::Resolve(result))
        }
        Request::FindByRunId {
            run_id, repo_key, ..
        } => {
            let mut rows: Vec<_> = model
                .rows
                .values()
                .filter(|row| {
                    row.summary.run_id == run_id
                        && repo_key.as_deref().is_none_or(|repo| row.repo_key == repo)
                })
                .map(public_row)
                .collect();
            rows.sort_by(public_row_order);
            Ok(ResponseResult::FindByRunId(rows))
        }
        Request::Stats {
            since_epoch,
            trait_id,
            repo_key,
            ..
        } => {
            let rows: Vec<_> = model
                .rows
                .values()
                .filter(|row| repo_key.as_deref().is_none_or(|repo| row.repo_key == repo))
                .collect();
            let records: Vec<_> = rows
                .iter()
                .filter_map(|row| row.session.as_ref())
                .map(ctx_traits_core::procedure::stats::RunRecord::from_session)
                .collect();
            let report = ctx_traits_core::procedure::stats::aggregate(
                &records,
                rows.len() as u64,
                (rows.len() - records.len()) as u64,
                since_epoch,
                trait_id.as_deref(),
            );
            Ok(ResponseResult::Stats(Box::new(report)))
        }
        Request::StandingWall {
            wall_id,
            dispatched_task,
            repo_key,
            ..
        } => {
            let mut rows: Vec<_> = model
                .rows
                .values()
                .filter(|row| repo_key.as_deref().is_none_or(|repo| row.repo_key == repo))
                .collect();
            // Store scans were lexical by ledger name; retain that selection rule
            // rather than exposing HashMap insertion order to dispatch refusal.
            rows.sort_by(|left, right| left.ledger_path.cmp(&right.ledger_path));
            let sessions: Vec<_> = rows
                .into_iter()
                .filter_map(|row| row.session.clone())
                .collect();
            Ok(ResponseResult::StandingWall(
                crate::dispatch_preflight::standing_wall_in_sessions(
                    &sessions,
                    &wall_id,
                    &dispatched_task,
                ),
            ))
        }
        Request::Subscribe { .. } => Err(protocol_error("subscribe is handled by the model owner")),
    }
}

/// Test-only process instrumentation. A marker write is deliberately
/// best-effort so it cannot make an accepted driver notification fail.
fn record_frame_notification() {
    let Some(path) = std::env::var_os("CTX_CENTER_FRAME_MARKER") else {
        return;
    };
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(b"frame\n"));
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CenterPublicRow {
    pub summary: crate::run_summary::RunSummary,
    pub repo_key: String,
    pub repo_path: String,
    pub ledger_path: String,
    pub live: bool,
    /// The ledger mtime is part of the center-owned fingerprint and is exposed
    /// for recency projections without reopening the ledger client-side.
    pub modified_epoch_secs: u64,
}

fn public_row(row: &CenterRow) -> CenterPublicRow {
    CenterPublicRow {
        summary: row.summary.clone(),
        repo_key: row.repo_key.clone(),
        repo_path: row.repo_path.clone(),
        ledger_path: row.ledger_path.to_string(),
        live: row.live,
        modified_epoch_secs: row
            .modified
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    }
}

fn repo_paths(repos: Vec<crate::state::RepoIndexEntry>) -> HashMap<String, String> {
    repos
        .into_iter()
        .map(|repo| (repo.key, repo.path))
        .collect()
}

fn public_rows(model: &CenterModel, repo_key: Option<&str>) -> Vec<CenterPublicRow> {
    let mut rows: Vec<_> = model
        .rows
        .values()
        .filter(|row| repo_key.is_none_or(|repo| row.repo_key == repo))
        .map(public_row)
        .collect();
    rows.sort_by(public_row_order);
    rows
}

fn matching_rows<'a>(
    model: &'a CenterModel,
    session_id: &str,
    repo_key: Option<&str>,
) -> Vec<&'a CenterRow> {
    let mut rows: Vec<_> = model
        .rows
        .values()
        .filter(|row| {
            row.summary.session_id.starts_with(session_id)
                && repo_key.is_none_or(|repo| row.repo_key == repo)
        })
        .collect();
    rows.sort_by(center_row_order);
    rows
}

fn public_row_order(left: &CenterPublicRow, right: &CenterPublicRow) -> std::cmp::Ordering {
    left.summary
        .session_id
        .cmp(&right.summary.session_id)
        .then_with(|| left.repo_key.cmp(&right.repo_key))
        .then_with(|| left.ledger_path.cmp(&right.ledger_path))
}

fn center_row_order(left: &&CenterRow, right: &&CenterRow) -> std::cmp::Ordering {
    left.summary
        .session_id
        .cmp(&right.summary.session_id)
        .then_with(|| left.repo_key.cmp(&right.repo_key))
        .then_with(|| left.ledger_path.cmp(&right.ledger_path))
}

enum RowSelection<'a> {
    Missing,
    One(&'a CenterRow),
    Ambiguous(Vec<&'a CenterRow>),
}

/// Exact IDs take precedence over prefix matches for both public resolution
/// and cached-session lookup. Keeping this classification in one place avoids
/// query surfaces drifting as their response shapes evolve.
fn select_row<'a>(
    model: &'a CenterModel,
    session_id: &str,
    repo_key: Option<&str>,
) -> RowSelection<'a> {
    let matches = matching_rows(model, session_id, repo_key);
    let exact: Vec<_> = matches
        .iter()
        .copied()
        .filter(|row| row.summary.session_id == session_id)
        .collect();
    let candidates = if exact.is_empty() { matches } else { exact };
    match candidates.len() {
        0 => RowSelection::Missing,
        1 => RowSelection::One(candidates[0]),
        _ => RowSelection::Ambiguous(candidates),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeSpawnChild {
        kill_succeeds: bool,
        exited: std::io::Result<bool>,
    }

    impl SpawnControl for FakeSpawnChild {
        fn kill(&mut self) -> std::io::Result<()> {
            if self.kill_succeeds {
                Ok(())
            } else {
                Err(std::io::Error::other("simulated kill failure"))
            }
        }

        fn is_exited(&mut self) -> std::io::Result<bool> {
            match &self.exited {
                Ok(exited) => Ok(*exited),
                Err(_) => Err(std::io::Error::other("simulated wait failure")),
            }
        }
    }

    fn scratch(name: &str) -> Utf8PathBuf {
        let path = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("UTF-8 temp directory")
            .join(format!("ctx-center-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(path.as_std_path());
        std::fs::create_dir_all(path.as_std_path()).expect("create scratch directory");
        path
    }

    fn paths(root: Utf8PathBuf) -> CenterPaths {
        CenterPaths {
            socket: root.join("center.sock"),
            spawn_lock: root.join("center.lock"),
            runs_root: root.clone(),
            index: root.join("index.sqlite3"),
        }
    }

    #[test]
    fn successful_kill_releases_spawn_lock_despite_reap_failure() {
        let root = scratch("abort-killed");
        let lock_path = root.join("center.lock");
        let lock = crate::file_lock::open_lock_file_no_follow(&lock_path).expect("open lock");
        assert!(crate::file_lock::try_lock_exclusive(&lock).expect("acquire lock"));

        assert!(
            abort_spawn(
                FakeSpawnChild {
                    kill_succeeds: true,
                    exited: Err(std::io::Error::other("simulated wait failure")),
                },
                lock,
                protocol_error("expected abort"),
            )
            .is_err()
        );

        let contender =
            crate::file_lock::open_lock_file_no_follow(&lock_path).expect("open contender lock");
        assert!(
            crate::file_lock::try_lock_exclusive(&contender).expect("check released lock"),
            "a successfully signaled child cannot retain spawn arbitration"
        );
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn unconfirmed_child_retains_spawn_lock_after_kill_and_wait_failures() {
        let root = scratch("abort-unconfirmed");
        let lock_path = root.join("center.lock");
        let lock = crate::file_lock::open_lock_file_no_follow(&lock_path).expect("open lock");
        assert!(crate::file_lock::try_lock_exclusive(&lock).expect("acquire lock"));

        assert!(
            abort_spawn(
                FakeSpawnChild {
                    kill_succeeds: false,
                    exited: Err(std::io::Error::other("simulated wait failure")),
                },
                lock,
                protocol_error("expected abort"),
            )
            .is_err()
        );

        // The fail-closed lease is intentionally process-lifetime scoped when
        // neither signal delivery nor process exit can be confirmed.
        let contender =
            crate::file_lock::open_lock_file_no_follow(&lock_path).expect("open contender lock");
        assert!(
            !crate::file_lock::try_lock_exclusive(&contender).expect("check retained lock"),
            "an unconfirmed child could still publish the listener"
        );
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    fn cached_summary() -> String {
        r#"{"session_id":"session","run_id":"run","trait_id":"trait","status":"awaiting-input","has_merge_frames":false}"#.to_string()
    }

    fn fixture_session(status: &str) -> ctx_traits_core::procedure::session::Session {
        fixture_session_with_state_source(status, "test")
    }

    fn fixture_session_with_state_source(
        status: &str,
        state_source: &str,
    ) -> ctx_traits_core::procedure::session::Session {
        serde_json::from_value(serde_json::json!({
            "schema-version": "0.1.0",
            "session-id": "session-fixture",
            "run-id": "run-fixture",
            "trait-id": "fixture-trait",
            "current-run-index": 0,
            "status": status,
            "provenance": {
                "started-by": {"surface": "test", "caller": "center-fixture"},
                "state-source": state_source,
                "started-at-epoch": 1000,
            },
            "ledger": {
                "run-id": "run-fixture",
                "trait-id": "fixture-trait",
                "current-run-index": 0,
                "final-state": "running",
            },
            "state-digest": "sha256:fixture",
        }))
        .expect("fixture session deserializes")
    }

    fn write_fixture_ledger(root: &Utf8Path, repo: &str, status: &str) -> Utf8PathBuf {
        let store = root.join(repo);
        std::fs::create_dir_all(store.as_std_path()).expect("create repository store");
        let ledger = store.join("session-fixture.json");
        crate::run_session::write_run_session(&ledger, &fixture_session(status))
            .expect("write fixture ledger");
        ledger
    }

    fn standing_wall_session(
        session_id: &str,
        run_id: &str,
    ) -> ctx_traits_core::procedure::session::Session {
        let mut value =
            serde_json::to_value(fixture_session("blocked")).expect("serialize fixture");
        value["session-id"] = serde_json::json!(session_id);
        value["run-id"] = serde_json::json!(run_id);
        value["ledger"]["run-id"] = serde_json::json!(run_id);
        value["ledger"]["final-state"] = serde_json::json!("blocked");
        value["accepted-port-values"] = serde_json::json!([
            {"ref-text": "port:task", "value": "other-task", "value-digest": "task-digest", "source": "ledger", "acceptance": "accepted"}
        ]);
        value["accepted-slot-values"] = serde_json::json!([
            {"ref-text": "slot:park-report", "value": [{"wall-id": "wall"}], "value-digest": "park-digest", "source": "ledger", "acceptance": "accepted"}
        ]);
        serde_json::from_value(value).expect("standing-wall fixture deserializes")
    }

    fn acknowledge_notification(stream: &mut UnixStream) -> Request {
        let request: Request =
            serde_json::from_slice(&read_line(stream).expect("read notification"))
                .expect("decode notification");
        write_line(
            stream,
            &WireMessage::Response {
                id: request.id().to_owned(),
                result: ResponseResult::Ok,
            },
        )
        .expect("acknowledge notification");
        request
    }

    fn test_activity() -> crate::activity_sidecar::ActivityRecord {
        crate::activity_sidecar::ActivityRecord::SessionTitle {
            at_epoch_ms: 1,
            title: "activity".to_string(),
        }
    }

    fn test_registration() -> DriverRegistration {
        DriverRegistration {
            ledger_path: "/ledger".to_string(),
            holder: crate::run_control::DriverHolder {
                pid: 1,
                session_id: "session".to_string(),
                run_id: "run".to_string(),
                started_at_epoch_secs: 1,
                control_token: "token".to_string(),
            },
        }
    }

    #[test]
    fn notifier_retries_the_failed_event_after_reregistering() {
        let (first_client, mut first_server) = UnixStream::pair().expect("first pair");
        let (second_client, mut second_server) = UnixStream::pair().expect("second pair");
        let server = std::thread::spawn(move || {
            let first_register = acknowledge_notification(&mut first_server);
            assert!(matches!(first_register, Request::Register { .. }));
            let dropped: Request =
                serde_json::from_slice(&read_line(&mut first_server).expect("read failed frame"))
                    .expect("decode failed frame");
            assert!(matches!(dropped, Request::FrameDone { .. }));
            drop(first_server);

            let second_register = acknowledge_notification(&mut second_server);
            assert!(matches!(second_register, Request::Register { .. }));
            let retried = acknowledge_notification(&mut second_server);
            assert!(matches!(retried, Request::FrameDone { .. }));
        });
        let (sender, receiver) = mpsc::sync_channel(2);
        sender.send(DriverEvent::FrameDone).expect("queue frame");
        drop(sender);
        let worker = std::thread::spawn(move || {
            let mut streams = std::collections::VecDeque::from([first_client, second_client]);
            notifier_worker_with(test_registration(), receiver, move || {
                Ok(streams.pop_front().expect("next connection"))
            });
        });
        server.join().expect("server completes");
        worker.join().expect("worker completes");
    }

    #[test]
    fn notifier_retries_after_a_malformed_acknowledgement() {
        let (first_client, mut first_server) = UnixStream::pair().expect("first pair");
        let (second_client, mut second_server) = UnixStream::pair().expect("second pair");
        let server = std::thread::spawn(move || {
            let _: Request = serde_json::from_slice(
                &read_line(&mut first_server).expect("read malformed-ack register"),
            )
            .expect("decode malformed-ack register");
            write_line(
                &mut first_server,
                &WireMessage::Response {
                    id: "wrong-id".to_string(),
                    result: ResponseResult::Ok,
                },
            )
            .expect("write malformed acknowledgement");

            let second_register = acknowledge_notification(&mut second_server);
            assert!(matches!(second_register, Request::Register { .. }));
            let retried = acknowledge_notification(&mut second_server);
            assert!(matches!(retried, Request::FrameDone { .. }));
        });
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(DriverEvent::FrameDone).expect("queue frame");
        drop(sender);
        let worker = std::thread::spawn(move || {
            let mut streams = std::collections::VecDeque::from([first_client, second_client]);
            notifier_worker_with(test_registration(), receiver, move || {
                Ok(streams.pop_front().expect("next connection"))
            });
        });
        server.join().expect("server completes");
        worker.join().expect("worker completes");
    }

    #[test]
    fn notifier_backoff_is_capped() {
        let mut backoff = NOTIFIER_BACKOFF_MIN;
        for _ in 0..32 {
            backoff = next_notifier_backoff(backoff);
        }
        assert_eq!(backoff, NOTIFIER_BACKOFF_MAX);
        assert_eq!(
            next_notifier_backoff(NOTIFIER_BACKOFF_MAX),
            NOTIFIER_BACKOFF_MAX,
            "a permanently absent center cannot create an unbounded retry delay"
        );
    }

    #[test]
    fn notifier_applies_backoff_between_refused_connections() {
        let (client, mut server) = UnixStream::pair().expect("eventual pair");
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(DriverEvent::FrameDone).expect("queue frame");
        drop(sender);
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_attempts = attempts.clone();
        let started = Instant::now();
        let worker = std::thread::spawn(move || {
            let mut client = Some(client);
            notifier_worker_with(test_registration(), receiver, move || {
                let attempt = observed_attempts.fetch_add(1, Ordering::SeqCst);
                if attempt < 2 {
                    Err(protocol_error("refused test connection"))
                } else {
                    Ok(client.take().expect("one eventual connection"))
                }
            });
        });
        let register = acknowledge_notification(&mut server);
        assert!(matches!(register, Request::Register { .. }));
        let frame = acknowledge_notification(&mut server);
        assert!(matches!(frame, Request::FrameDone { .. }));
        worker.join().expect("worker completes");
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        assert!(
            started.elapsed() >= NOTIFIER_BACKOFF_MIN + NOTIFIER_BACKOFF_MIN * 2,
            "refused endpoints must not spin through reconnect attempts"
        );
    }

    #[test]
    fn notifier_uses_capped_intervals_while_the_center_stays_absent() {
        let (client, mut server) = UnixStream::pair().expect("eventual pair");
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(DriverEvent::FrameDone).expect("queue frame");
        drop(sender);
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_attempts = attempts.clone();
        let delays = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed_delays = delays.clone();
        let worker = std::thread::spawn(move || {
            let mut client = Some(client);
            notifier_worker_with_sleep(
                test_registration(),
                receiver,
                move || {
                    let attempt = observed_attempts.fetch_add(1, Ordering::SeqCst);
                    if attempt < 9 {
                        Err(protocol_error("permanently absent test connection"))
                    } else {
                        Ok(client.take().expect("eventual connection"))
                    }
                },
                move |delay| observed_delays.lock().expect("record delay").push(delay),
            );
        });
        assert!(matches!(
            acknowledge_notification(&mut server),
            Request::Register { .. }
        ));
        assert!(matches!(
            acknowledge_notification(&mut server),
            Request::FrameDone { .. }
        ));
        worker.join().expect("worker completes");
        assert_eq!(attempts.load(Ordering::SeqCst), 10);
        assert_eq!(
            *delays.lock().expect("read delays"),
            vec![
                NOTIFIER_BACKOFF_MIN,
                NOTIFIER_BACKOFF_MIN * 2,
                NOTIFIER_BACKOFF_MIN * 4,
                NOTIFIER_BACKOFF_MIN * 8,
                NOTIFIER_BACKOFF_MIN * 16,
                NOTIFIER_BACKOFF_MIN * 32,
                NOTIFIER_BACKOFF_MIN * 64,
                NOTIFIER_BACKOFF_MAX,
                NOTIFIER_BACKOFF_MAX,
            ],
            "a missing center reaches the cap and then retries at the capped rate"
        );
    }

    #[test]
    fn notifier_producers_drop_saturated_notifications_without_waiting() {
        let (sender, receiver) = mpsc::sync_channel(NOTIFIER_QUEUE);
        let notifier = DriverNotifier { sender };
        for _ in 0..NOTIFIER_QUEUE {
            notifier.frame_done();
        }
        let started = Instant::now();
        notifier.frame_done();
        notifier.activity_line(test_activity());
        notifier.ended();
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "a saturated notification queue must not delay the driver"
        );
        assert_eq!(
            (0..NOTIFIER_QUEUE)
                .filter_map(|_| receiver.try_recv().ok())
                .count(),
            NOTIFIER_QUEUE,
            "the bounded queue retains only its fixed capacity"
        );
    }

    #[test]
    fn activity_notification_fans_out_without_a_ledger_refresh() {
        let root = scratch("activity-only");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("construct model");
        let (sender, receiver) = mpsc::sync_channel(4);
        model
            .subscribe(1, "activity-subscription".to_string(), None, sender)
            .expect("subscribe");
        let _ = receiver.recv().expect("snapshot start");
        model.advance_snapshot(1);
        let _ = receiver.recv().expect("snapshot row");
        model.advance_snapshot(1);
        let _ = receiver.recv().expect("snapshot end");

        LEDGER_READS.with(|reads| reads.set(0));
        handle_request(
            &mut model,
            &paths,
            Request::ActivityLine {
                id: next_id("test"),
                ledger_path: ledger.to_string(),
                activity: test_activity(),
            },
        )
        .expect("fan out durable activity");
        LEDGER_READS.with(|reads| assert_eq!(reads.get(), 0));
        assert!(matches!(
            receiver.recv(),
            Ok(Outbound::Delta(CenterDelta::ActivityLine { .. }))
        ));
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn notifications_require_registration_on_each_connection() {
        let (mut client, server) = UnixStream::pair().expect("stream pair");
        let (jobs, receiver) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || serve_connection_worker(server, jobs));
        write_line(
            &mut client,
            &WireMessage::Hello {
                id: "hello".to_string(),
            },
        )
        .expect("send hello");
        let ready: WireMessage =
            serde_json::from_slice(&read_line(&mut client).expect("read ready"))
                .expect("decode ready");
        assert!(matches!(ready, WireMessage::Ready { id } if id == "hello"));
        write_line(
            &mut client,
            &Request::FrameDone {
                id: "frame".to_string(),
                ledger_path: "/ledger".to_string(),
            },
        )
        .expect("send unregistered frame");
        let reply: WireMessage =
            serde_json::from_slice(&read_line(&mut client).expect("read rejection"))
                .expect("decode rejection");
        assert!(
            matches!(reply, WireMessage::Response { id, result: ResponseResult::Error { .. } } if id == "frame")
        );
        assert!(
            receiver.try_recv().is_err(),
            "unregistered frame reached model owner"
        );
        worker.join().expect("worker exits");
    }

    #[test]
    fn registration_requires_holder_and_binds_events_to_its_ledger() {
        let (mut client, server) = UnixStream::pair().expect("stream pair");
        let (jobs, receiver) = mpsc::sync_channel(2);
        let worker = std::thread::spawn(move || serve_connection_worker(server, jobs));
        write_line(
            &mut client,
            &WireMessage::Hello {
                id: "hello".to_string(),
            },
        )
        .expect("send hello");
        let _: WireMessage = serde_json::from_slice(&read_line(&mut client).expect("read ready"))
            .expect("decode ready");

        // Holder metadata comes from DriverLockGuard, not from a caller-made
        // ledger description. A malformed register closes this connection.
        client
            .write_all(b"{\"kind\":\"register\",\"id\":\"bad\",\"ledger_path\":\"/ledger\"}\n")
            .expect("send malformed registration");
        assert!(receiver.try_recv().is_err());
        worker.join().expect("worker exits");

        let (mut client, server) = UnixStream::pair().expect("stream pair");
        let (jobs, receiver) = mpsc::sync_channel(2);
        let worker = std::thread::spawn(move || serve_connection_worker(server, jobs));
        write_line(
            &mut client,
            &WireMessage::Hello {
                id: "hello".to_string(),
            },
        )
        .expect("send hello");
        let _: WireMessage = serde_json::from_slice(&read_line(&mut client).expect("read ready"))
            .expect("decode ready");
        write_line(
            &mut client,
            &Request::Register {
                id: "register".to_string(),
                registration: DriverRegistration {
                    ledger_path: "/ledger".to_string(),
                    holder: crate::run_control::DriverHolder {
                        pid: 1,
                        session_id: "session".to_string(),
                        run_id: "run".to_string(),
                        started_at_epoch_secs: 1,
                        control_token: "token".to_string(),
                    },
                },
            },
        )
        .expect("send registration");
        let ModelCommand::Request { reply, .. } = receiver.recv().expect("registration job") else {
            panic!("expected registration request");
        };
        reply
            .send(Ok(ResponseResult::Ok))
            .expect("ack registration");
        let _: WireMessage =
            serde_json::from_slice(&read_line(&mut client).expect("read registration ack"))
                .expect("decode registration ack");
        write_line(
            &mut client,
            &Request::FrameDone {
                id: "wrong".to_string(),
                ledger_path: "/other".to_string(),
            },
        )
        .expect("send mismatched frame");
        let rejected: WireMessage =
            serde_json::from_slice(&read_line(&mut client).expect("read rejection"))
                .expect("decode rejection");
        assert!(matches!(
            rejected,
            WireMessage::Response {
                result: ResponseResult::Error { .. },
                ..
            }
        ));
        assert!(
            receiver.try_recv().is_err(),
            "mismatched event reached model owner"
        );
        worker.join().expect("worker exits");
    }

    #[test]
    fn stalled_peer_does_not_delay_another_typed_request() {
        let (stalled_client, stalled_server) = UnixStream::pair().expect("stalled pair");
        let (jobs, receiver) = mpsc::sync_channel(2);
        let stalled_worker = std::thread::spawn({
            let jobs = jobs.clone();
            move || serve_connection_worker(stalled_server, jobs)
        });

        let (mut client, server) = UnixStream::pair().expect("request pair");
        let request_worker = std::thread::spawn(move || serve_connection_worker(server, jobs));
        write_line(
            &mut client,
            &WireMessage::Hello {
                id: "hello".to_string(),
            },
        )
        .expect("send hello");
        let _: WireMessage = serde_json::from_slice(&read_line(&mut client).expect("read ready"))
            .expect("decode ready");
        write_line(
            &mut client,
            &Request::List {
                id: "list".to_string(),
                repo_key: None,
            },
        )
        .expect("send list");
        let ModelCommand::Request { reply, .. } = receiver
            .recv_timeout(Duration::from_millis(250))
            .expect("stalled peer must not block owner work")
        else {
            panic!("expected typed request");
        };
        reply
            .send(Ok(ResponseResult::List(Vec::new())))
            .expect("reply to worker");
        assert!(matches!(
            serde_json::from_slice::<WireMessage>(&read_line(&mut client).expect("read list")),
            Ok(WireMessage::Response { id, result: ResponseResult::List(rows) }) if id == "list" && rows.is_empty()
        ));
        drop(client);
        drop(stalled_client);
        request_worker.join().expect("request worker exits");
        stalled_worker.join().expect("stalled worker exits");
    }

    fn handshaken_worker(
        jobs: mpsc::SyncSender<ModelCommand>,
    ) -> (UnixStream, std::thread::JoinHandle<()>) {
        let (mut client, server) = UnixStream::pair().expect("socket pair");
        let worker = std::thread::spawn(move || serve_connection_worker(server, jobs));
        write_line(
            &mut client,
            &WireMessage::Hello {
                id: "hello".to_string(),
            },
        )
        .expect("send hello");
        assert!(matches!(
            serde_json::from_slice::<WireMessage>(&read_line(&mut client).expect("read ready")),
            Ok(WireMessage::Ready { id }) if id == "hello"
        ));
        (client, worker)
    }

    fn await_server(paths: &CenterPaths) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match UnixStream::connect(paths.socket.as_std_path()) {
                Ok(mut stream) => {
                    handshake(&mut stream).expect("handshake with test server");
                    return;
                }
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("test server did not bind: {error}"),
            }
        }
    }

    fn registered_notifier_stream(paths: &CenterPaths, ledger: &Utf8Path) -> UnixStream {
        let mut stream = UnixStream::connect(paths.socket.as_std_path()).expect("connect notifier");
        handshake(&mut stream).expect("notifier handshake");
        let request = Request::Register {
            id: "register".to_string(),
            registration: DriverRegistration {
                ledger_path: ledger.to_string(),
                holder: crate::run_control::DriverHolder {
                    pid: std::process::id(),
                    session_id: "session-fixture".to_string(),
                    run_id: "run-fixture".to_string(),
                    started_at_epoch_secs: 1,
                    control_token: "test".to_string(),
                },
            },
        };
        write_line(&mut stream, &request).expect("register notifier");
        assert!(matches!(
            serde_json::from_slice::<WireMessage>(
                &read_line(&mut stream).expect("register acknowledgement")
            ),
            Ok(WireMessage::Response {
                result: ResponseResult::Ok,
                ..
            })
        ));
        stream
    }

    #[test]
    fn accepted_hostile_peers_do_not_delay_a_second_request() {
        let (jobs, receiver) = mpsc::sync_channel(4);

        // This peer has completed the handshake then trickles a malformed
        // request. Its worker owns the deadline, never the model owner.
        let (mut trickler, trickler_worker) = handshaken_worker(jobs.clone());
        let trickle = std::thread::spawn(move || {
            for byte in b"{\"kind\":\"list\"" {
                if trickler.write_all(&[*byte]).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });

        // A malformed peer must similarly exit locally instead of injecting an
        // untyped command into the owner queue.
        let (mut malformed, malformed_worker) = handshaken_worker(jobs.clone());
        malformed
            .write_all(b"not-json\n")
            .expect("send malformed request");

        let (mut client, client_worker) = handshaken_worker(jobs);
        write_line(
            &mut client,
            &Request::List {
                id: "list".to_string(),
                repo_key: None,
            },
        )
        .expect("send second request");
        let ModelCommand::Request { reply, .. } = receiver
            .recv_timeout(Duration::from_millis(250))
            .expect("hostile peers must not delay owner work")
        else {
            panic!("expected typed request");
        };
        reply
            .send(Ok(ResponseResult::List(Vec::new())))
            .expect("reply to second request");
        assert!(matches!(
            serde_json::from_slice::<WireMessage>(&read_line(&mut client).expect("read list")),
            Ok(WireMessage::Response { id, result: ResponseResult::List(rows) }) if id == "list" && rows.is_empty()
        ));
        drop(client);
        drop(malformed);
        trickle.join().expect("trickler exits");
        client_worker.join().expect("client worker exits");
        malformed_worker.join().expect("malformed worker exits");
        trickler_worker.join().expect("trickler worker exits");
    }

    #[test]
    fn subscriber_backpressure_closes_client_stream() {
        let root = scratch("nonreading-subscriber");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let server_paths = paths.clone();
        let server = std::thread::spawn(move || {
            run_server_at(
                server_paths,
                Duration::from_secs(3),
                Duration::from_millis(10),
            )
        });
        await_server(&paths);

        // Leave this client subscribed but never read its snapshot or deltas.
        // Its writer may block on the peer socket, but all model work must stay
        // available through the independent notifier and query workers.
        let mut subscriber =
            UnixStream::connect(paths.socket.as_std_path()).expect("connect subscriber");
        handshake(&mut subscriber).expect("subscriber handshake");
        write_line(
            &mut subscriber,
            &Request::Subscribe {
                id: "stalled-subscription".to_string(),
                repo_key: None,
            },
        )
        .expect("subscribe without reading");

        let mut notifier = registered_notifier_stream(&paths, &ledger);
        // Once the socket writer blocks, this exceeds the bounded outbound
        // queue and causes the model to evict the subscriber. The writer's
        // timeout must then make that eviction visible as EOF to this socket.
        for sequence in 0..(SUBSCRIBER_QUEUE * 2) {
            let request = Request::ActivityLine {
                id: format!("activity-{sequence}"),
                ledger_path: ledger.to_string(),
                activity: crate::activity_sidecar::ActivityRecord::SessionTitle {
                    at_epoch_ms: sequence as u64,
                    title: "x".repeat(64 * 1024),
                },
            };
            write_line(&mut notifier, &request).expect("send durable activity notification");
            assert!(matches!(
                serde_json::from_slice::<WireMessage>(
                    &read_line(&mut notifier).expect("activity acknowledgement")
                ),
                Ok(WireMessage::Response {
                    result: ResponseResult::Ok,
                    ..
                })
            ));
        }

        crate::run_session::write_run_session(&ledger, &fixture_session("failed"))
            .expect("rewrite authoritative ledger");
        let started = Instant::now();
        let mut query = UnixStream::connect(paths.socket.as_std_path()).expect("connect query");
        handshake(&mut query).expect("query handshake");
        write_line(
            &mut query,
            &Request::List {
                id: "independent-list".to_string(),
                repo_key: None,
            },
        )
        .expect("send independent query");
        let response: WireMessage =
            serde_json::from_slice(&read_line(&mut query).expect("query response"))
                .expect("decode query response");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(matches!(
            response,
            WireMessage::Response { id, result: ResponseResult::List(rows) }
                if id == "independent-list"
                    && rows.iter().any(|row| row.summary.status == ctx_traits_core::procedure::session::Status::Failed)
        ));

        write_line(
            &mut notifier,
            &Request::Ended {
                id: "ended".to_string(),
                ledger_path: ledger.to_string(),
            },
        )
        .expect("send terminal notification");
        let _: WireMessage =
            serde_json::from_slice(&read_line(&mut notifier).expect("ended acknowledgement"))
                .expect("decode ended acknowledgement");

        std::thread::sleep(STREAM_TIMEOUT + Duration::from_millis(100));
        loop {
            match read_line(&mut subscriber) {
                Ok(_) => continue,
                Err(error)
                    if error.to_string().contains("unexpected EOF")
                        || error.to_string().contains("unterminated line") =>
                {
                    break;
                }
                Err(error) => panic!("evicted subscriber did not observe EOF: {error}"),
            }
        }

        // A disconnected subscriber does not poison future subscriptions: a
        // reconnect receives a complete new snapshot from the owner model.
        let mut replacement = UnixStream::connect(paths.socket.as_std_path())
            .expect("reconnect after subscriber eviction");
        handshake(&mut replacement).expect("replacement handshake");
        write_line(
            &mut replacement,
            &Request::Subscribe {
                id: "replacement-subscription".to_string(),
                repo_key: None,
            },
        )
        .expect("subscribe replacement");
        assert!(matches!(
            serde_json::from_slice::<WireMessage>(&read_line(&mut replacement).expect("snapshot start")),
            Ok(WireMessage::SnapshotStart { id }) if id == "replacement-subscription"
        ));
        assert!(matches!(
            serde_json::from_slice::<WireMessage>(
                &read_line(&mut replacement).expect("snapshot row")
            ),
            Ok(WireMessage::SnapshotRow { .. })
        ));
        assert!(matches!(
            serde_json::from_slice::<WireMessage>(
                &read_line(&mut replacement).expect("snapshot end")
            ),
            Ok(WireMessage::SnapshotEnd)
        ));

        drop(query);
        drop(notifier);
        drop(replacement);
        drop(subscriber);
        server.join().expect("server exits").expect("server result");
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn bounded_socket_transport_carries_all_queries_and_large_get() {
        let (jobs, receiver) = mpsc::sync_channel(8);
        let (mut client, worker) = handshaken_worker(jobs);
        let large_state_source = "x".repeat(8 * 1024);
        let large_session = fixture_session_with_state_source("completed", &large_state_source);

        let requests = [
            Request::List {
                id: "list".to_string(),
                repo_key: None,
            },
            Request::Get {
                id: "get".to_string(),
                session_id: "session".to_string(),
                repo_key: None,
            },
            Request::Resolve {
                id: "resolve".to_string(),
                session_id: "session".to_string(),
                repo_key: None,
            },
            Request::FindByRunId {
                id: "run".to_string(),
                run_id: "run".to_string(),
                repo_key: None,
            },
            Request::Stats {
                id: "stats".to_string(),
                since_epoch: None,
                trait_id: None,
                repo_key: None,
            },
        ];
        for request in requests {
            let id = request.id().to_string();
            write_line(&mut client, &request).expect("write typed query");
            let ModelCommand::Request { request, reply } =
                receiver.recv().expect("query reaches owner")
            else {
                panic!("expected query");
            };
            let result = match request {
                Request::List { .. } => ResponseResult::List(Vec::new()),
                Request::Get { .. } => {
                    ResponseResult::Get(GetWireResult::Session(Box::new(large_session.clone())))
                }
                Request::Resolve { .. } => ResponseResult::Resolve(ResolveWireResult::Missing),
                Request::FindByRunId { .. } => ResponseResult::FindByRunId(Vec::new()),
                Request::Stats { .. } => ResponseResult::Stats(Box::new(
                    ctx_traits_core::procedure::stats::aggregate(&[], 0, 0, None, None),
                )),
                _ => panic!("unexpected non-query"),
            };
            reply.send(Ok(result)).expect("owner reply");
            let line = read_line(&mut client).expect("read query response");
            if id == "get" {
                assert!(
                    line.len() > 4 * 1024,
                    "cached get crossed the old response bound"
                );
            }
            assert!(matches!(
                serde_json::from_slice::<WireMessage>(&line),
                Ok(WireMessage::Response { id: response_id, .. }) if response_id == id
            ));
        }
        drop(client);
        worker.join().expect("query worker exits");
    }

    #[test]
    fn version_scoped_paths_are_distinct() {
        let paths = production_paths().expect("derive production paths");
        let version = env!("CARGO_PKG_VERSION");
        let uid = unsafe { libc::getuid() };
        assert_eq!(paths.socket, format!("/tmp/ctx-{uid}-{version}.sock"));
        assert_eq!(
            paths.spawn_lock,
            format!("/tmp/ctx-{uid}-{version}.spawn.lock")
        );
        assert_eq!(paths.index, paths.runs_root.join("index.sqlite3"));

        let root = Utf8PathBuf::from("/tmp/ctx-center-version-test");
        let first = versioned_paths(uid, "1.2.3", root.clone());
        let second = versioned_paths(uid, "1.2.4", root);
        assert_ne!(first.socket, second.socket);
        assert_ne!(first.spawn_lock, second.spawn_lock);
        assert_eq!(first.index, second.index);
    }

    #[test]
    fn json_line_handshake_is_correlated_and_bounded() {
        let (mut client, mut server) = UnixStream::pair().expect("socket pair");
        let thread = std::thread::spawn(move || serve_handshake(&mut server));
        handshake(&mut client).expect("handshake");
        thread
            .join()
            .expect("server thread")
            .expect("server handshake");
    }

    #[test]
    fn partial_handshake_eof_gets_the_single_retry_classification() {
        let (mut client, mut server) = UnixStream::pair().expect("socket pair");
        let thread = std::thread::spawn(move || {
            server
                .write_all(b"{\"kind\":\"ready\"")
                .expect("write partial line");
        });
        let error = handshake(&mut client).expect_err("partial reply must fail");
        assert!(is_handshake_eof(&error), "{error}");
        thread.join().expect("server thread");
    }

    #[test]
    fn complete_handshake_eof_gets_the_single_retry_classification() {
        let (mut client, mut server) = UnixStream::pair().expect("socket pair");
        let thread = std::thread::spawn(move || {
            let _ = read_line(&mut server).expect("read hello");
        });
        let error = handshake(&mut client).expect_err("empty reply must fail");
        assert!(is_handshake_eof(&error), "{error}");
        thread.join().expect("server thread");
    }

    #[test]
    fn trickling_line_uses_an_absolute_read_deadline() {
        let (mut client, mut server) = UnixStream::pair().expect("socket pair");
        let thread = std::thread::spawn(move || {
            for _ in 0..100 {
                if server.write_all(b"x").is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(30));
            }
        });
        let started = Instant::now();
        let error = read_line(&mut client).expect_err("trickled line must time out");
        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < STREAM_TIMEOUT + Duration::from_secs(1));
        thread.join().expect("server thread");
    }

    #[test]
    fn malformed_ready_correlation_is_rejected() {
        let (mut client, mut server) = UnixStream::pair().expect("socket pair");
        let thread = std::thread::spawn(move || {
            let _ = read_line(&mut server).expect("read hello");
            server
                .write_all(b"{\"kind\":\"ready\",\"id\":\"other\"}\n")
                .expect("write mismatched ready");
        });
        assert!(handshake(&mut client).is_err());
        thread.join().expect("server thread");
    }

    #[test]
    fn short_handshake_deadline_does_not_inherit_the_stream_timeout() {
        let (mut client, _silent_peer) = UnixStream::pair().expect("socket pair");
        let timeout = Duration::from_millis(25);
        let started = Instant::now();
        assert!(handshake_with_timeout(&mut client, timeout).is_err());
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "short handshake waited {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn oversized_protocol_line_is_rejected() {
        let (mut client, mut server) = UnixStream::pair().expect("socket pair");
        let thread = std::thread::spawn(move || {
            server
                .write_all(&vec![b'x'; MAX_LINE_BYTES + 1])
                .expect("write oversized line");
        });
        assert!(read_line(&mut client).is_err());
        thread.join().expect("server thread");
    }

    #[test]
    fn sqlite_index_initializes_and_reopens() {
        let root = scratch("sqlite");
        let paths = paths(root.clone());
        let model = CenterModel::open(&paths).expect("open index");
        assert!(model.rows.is_empty());
        drop(model);
        let reopened = CenterModel::open(&paths).expect("reopen index");
        assert!(reopened.rows.is_empty());
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn corrupt_cached_fingerprint_is_removable_not_a_panic() {
        let root = scratch("bad-cache");
        let paths = paths(root.clone());
        let model = CenterModel::open(&paths).expect("open index");
        model
            .db
            .execute(
                "INSERT INTO center_rows VALUES ('x', 'key', '', -1, 0, 0, '{}')",
                [],
            )
            .expect("insert corrupt row");
        drop(model);
        let error = match CenterModel::open(&paths) {
            Ok(_) => panic!("negative field accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains(paths.index.as_str()));
        assert!(error.to_string().contains("remove"));
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn incompatible_schema_rebuilds_the_disposable_index() {
        let root = scratch("schema");
        let paths = paths(root.clone());
        let model = CenterModel::open(&paths).expect("open index");
        model
            .db
            .execute(
                "UPDATE center_projection_meta SET version = ?1",
                [CENTER_PROJECTION_VERSION - 1],
            )
            .expect("make index use the preceding schema version");
        drop(model);
        let model = CenterModel::open(&paths).expect("rebuild incompatible index");
        assert!(model.rows.is_empty());
        let version: i64 = model
            .db
            .query_row("SELECT version FROM center_projection_meta", [], |row| {
                row.get(0)
            })
            .expect("read rebuilt version");
        assert_eq!(version, CENTER_PROJECTION_VERSION);
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn prior_projection_marker_is_migrated_back_to_the_v1_shared_marker() {
        let root = scratch("prior-projection-marker");
        let paths = paths(root.clone());
        let model = CenterModel::open(&paths).expect("open index");
        model
            .db
            .execute(
                "UPDATE center_meta SET version = ?1",
                [CENTER_PROJECTION_VERSION],
            )
            .expect("simulate prior marker");
        drop(model);

        let model = CenterModel::open(&paths).expect("migrate prior marker");
        let version: i64 = model
            .db
            .query_row("SELECT version FROM center_meta", [], |row| row.get(0))
            .expect("read shared marker");
        assert_eq!(version, CENTER_SCHEMA_VERSION);
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn a_version_one_index_is_reprojected_by_the_current_center() {
        use std::os::unix::fs::MetadataExt;

        let root = scratch("v1-summary-reproject");
        let paths = paths(root.clone());
        let ledger = root.join("repo/session.json");
        std::fs::create_dir_all(ledger.parent().expect("ledger parent").as_std_path())
            .expect("create repository store");
        let mut session = fixture_session("completed");
        session.provenance.task_key = Some("0243.4".to_string());
        crate::run_session::write_run_session(&ledger, &session).expect("write ledger");

        let metadata = std::fs::metadata(ledger.as_std_path()).expect("stat ledger");
        let secs = metadata.mtime();
        let nanos = metadata.mtime_nsec();
        let size = i64::try_from(metadata.size()).expect("ledger size fits sqlite");
        let db = Connection::open(paths.index.as_std_path()).expect("create v1 index");
        db.execute_batch("CREATE TABLE center_meta (version INTEGER NOT NULL); CREATE TABLE center_rows (ledger TEXT PRIMARY KEY, repo_key TEXT NOT NULL, repo_path TEXT NOT NULL, mtime_secs INTEGER NOT NULL, mtime_nanos INTEGER NOT NULL, size INTEGER NOT NULL, summary TEXT NOT NULL); CREATE TABLE center_sessions (ledger TEXT PRIMARY KEY, mtime_secs INTEGER NOT NULL, mtime_nanos INTEGER NOT NULL, size INTEGER NOT NULL, session TEXT NOT NULL);")
            .expect("create v1 tables");
        db.execute(
            "INSERT INTO center_meta(version) VALUES (?1)",
            [CENTER_SCHEMA_VERSION],
        )
        .expect("write v1 marker");
        db.execute(
            "INSERT INTO center_rows(ledger, repo_key, repo_path, mtime_secs, mtime_nanos, size, summary) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![ledger.as_str(), "repo", "/repo", secs, nanos, size, cached_summary()],
        )
        .expect("write cached row");
        db.execute(
            "INSERT INTO center_sessions(ledger, mtime_secs, mtime_nanos, size, session) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ledger.as_str(), secs, nanos, size, serde_json::to_string(&session).expect("encode session")],
        )
        .expect("write cached session");
        drop(db);

        let model = CenterModel::open(&paths).expect("reopen and reproject");
        let row = model.rows.get(&ledger).expect("reprojected row");
        assert_eq!(row.summary.run_id, "run-fixture");
        assert_eq!(row.summary.task_key.as_deref(), Some("0243.4"));
        assert!(row.summary.parse_error.is_none());
        assert!(row.session.is_some());
        let schema_version: i64 = model
            .db
            .query_row("SELECT version FROM center_meta", [], |row| row.get(0))
            .expect("read shared marker");
        assert_eq!(schema_version, CENTER_SCHEMA_VERSION);
        let projection_version: i64 = model
            .db
            .query_row("SELECT version FROM center_projection_meta", [], |row| {
                row.get(0)
            })
            .expect("read projection marker");
        assert_eq!(projection_version, CENTER_PROJECTION_VERSION);
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn standing_wall_preserves_ledger_order() {
        let root = scratch("standing-wall-order");
        let paths = paths(root.clone());
        let repository = root.join("repo");
        std::fs::create_dir_all(repository.as_std_path()).expect("create repository store");
        let first = repository.join("a-session.json");
        let second = repository.join("z-session.json");
        crate::run_session::write_run_session(&first, &standing_wall_session("first", "first-run"))
            .expect("write first ledger");
        crate::run_session::write_run_session(
            &second,
            &standing_wall_session("second", "second-run"),
        )
        .expect("write second ledger");

        let mut model = CenterModel::open(&paths).expect("open center");
        // Insert reverse-lexically to prove request handling, rather than map
        // insertion order, chooses the same row as the former store scan.
        model
            .refresh_ledger_inner(&paths, &second, false, Some(&HashMap::new()))
            .expect("cache second ledger");
        model
            .refresh_ledger_inner(&paths, &first, false, Some(&HashMap::new()))
            .expect("cache first ledger");

        for _ in 0..4 {
            match handle_request(
                &mut model,
                &paths,
                Request::StandingWall {
                    id: next_id("standing-wall-order"),
                    wall_id: "wall".to_string(),
                    dispatched_task: "new-task".to_string(),
                    repo_key: None,
                },
            )
            .expect("standing wall request")
            {
                ResponseResult::StandingWall(Some(wall)) => {
                    assert_eq!(wall.origin_run_id, "first-run");
                }
                other => panic!("expected standing wall, got {other:?}"),
            }
        }
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn corrupt_ledger_remains_a_visible_unreadable_row_without_blocking_idle() {
        let root = scratch("unreadable-row");
        let paths = paths(root.clone());
        let ledger = root.join("repo/session-corrupt.json");
        std::fs::create_dir_all(ledger.parent().expect("ledger parent").as_std_path())
            .expect("create repository store");
        std::fs::write(ledger.as_std_path(), "not json").expect("write corrupt ledger");

        let mut model = CenterModel::open(&paths).expect("open center");
        model.discover(&paths).expect("discover corrupt ledger");
        let row = model.rows.get(&ledger).expect("retain unreadable row");
        assert!(row.session.is_none());
        assert!(row.summary.parse_error.is_some());
        assert!(!model.has_live());
        drop(model);
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn unchanged_unreadable_fingerprint_does_not_parse() {
        let root = scratch("unchanged-unreadable");
        let paths = paths(root.clone());
        let ledger = root.join("repo/session-corrupt.json");
        std::fs::create_dir_all(ledger.parent().expect("ledger parent").as_std_path())
            .expect("create repository store");
        std::fs::write(ledger.as_std_path(), "not json").expect("write corrupt ledger");

        let mut model = CenterModel::open(&paths).expect("open center");
        model.discover(&paths).expect("initial discovery");
        assert!(
            model
                .rows
                .get(&ledger)
                .is_some_and(|row| row.session.is_none() && row.summary.parse_error.is_some())
        );

        LEDGER_READS.with(|reads| reads.set(0));
        model.discover(&paths).expect("unchanged discovery");
        for request in [
            Request::List {
                id: next_id("test"),
                repo_key: Some("repo".to_string()),
            },
            Request::Stats {
                id: next_id("test"),
                since_epoch: None,
                trait_id: None,
                repo_key: Some("repo".to_string()),
            },
        ] {
            handle_request(&mut model, &paths, request).expect("cached unreadable query");
        }
        LEDGER_READS.with(|reads| assert_eq!(reads.get(), 0));
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn corrupt_only_store_still_idle_exits() {
        let root = scratch("unreadable-idle-exit");
        let paths = paths(root.clone());
        let ledger = root.join("repo/session-corrupt.json");
        std::fs::create_dir_all(ledger.parent().expect("ledger parent").as_std_path())
            .expect("create repository store");
        std::fs::write(ledger.as_std_path(), "not json").expect("write corrupt ledger");

        let server_paths = paths.clone();
        let (completed, result) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = completed.send(run_server_at(
                server_paths,
                Duration::from_millis(40),
                Duration::from_secs(1),
            ));
        });
        assert!(matches!(
            result.recv_timeout(Duration::from_secs(1)),
            Ok(Ok(()))
        ));
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn corrupt_first_discovery_publishes_an_appeared_row() {
        let root = scratch("unreadable-appeared");
        let paths = paths(root.clone());
        let ledger = root.join("repo/session-corrupt.json");
        std::fs::create_dir_all(ledger.parent().expect("ledger parent").as_std_path())
            .expect("create repository store");
        std::fs::write(ledger.as_std_path(), "not json").expect("write corrupt ledger");

        let mut model = CenterModel::open(&paths).expect("open center");
        let (outbound, receiver) = mpsc::sync_channel(4);
        model.subscribers.insert(
            1,
            Subscriber {
                repo_key: None,
                outbound,
                snapshot: None,
                pending_deltas: VecDeque::new(),
            },
        );
        model.discover(&paths).expect("discover corrupt ledger");
        match receiver.recv().expect("unreadable delta") {
            Outbound::Delta(CenterDelta::Appeared { row }) => {
                assert_eq!(row.ledger_path, ledger);
                assert!(row.summary.parse_error.is_some());
            }
            _ => panic!("expected appeared delta"),
        }
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn readable_to_corrupt_refresh_emits_row_changed() {
        let root = scratch("readable-to-corrupt");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repo", "completed");
        let mut model = CenterModel::open(&paths).expect("open center");
        model
            .refresh_ledger_inner(&paths, &ledger, false, Some(&HashMap::new()))
            .expect("cache readable ledger");
        let (outbound, receiver) = mpsc::sync_channel(4);
        model.subscribers.insert(
            1,
            Subscriber {
                repo_key: None,
                outbound,
                snapshot: None,
                pending_deltas: VecDeque::new(),
            },
        );
        std::thread::sleep(Duration::from_millis(2));
        std::fs::write(ledger.as_std_path(), "not json").expect("corrupt ledger");
        model
            .refresh_ledger_inner(&paths, &ledger, true, Some(&HashMap::new()))
            .expect("publish unreadable candidate");
        match receiver.recv().expect("row delta") {
            Outbound::Delta(CenterDelta::RowChanged { row }) => {
                assert!(row.summary.parse_error.is_some());
            }
            _ => panic!("expected row-changed delta"),
        }
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn persistence_failure_restores_the_previous_verified_row() {
        let root = scratch("unreadable-persist-rollback");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repo", "completed");
        let mut model = CenterModel::open(&paths).expect("open center");
        model
            .refresh_ledger_inner(&paths, &ledger, false, Some(&HashMap::new()))
            .expect("cache readable ledger");
        let previous = model.rows.get(&ledger).expect("verified row").clone();

        // Force SQLite to reject the candidate after parsing and liveness have
        // succeeded. The refresh must retain both its in-memory and durable
        // verified predecessor rather than publishing a half-persisted row.
        model
            .db
            .execute_batch(
                "CREATE TRIGGER reject_center_row BEFORE INSERT ON center_rows \
                 BEGIN SELECT RAISE(ABORT, 'injected persist failure'); END;",
            )
            .expect("install failing trigger");
        std::thread::sleep(Duration::from_millis(2));
        std::fs::write(ledger.as_std_path(), "not json").expect("corrupt ledger");
        assert!(
            model
                .refresh_ledger_inner(&paths, &ledger, false, Some(&HashMap::new()))
                .is_err()
        );
        let retained = model.rows.get(&ledger).expect("previous row remains");
        assert_eq!(retained.summary, previous.summary);
        assert_eq!(retained.modified, previous.modified);
        assert_eq!(retained.size, previous.size);

        model
            .db
            .execute_batch("DROP TRIGGER reject_center_row")
            .expect("remove failing trigger");
        drop(model);
        let model = CenterModel::open(&paths).expect("reopen durable cache");
        assert_eq!(
            model
                .rows
                .get(&ledger)
                .expect("persisted previous row")
                .summary,
            previous.summary
        );
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn sqlite_index_round_trips_a_populated_row() {
        let root = scratch("round-trip");
        let paths = paths(root.clone());
        let mut model = CenterModel::open(&paths).expect("open index");
        let ledger = root.join("repo/session.json");
        model.rows.insert(
            ledger.clone(),
            CenterRow {
                summary: serde_json::from_str(&cached_summary()).expect("summary"),
                session: None,
                repo_key: "repo".to_string(),
                repo_path: "/repo".to_string(),
                ledger_path: ledger.clone(),
                modified: UNIX_EPOCH + Duration::new(42, 7),
                size: 99,
                live_holder: None,
                live: false,
            },
        );
        model.persist().expect("persist row");
        drop(model);
        let reopened = CenterModel::open(&paths).expect("reopen index");
        let row = reopened.rows.get(&ledger).expect("row restored");
        assert_eq!(row.repo_key, "repo");
        assert_eq!(row.size, 99);
        assert_eq!(row.modified, UNIX_EPOCH + Duration::new(42, 7));
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn deleting_the_disposable_index_rebuilds_from_ledgers() {
        let root = scratch("deleted-index");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("initial discovery");
        let summary = model
            .rows
            .get(&ledger)
            .expect("initial row")
            .summary
            .clone();
        drop(model);

        std::fs::remove_file(paths.index.as_std_path()).expect("delete index");
        let mut rebuilt = CenterModel::open(&paths).expect("reopen rebuilt index");
        rebuilt.discover(&paths).expect("rebuild from ledger");
        assert_eq!(
            rebuilt.rows.get(&ledger).expect("rebuilt row").summary,
            summary
        );
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn discovery_reads_direct_unindexed_store_and_deletes_missing_ledger() {
        let root = scratch("direct-discovery");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "unindexed-repository", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");

        model.discover(&paths).expect("discover direct store");
        let row = model.rows.get(&ledger).expect("unindexed ledger found");
        assert_eq!(row.repo_key, "unindexed-repository");
        assert!(row.repo_path.is_empty());

        std::fs::remove_file(ledger.as_std_path()).expect("remove ledger");
        model.discover(&paths).expect("rediscover after removal");
        assert!(!model.rows.contains_key(&ledger));
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn persistence_removes_only_deleted_session_cache_entries() {
        let root = scratch("session-cache-retention");
        let paths = paths(root.clone());
        let removed = write_fixture_ledger(&root, "removed", "completed");
        let retained = write_fixture_ledger(&root, "retained", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("populate cache");
        assert_eq!(
            model
                .db
                .query_row("SELECT COUNT(*) FROM center_sessions", [], |row| row
                    .get::<_, i64>(0))
                .expect("count cached sessions"),
            2
        );

        std::fs::remove_file(removed.as_std_path()).expect("remove ledger");
        model.discover(&paths).expect("reconcile deletion");
        assert_eq!(
            model
                .db
                .query_row(
                    "SELECT COUNT(*) FROM center_sessions WHERE ledger = ?1",
                    params![removed.as_str()],
                    |row| row.get::<_, i64>(0),
                )
                .expect("deleted cache entry is gone"),
            0
        );
        assert_eq!(
            model
                .db
                .query_row(
                    "SELECT COUNT(*) FROM center_sessions WHERE ledger = ?1",
                    params![retained.as_str()],
                    |row| row.get::<_, i64>(0),
                )
                .expect("surviving cache entry remains"),
            1
        );
        drop(model);

        let reopened = CenterModel::open(&paths).expect("reopen index");
        assert!(
            reopened
                .rows
                .get(&retained)
                .is_some_and(|row| row.session.is_some()),
            "the surviving cached session remains queryable after restart"
        );
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn short_driver_handshake_timeout_preserves_an_existing_listener() {
        // AF_UNIX pathnames are short on Darwin, so keep this socket fixture
        // directly under /tmp instead of the deeper generic test directory.
        let root = Utf8PathBuf::from(format!(
            "/tmp/ctx-center-timeout-{}-{}",
            std::process::id(),
            epoch_millis()
        ));
        let _ = std::fs::remove_dir_all(root.as_std_path());
        std::fs::create_dir_all(root.as_std_path()).expect("create scratch directory");
        let paths = paths(root.clone());
        let listener = UnixListener::bind(paths.socket.as_std_path()).expect("bind listener");
        let server = std::thread::spawn(move || {
            let (_stream, _) = listener.accept().expect("accept driver hello");
            std::thread::sleep(Duration::from_millis(100));
        });

        assert!(
            prepare_spawn(&paths, Duration::from_millis(25))
                .expect("a timed-out connected socket is inconclusive"),
            "driver startup must not replace a center merely because its short handshake timed out"
        );
        assert!(paths.socket.exists(), "inconclusive listener was unlinked");
        server.join().expect("listener exits");
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn discovery_repairs_an_unheld_nonterminal_ledger() {
        let root = scratch("orphan-repair");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "awaiting-agent-output");
        let mut model = CenterModel::open(&paths).expect("open index");

        model.discover(&paths).expect("discover orphan");
        let row = model.rows.get(&ledger).expect("repaired row");
        assert!(terminal(&row.summary));
        assert!(!row.live);
        assert!(
            crate::run_session::read_run_session(&ledger)
                .expect("read repaired ledger")
                .last_drive_outcome
                .is_some()
        );
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn discovery_rechecks_terminal_row_and_clears_stale_metadata() {
        let root = scratch("terminal-metadata");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let lock_path = crate::run_control::driver_lock_path(&ledger);
        let mut lock =
            crate::file_lock::open_lock_file_no_follow(&lock_path).expect("open stale driver lock");
        crate::file_lock::write_lock_metadata(
            &mut lock,
            &crate::run_control::DriverHolder {
                pid: 1,
                session_id: "stale-session".to_string(),
                run_id: "stale-run".to_string(),
                started_at_epoch_secs: 1,
                control_token: String::new(),
            },
        )
        .expect("write stale metadata");
        let mut model = CenterModel::open(&paths).expect("open index");

        model.discover(&paths).expect("discover terminal ledger");
        assert!(!model.rows.get(&ledger).expect("terminal row").live);
        assert!(
            crate::file_lock::read_lock_metadata::<crate::run_control::DriverHolder>(&mut lock)
                .is_none()
        );
        // The cached terminal projection remains usable on subsequent scans;
        // only the lock probe and stale-metadata maintenance check are needed.
        let cached_modified = model.rows.get(&ledger).expect("cached row").modified;
        model
            .discover(&paths)
            .expect("rediscover unchanged terminal ledger");
        assert_eq!(
            model.rows.get(&ledger).expect("cached row").modified,
            cached_modified
        );
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn unchanged_terminal_fingerprint_does_not_parse() {
        let root = scratch("unchanged-terminal");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");

        model.discover(&paths).expect("initial discovery");
        LEDGER_READS.with(|reads| reads.set(0));
        model.discover(&paths).expect("unchanged discovery");
        LEDGER_READS.with(|reads| assert_eq!(reads.get(), 0));
        assert!(terminal(
            &model.rows.get(&ledger).expect("cached row").summary
        ));
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn model_queries_never_reopen_a_constructed_ledger() {
        let root = scratch("model-queries");
        let paths = paths(root.clone());
        let _ledger = write_fixture_ledger(&root, "repository", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("construct model");
        LEDGER_READS.with(|reads| reads.set(0));
        for request in [
            Request::List {
                id: next_id("test"),
                repo_key: None,
            },
            Request::Get {
                id: next_id("test"),
                session_id: "session-fixture".to_string(),
                repo_key: None,
            },
            Request::Resolve {
                id: next_id("test"),
                session_id: "session-fixture".to_string(),
                repo_key: None,
            },
            Request::Resolve {
                id: next_id("test"),
                session_id: "session".to_string(),
                repo_key: None,
            },
            Request::FindByRunId {
                id: next_id("test"),
                run_id: "run-fixture".to_string(),
                repo_key: None,
            },
            Request::Stats {
                id: next_id("test"),
                since_epoch: None,
                trait_id: None,
                repo_key: None,
            },
        ] {
            handle_request(&mut model, &paths, request).expect("cached query");
        }
        LEDGER_READS.with(|reads| assert_eq!(reads.get(), 0));
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn fingerprint_matching_session_cache_serves_every_query_without_ledger_reads() {
        let root = scratch("restart-query-cache");
        let paths = paths(root.clone());
        let _ledger = write_fixture_ledger(&root, "repository", "completed");
        let mut initial = CenterModel::open(&paths).expect("open index");
        initial.discover(&paths).expect("populate index");
        drop(initial);

        let mut reopened = CenterModel::open(&paths).expect("reopen cached index");
        assert!(reopened.rows.values().all(|row| row.session.is_some()));
        LEDGER_READS.with(|reads| reads.set(0));
        for request in [
            Request::List {
                id: next_id("test"),
                repo_key: Some("repository".to_string()),
            },
            Request::Get {
                id: next_id("test"),
                session_id: "session-fixture".to_string(),
                repo_key: Some("repository".to_string()),
            },
            Request::Resolve {
                id: next_id("test"),
                session_id: "session-fixture".to_string(),
                repo_key: Some("repository".to_string()),
            },
            Request::Resolve {
                id: next_id("test"),
                session_id: "session".to_string(),
                repo_key: Some("repository".to_string()),
            },
            Request::FindByRunId {
                id: next_id("test"),
                run_id: "run-fixture".to_string(),
                repo_key: Some("repository".to_string()),
            },
            Request::Stats {
                id: next_id("test"),
                since_epoch: None,
                trait_id: None,
                repo_key: Some("repository".to_string()),
            },
        ] {
            handle_request(&mut reopened, &paths, request).expect("cached query");
        }
        LEDGER_READS.with(|reads| assert_eq!(reads.get(), 0));
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn repository_scoped_queries_exclude_other_rows_without_ledger_reads() {
        let root = scratch("scoped-queries");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "first", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("construct model");
        let mut second = model.rows.get(&ledger).expect("first row").clone();
        second.repo_key = "second".to_string();
        second.ledger_path = root.join("second/session-fixture.json");
        model.rows.insert(second.ledger_path.clone(), second);
        LEDGER_READS.with(|reads| reads.set(0));

        assert!(matches!(
            handle_request(&mut model, &paths, Request::List { id: next_id("test"), repo_key: Some("first".to_string()) }),
            Ok(ResponseResult::List(rows)) if rows.len() == 1 && rows[0].repo_key == "first"
        ));
        assert!(matches!(
            handle_request(
                &mut model,
                &paths,
                Request::Get {
                    id: next_id("test"),
                    session_id: "session-fixture".to_string(),
                    repo_key: Some("missing".to_string())
                }
            ),
            Ok(ResponseResult::Get(GetWireResult::Missing))
        ));
        assert!(matches!(
            handle_request(&mut model, &paths, Request::Resolve { id: next_id("test"), session_id: "session".to_string(), repo_key: Some("first".to_string()) }),
            Ok(ResponseResult::Resolve(ResolveWireResult::Row(row))) if row.repo_key == "first"
        ));
        assert!(matches!(
            handle_request(&mut model, &paths, Request::FindByRunId { id: next_id("test"), run_id: "run-fixture".to_string(), repo_key: Some("first".to_string()) }),
            Ok(ResponseResult::FindByRunId(rows)) if rows.len() == 1 && rows[0].repo_key == "first"
        ));
        assert!(matches!(
            handle_request(&mut model, &paths, Request::Stats { id: next_id("test"), since_epoch: None, trait_id: None, repo_key: Some("first".to_string()) }),
            Ok(ResponseResult::Stats(report)) if report.total_runs == 1
        ));
        LEDGER_READS.with(|reads| assert_eq!(reads.get(), 0));
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn cached_resolution_prefers_exact_ids_and_reports_prefix_ambiguity() {
        let root = scratch("resolve");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("construct model");
        let mut other = model.rows.get(&ledger).expect("fixture row").clone();
        let other_ledger = root.join("repository/session-other.json");
        other.ledger_path = other_ledger.clone();
        other.summary.session_id = "session-fixture-other".to_string();
        model.rows.insert(other_ledger, other);

        let exact = handle_request(
            &mut model,
            &paths,
            Request::Resolve {
                id: next_id("test"),
                session_id: "session-fixture".to_string(),
                repo_key: None,
            },
        )
        .expect("exact resolve");
        assert!(
            matches!(exact, ResponseResult::Resolve(ResolveWireResult::Row(row)) if row.summary.session_id == "session-fixture")
        );
        let ambiguous = handle_request(
            &mut model,
            &paths,
            Request::Resolve {
                id: next_id("test"),
                session_id: "session".to_string(),
                repo_key: None,
            },
        )
        .expect("prefix resolve");
        assert!(
            matches!(ambiguous, ResponseResult::Resolve(ResolveWireResult::Ambiguous(ids)) if ids.len() == 2)
        );
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn duplicate_ids_are_deterministic_and_exact_lookup_is_not_arbitrary() {
        let root = scratch("duplicate-id-order");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "first", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("construct model");
        let mut duplicate = model.rows.get(&ledger).expect("fixture row").clone();
        duplicate.repo_key = "second".to_string();
        duplicate.ledger_path = root.join("second/session-fixture.json");
        model.rows.insert(duplicate.ledger_path.clone(), duplicate);

        let ids = match handle_request(
            &mut model,
            &paths,
            Request::Resolve {
                id: next_id("test"),
                session_id: "session-fixture".to_string(),
                repo_key: None,
            },
        )
        .expect("resolve duplicate exact")
        {
            ResponseResult::Resolve(ResolveWireResult::Ambiguous(ids)) => ids,
            other => panic!("unexpected resolution: {other:?}"),
        };
        assert_eq!(ids, vec!["session-fixture", "session-fixture"]);
        let rows = public_rows(&model, None);
        assert_eq!(rows[0].repo_key, "first");
        assert_eq!(rows[1].repo_key, "second");
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn subscription_snapshot_precedes_later_deltas_and_backpressure_disconnects() {
        let root = scratch("subscription");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("construct model");
        let (sender, receiver) = mpsc::sync_channel(4);
        model
            .subscribe(
                1,
                "test-subscription".to_string(),
                Some("repository".to_string()),
                sender,
            )
            .expect("subscribe");
        assert!(matches!(
            receiver.recv(),
            Ok(Outbound::SnapshotStart(id)) if id == "test-subscription"
        ));
        model.advance_snapshot(1);
        assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotRow(_))));
        model.advance_snapshot(1);
        assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotEnd)));
        model.broadcast_activity(ledger.as_str(), test_activity());
        assert!(matches!(
            receiver.recv(),
            Ok(Outbound::Delta(CenterDelta::ActivityLine { .. }))
        ));
        for _ in 0..=SUBSCRIBER_QUEUE {
            model.broadcast_activity(ledger.as_str(), test_activity());
        }
        assert!(
            model.subscribers.is_empty(),
            "full subscriber queue disconnects peer"
        );
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn delta_racing_snapshot_follows_snapshot_end() {
        let root = scratch("snapshot-race");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("construct model");
        let (sender, receiver) = mpsc::sync_channel(4);
        model
            .subscribe(1, "test-subscription".to_string(), None, sender)
            .expect("subscribe");
        assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotStart(_))));

        // This update arrives while the writer still has snapshot rows to
        // stream. It must be retained until its SnapshotEnd acknowledgement.
        model.broadcast_activity(ledger.as_str(), test_activity());
        assert_eq!(model.subscribers.len(), 1);
        model.advance_snapshot(1);
        assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotRow(_))));
        model.advance_snapshot(1);
        assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotEnd)));
        assert!(matches!(
            receiver.recv(),
            Ok(Outbound::Delta(CenterDelta::ActivityLine { .. }))
        ));
        assert_eq!(model.subscribers.len(), 1);
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn subscriber_keeps_an_otherwise_idle_center_alive_until_cleanup() {
        let root = scratch("subscriber-liveness");
        let paths = paths(root.clone());
        let mut model = CenterModel::open(&paths).expect("open index");
        let (sender, _receiver) = mpsc::sync_channel(1);
        model
            .subscribe(1, "test-subscription".to_string(), None, sender)
            .expect("subscribe");
        assert!(
            model.has_live(),
            "an open subscription prevents idle shutdown"
        );
        model.subscribers.remove(&1);
        assert!(!model.has_live(), "cleanup restores ordinary idle shutdown");
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn large_snapshot_uses_one_bounded_queue_slot() {
        let root = scratch("large-subscription");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("construct model");
        let prototype = model.rows.get(&ledger).expect("fixture row").clone();
        for index in 0..(SUBSCRIBER_QUEUE * 2) {
            let mut row = prototype.clone();
            row.ledger_path = root.join(format!("repository/session-{index}.json"));
            row.summary.session_id = format!("session-{index:03}");
            model.rows.insert(row.ledger_path.clone(), row);
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        model
            .subscribe(
                1,
                "test-subscription".to_string(),
                Some("repository".to_string()),
                sender,
            )
            .expect("large snapshot starts without inventory queueing");
        assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotStart(_))));
        for _ in 0..(SUBSCRIBER_QUEUE * 2 + 1) {
            model.advance_snapshot(1);
            assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotRow(_))));
        }
        model.advance_snapshot(1);
        assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotEnd)));
        assert_eq!(model.subscribers.len(), 1);
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn terminal_notification_fans_out_a_retained_row_change() {
        let root = scratch("ended-delta");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "awaiting-agent-output");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("construct model");
        let (sender, receiver) = mpsc::sync_channel(2);
        model
            .subscribe(1, "test-subscription".to_string(), None, sender)
            .expect("subscribe");
        let _ = receiver.recv().expect("snapshot start");
        model.advance_snapshot(1);
        let _ = receiver.recv().expect("snapshot row");
        model.advance_snapshot(1);
        let _ = receiver.recv().expect("snapshot end");
        crate::run_session::write_run_session(&ledger, &fixture_session("completed"))
            .expect("write final outcome");
        handle_request(
            &mut model,
            &paths,
            Request::Ended {
                id: next_id("test"),
                ledger_path: ledger.to_string(),
            },
        )
        .expect("ended notification");
        assert!(matches!(
            receiver.recv(),
            Ok(Outbound::Delta(CenterDelta::RowChanged { .. }))
        ));
        handle_request(
            &mut model,
            &paths,
            Request::Ended {
                id: next_id("test"),
                ledger_path: ledger.to_string(),
            },
        )
        .expect("repeated ended notification");
        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout),
        ));
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn repository_scoped_delta_does_not_unregister_other_subscribers() {
        let root = scratch("subscription-scope");
        let paths = paths(root.clone());
        let first = write_fixture_ledger(&root, "first", "completed");
        let _second = write_fixture_ledger(&root, "second", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("construct model");
        let (first_sender, first_receiver) = mpsc::sync_channel(8);
        let (second_sender, second_receiver) = mpsc::sync_channel(8);
        model
            .subscribe(
                1,
                "test-first".to_string(),
                Some("first".to_string()),
                first_sender,
            )
            .expect("subscribe first");
        model
            .subscribe(
                2,
                "test-second".to_string(),
                Some("second".to_string()),
                second_sender,
            )
            .expect("subscribe second");
        for (id, receiver) in [(1, &first_receiver), (2, &second_receiver)] {
            assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotStart(_))));
            model.advance_snapshot(id);
            assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotRow(_))));
            model.advance_snapshot(id);
            assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotEnd)));
        }

        model.broadcast_activity(first.as_str(), test_activity());
        assert!(matches!(
            first_receiver.recv(),
            Ok(Outbound::Delta(CenterDelta::ActivityLine { .. }))
        ));
        assert!(matches!(
            second_receiver.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert_eq!(model.subscribers.len(), 2);
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn refresh_ledger_parses_at_most_once_for_changed_orphan_repair() {
        let root = scratch("changed-terminal");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");

        model.discover(&paths).expect("initial discovery");
        crate::run_session::write_run_session(&ledger, &fixture_session("awaiting-agent-output"))
            .expect("replace with nonterminal ledger");
        LEDGER_READS.with(|reads| reads.set(0));
        model.discover(&paths).expect("changed discovery");

        let row = model.rows.get(&ledger).expect("repaired row");
        assert!(terminal(&row.summary));
        LEDGER_READS.with(|reads| assert_eq!(reads.get(), 1));
        assert!(
            crate::run_session::read_run_session(&ledger)
                .expect("read repaired ledger")
                .last_drive_outcome
                .is_some()
        );
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn refresh_ledger_uses_one_parse_when_the_cached_terminal_row_changes() {
        let root = scratch("terminal-post-stat-change");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");

        model.discover(&paths).expect("initial discovery");
        // A changed ledger must be parsed once and carried through orphan
        // repair while maintenance ownership excludes a replacement driver.
        crate::run_session::write_run_session(&ledger, &fixture_session("awaiting-agent-output"))
            .expect("replace with nonterminal ledger");
        LEDGER_READS.with(|reads| reads.set(0));
        model
            .refresh_ledger(&paths, &ledger)
            .expect("maintenance repairs changed ledger");

        let row = model.rows.get(&ledger).expect("repaired row");
        assert!(terminal(&row.summary));
        LEDGER_READS.with(|reads| assert_eq!(reads.get(), 1));
        assert!(
            crate::run_session::read_run_session(&ledger)
                .expect("read repaired ledger")
                .last_drive_outcome
                .is_some()
        );
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn refresh_ledger_parses_at_most_once() {
        for (name, initial, replacement, repairs_orphan) in [
            ("stat-race-changed-terminal", "failed", "completed", false),
            (
                "stat-race-orphan-repair",
                "awaiting-agent-output",
                "awaiting-agent-output",
                true,
            ),
        ] {
            let root = scratch(name);
            let paths = paths(root.clone());
            let ledger = write_fixture_ledger(&root, "repository", "completed");
            let mut model = CenterModel::open(&paths).expect("open index");
            model.discover(&paths).expect("initial discovery");
            crate::run_session::write_run_session(&ledger, &fixture_session(initial))
                .expect("stage changed ledger");

            // Hold the driver lock through the replacement, matching the live
            // writer's atomic-save window. The checkpoint fires after refresh
            // has captured its candidate fingerprint but before it parses.
            let lock_path = crate::run_control::driver_lock_path(&ledger);
            let lock =
                crate::file_lock::open_lock_file_no_follow(&lock_path).expect("open driver lock");
            crate::file_lock::lock_exclusive_blocking(&lock).expect("hold driver lock");
            let replacement_ledger = ledger.clone();
            let replacement_session = fixture_session(replacement);
            AFTER_REFRESH_STAT.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    crate::run_session::write_run_session(
                        &replacement_ledger,
                        &replacement_session,
                    )
                    .expect("replace ledger after refresh stat");
                }));
            });
            LEDGER_READS.with(|reads| reads.set(0));
            assert!(model.refresh_ledger(&paths, &ledger).is_err());
            LEDGER_READS.with(|reads| assert_eq!(reads.get(), 1));
            drop(lock);

            // The next shared refresh repairs the replacement. In particular,
            // an orphaned nonterminal ledger is persisted as interrupted using
            // the same single parsed Session, not a second ledger read.
            LEDGER_READS.with(|reads| reads.set(0));
            let deadline = Instant::now() + Duration::from_secs(1);
            loop {
                model
                    .refresh_ledger(&paths, &ledger)
                    .expect("refresh replacement after driver release");
                if !repairs_orphan
                    || terminal(&model.rows.get(&ledger).expect("refreshed row").summary)
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "maintenance lock did not release"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            LEDGER_READS.with(|reads| assert!(reads.get() <= 1));
            if repairs_orphan {
                assert!(terminal(
                    &model.rows.get(&ledger).expect("repaired row").summary
                ));
                assert!(
                    crate::run_session::read_run_session(&ledger)
                        .expect("read repaired ledger")
                        .last_drive_outcome
                        .is_some()
                );
            }
            let _ = std::fs::remove_dir_all(root.as_std_path());
        }
    }

    #[test]
    fn unchanged_ledger_refreshes_repo_metadata_without_a_ledger_read() {
        let root = scratch("repo-metadata-refresh");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");
        let original = HashMap::from([("repository".to_string(), "/old/path".to_string())]);
        model
            .refresh_ledger_inner(&paths, &ledger, false, Some(&original))
            .expect("cache ledger");
        let moved = HashMap::from([("repository".to_string(), "/new/path".to_string())]);
        LEDGER_READS.with(|reads| reads.set(0));
        model
            .refresh_ledger_inner(&paths, &ledger, true, Some(&moved))
            .expect("refresh moved repository metadata");
        assert_eq!(
            model.rows.get(&ledger).expect("cached row").repo_path,
            "/new/path"
        );
        LEDGER_READS.with(|reads| assert_eq!(reads.get(), 0));
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn unavailable_repo_index_retains_last_good_metadata_and_marks_uncertain() {
        let root = scratch("repo-metadata-failure");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");
        let indexed = HashMap::from([("repository".to_string(), "/known/path".to_string())]);
        model
            .refresh_ledger_inner(&paths, &ledger, false, Some(&indexed))
            .expect("cache ledger");
        LEDGER_READS.with(|reads| reads.set(0));
        model.uncertain = true;
        model
            .refresh_ledger_inner(&paths, &ledger, true, None)
            .expect("retain cached metadata while index is unavailable");
        assert_eq!(
            model.rows.get(&ledger).expect("cached row").repo_path,
            "/known/path"
        );
        assert!(model.uncertain);
        LEDGER_READS.with(|reads| assert_eq!(reads.get(), 0));
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn held_driver_lock_is_live_with_or_without_metadata() {
        for with_metadata in [false, true] {
            let root = scratch(if with_metadata {
                "held-with-metadata"
            } else {
                "held-without-metadata"
            });
            let paths = paths(root.clone());
            let ledger = write_fixture_ledger(&root, "repository", "awaiting-agent-output");
            let lock_path = crate::run_control::driver_lock_path(&ledger);
            let mut lock =
                crate::file_lock::open_lock_file_no_follow(&lock_path).expect("open driver lock");
            crate::file_lock::lock_exclusive_blocking(&lock).expect("hold driver lock");
            if with_metadata {
                crate::file_lock::write_lock_metadata(
                    &mut lock,
                    &crate::run_control::DriverHolder {
                        pid: std::process::id(),
                        session_id: "session".to_string(),
                        run_id: "run".to_string(),
                        started_at_epoch_secs: 1,
                        control_token: "token".to_string(),
                    },
                )
                .expect("write holder metadata");
            }
            let mut model = CenterModel::open(&paths).expect("open index");
            model.discover(&paths).expect("discover held driver");
            let row = model.rows.get(&ledger).expect("held row");
            assert!(row.live);
            assert_eq!(row.live_holder.is_some(), with_metadata);
            drop(lock);
            let _ = std::fs::remove_dir_all(root.as_std_path());
        }
    }

    #[test]
    fn maintenance_handoff_to_a_driver_remains_live() {
        let root = scratch("maintenance-handoff");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("discover terminal ledger");

        // This stages the gap between an uncontended probe and maintenance
        // acquisition: the driver wins, so repair must classify it live.
        let lock_path = crate::run_control::driver_lock_path(&ledger);
        let lock = crate::file_lock::open_lock_file_no_follow(&lock_path).expect("open lock");
        crate::file_lock::lock_exclusive_blocking(&lock).expect("driver wins lock");
        model
            .refresh_after_probe(
                &ledger,
                true,
                crate::run_control::DriverProbe::Unheld {
                    stale_metadata: None,
                },
            )
            .expect("maintenance handoff");
        assert!(model.rows.get(&ledger).expect("row").live);
        drop(lock);
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn failed_liveness_probe_retains_cached_row_and_blocks_idle_exit() {
        let root = scratch("failed-probe");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("initial discovery");
        let previous = model.rows.get(&ledger).expect("cached row").summary.clone();

        // A symlink at the no-follow lock path makes maintenance acquisition
        // fail after the probe's initial unheld result.
        let lock_path = crate::run_control::driver_lock_path(&ledger);
        std::fs::remove_file(lock_path.as_std_path()).expect("remove maintenance lock file");
        let symlink_target = root.join("not-a-lock");
        std::fs::write(symlink_target.as_std_path(), b"not a lock").expect("write symlink target");
        std::os::unix::fs::symlink(&symlink_target, lock_path.as_std_path())
            .expect("create invalid lock path");
        model.discover(&paths).expect("failed probe is retained");
        let row = model.rows.get(&ledger).expect("retained row");
        assert_eq!(row.summary, previous);
        assert!(!row.live, "failed probe must not alter a verified row");
        assert!(model.has_live());
        std::fs::remove_file(lock_path.as_std_path()).expect("remove invalid lock path");
        model.discover(&paths).expect("retry liveness probe");
        assert!(!model.has_live());
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn changed_ledger_probe_failure_rolls_back_the_candidate_until_retry() {
        let root = scratch("changed-probe-rollback");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("initial discovery");
        let previous = model.rows.get(&ledger).expect("cached row").clone();

        // Change both the parsed summary and fingerprint, then make the
        // authoritative driver lock path unprobeable. The candidate must never
        // leak through the in-memory model or its persisted cache.
        crate::run_session::write_run_session(&ledger, &fixture_session("failed"))
            .expect("replace ledger");
        let lock_path = crate::run_control::driver_lock_path(&ledger);
        let _ = std::fs::remove_file(lock_path.as_std_path());
        let symlink_target = root.join("not-a-lock");
        std::fs::write(symlink_target.as_std_path(), b"not a lock").expect("write symlink target");
        std::os::unix::fs::symlink(&symlink_target, lock_path.as_std_path())
            .expect("make invalid lock path");
        assert!(model.refresh_ledger(&paths, &ledger).is_err());
        let retained = model.rows.get(&ledger).expect("previous row remains");
        assert_eq!(retained.summary, previous.summary);
        assert_eq!(retained.modified, previous.modified);
        assert_eq!(retained.size, previous.size);

        drop(model);
        let mut model = CenterModel::open(&paths).expect("reopen verified cache");
        assert_eq!(
            model
                .rows
                .get(&ledger)
                .expect("persisted previous row")
                .summary,
            previous.summary,
            "a failed refresh must not persist its candidate"
        );
        std::fs::remove_file(lock_path.as_std_path()).expect("restore lock path");
        model.discover(&paths).expect("retry changed ledger");
        assert_eq!(
            model
                .rows
                .get(&ledger)
                .expect("refreshed row")
                .summary
                .status,
            ctx_traits_core::procedure::session::Status::Failed
        );
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn newly_discovered_ledger_probe_failure_is_not_cached_or_announced_until_retry() {
        let root = scratch("new-probe-rollback");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let lock_path = crate::run_control::driver_lock_path(&ledger);
        let _ = std::fs::remove_file(lock_path.as_std_path());
        let target = root.join("not-a-lock");
        std::fs::write(target.as_std_path(), b"not a lock").expect("write target");
        std::os::unix::fs::symlink(&target, lock_path.as_std_path())
            .expect("make lock path unprobeable");

        let mut model = CenterModel::open(&paths).expect("open index");
        let (sender, receiver) = mpsc::sync_channel(2);
        model
            .subscribe(1, "test-subscription".to_string(), None, sender)
            .expect("subscribe");
        let _ = receiver.recv().expect("snapshot start");
        model.advance_snapshot(1);
        let _ = receiver.recv().expect("snapshot end");
        model
            .discover(&paths)
            .expect("failed discovery is retained for retry");
        assert!(!model.rows.contains_key(&ledger));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        drop(model);

        let mut model = CenterModel::open(&paths).expect("reopen index");
        assert!(
            !model.rows.contains_key(&ledger),
            "candidate was not persisted"
        );
        std::fs::remove_file(lock_path.as_std_path()).expect("restore lock path");
        model.discover(&paths).expect("retry discovery");
        assert!(model.rows.contains_key(&ledger));
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn every_invalid_cached_field_names_the_removable_index() {
        for (name, secs, nanos, size, summary) in [
            ("negative-seconds", -1, 0, 0, cached_summary()),
            ("invalid-nanos", 0, 1_000_000_000, 0, cached_summary()),
            ("negative-size", 0, 0, -1, cached_summary()),
            ("bad-summary", 0, 0, 0, "not-json".to_string()),
        ] {
            let root = scratch(name);
            let paths = paths(root.clone());
            let model = CenterModel::open(&paths).expect("open index");
            model
                .db
                .execute(
                    "INSERT INTO center_rows VALUES ('x', 'key', '', ?1, ?2, ?3, ?4)",
                    params![secs, nanos, size, summary],
                )
                .expect("insert corrupt row");
            drop(model);
            let error = match CenterModel::open(&paths) {
                Ok(_) => panic!("invalid row accepted"),
                Err(error) => error,
            };
            assert!(error.to_string().contains(paths.index.as_str()));
            assert!(error.to_string().contains("remove"));
            let _ = std::fs::remove_dir_all(root.as_std_path());
        }
    }

    #[test]
    fn max_sqlite_time_is_checked_without_panicking() {
        let root = scratch("overflowing-time");
        let paths = paths(root.clone());
        let model = CenterModel::open(&paths).expect("open index");
        model
            .db
            .execute(
                "INSERT INTO center_rows VALUES ('x', 'key', '', ?1, 0, 0, ?2)",
                params![i64::MAX, cached_summary()],
            )
            .expect("insert overflowing row");
        drop(model);
        match CenterModel::open(&paths) {
            // Some platforms can represent every timestamp expressible by the
            // signed SQLite column. Others reject this value through the
            // checked reconstruction in `open`; neither may panic.
            Ok(_) => {}
            Err(error) => {
                assert!(error.to_string().contains(paths.index.as_str()));
                assert!(error.to_string().contains("remove"));
            }
        }
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn socket_guard_does_not_remove_replacement() {
        let root = scratch("socket-guard");
        let path = root.join("center.sock");
        let original = UnixListener::bind(path.as_std_path()).expect("bind original");
        let guard = SocketGuard::for_listener(path.clone()).expect("guard");
        std::fs::remove_file(path.as_std_path()).expect("unlink original");
        let replacement = UnixListener::bind(path.as_std_path()).expect("bind replacement");
        drop(original);
        drop(guard);
        let client = UnixStream::connect(path.as_std_path()).expect("replacement remains bound");
        drop(client);
        drop(replacement);
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }
}
