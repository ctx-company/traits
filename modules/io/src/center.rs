//! Machine-wide run center: a small, disposable index over run ledgers.
//!
//! The SQLite database is only a restart cache. Ledgers and their driver
//! flocks remain authoritative, so a deleted database is rebuilt on the next
//! scan and a crashed center never affects a driver.
//! Unlike `run_session::InventoryCache`, which intentionally dies with each
//! process and makes cold dashboard processes parse again, this cache is a
//! disposable machine-wide derived index for the long-lived center.

use camino::{Utf8Path, Utf8PathBuf};
use ctx_traits_core::task::provider::TaskProvider;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
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
const ACTION_TIMEOUT: Duration = Duration::from_secs(600);
const START_STDERR_BYTES: usize = 8 * 1024;
const SPAWN_TOKEN_ENV: &str = "CTX_CENTER_SPAWN_TOKEN";
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
// Production default: the dashboard closes and reopens within seconds all
// day; a 15s lifetime forced a respawn-and-rewarm on nearly every open.
// Fixtures that need fast reaping set CTX_CENTER_IDLE_MS explicitly.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
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
// Version 3 dropped the cached `center_sessions` payload table: every row
// surviving a marker-gated rebuild was written by the metadata-only
// projection, so fingerprint match alone is a sound `unchanged` signal.
const CENTER_PROJECTION_VERSION: i64 = 3;
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
    Start {
        id: String,
        target: StartTarget,
    },
    Control {
        id: String,
        session_id: String,
        repo_key: Option<String>,
        command: ControlAction,
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
    ClaimedTask {
        id: String,
        session_id: String,
        repo_key: Option<String>,
    },
    Board {
        id: String,
        repo_key: String,
    },
    CreateTask {
        id: String,
        repo_key: String,
        new_task: ctx_traits_core::task::provider::NewTask,
    },
    TaskDetail {
        id: String,
        repo_key: String,
        task_key: String,
    },
    Library {
        id: String,
        repo_key: String,
    },
    LibraryDetail {
        id: String,
        repo_key: String,
        selector: crate::library::LibraryDetailSelector,
    },
    LibraryChangedNotice {
        id: String,
        repo_key: String,
    },
    Config {
        id: String,
        scope_path: String,
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
            | Self::Start { id, .. }
            | Self::Control { id, .. }
            | Self::List { id, .. }
            | Self::Get { id, .. }
            | Self::Resolve { id, .. }
            | Self::FindByRunId { id, .. }
            | Self::Stats { id, .. }
            | Self::StandingWall { id, .. }
            | Self::ClaimedTask { id, .. }
            | Self::Board { id, .. }
            | Self::CreateTask { id, .. }
            | Self::TaskDetail { id, .. }
            | Self::Library { id, .. }
            | Self::LibraryDetail { id, .. }
            | Self::LibraryChangedNotice { id, .. }
            | Self::Config { id, .. } => id,
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
    Hello {
        id: String,
    },
    Ready {
        id: String,
    },
    Response {
        id: String,
        result: ResponseResult,
    },
    SnapshotStart {
        id: String,
    },
    SnapshotRow {
        row: Box<CenterPublicRow>,
    },
    SnapshotEnd,
    Delta {
        delta: CenterDelta,
    },
    BoardChanged {
        repo_key: String,
        board: Box<BoardWireResult>,
    },
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
    Start(StartWireResult),
    Control(ControlWireResult),
    ClaimedTask(ClaimedTaskWireResult),
    Board(Box<BoardWireResult>),
    CreateTask(CreateTaskWireResult),
    TaskDetail(Box<TaskDetailWireResult>),
    Library(Box<LibraryWireResult>),
    LibraryDetail(Box<crate::library::LibraryDetailResolution>),
    Config(Box<ConfigWireResult>),
    Error { message: String },
}

/// Compact board data for the Tasks pane and sibling task consumers. The
/// resolution retains every board row; joins and sections are model-owned
/// facts layered onto it without another filesystem read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct BoardWireResult {
    pub resolution: crate::task_files::BoardResolution,
    pub joined_runs: BTreeMap<String, Vec<ctx_traits_core::task::provider::BoardRun>>,
    pub sections: BTreeMap<String, Option<ctx_traits_core::task::provider::BoardSection>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ConfigWireResult {
    pub repo_key: String,
    pub repo_path: String,
    pub resolution: crate::config_view::ConfigResolution,
}

/// Expected outcomes of a board-scoped task creation. These are data rather
/// than protocol failures so every face can render the same refusal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "kebab-case")]
pub enum CreateTaskWireResult {
    Created(ctx_traits_core::task::provider::TaskSummary),
    Occupied,
    InvalidField { field: String, reason: String },
    UnknownParent { parent: String },
    AmbiguousParent { parent: String },
    BoardAbsent,
    BoardUnreadable { reason: String },
}

impl std::fmt::Display for CreateTaskWireResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Created(summary) => write!(f, "created {}", summary.key),
            Self::Occupied => f.write_str("another task creation is in progress"),
            Self::InvalidField { field, reason } => write!(f, "invalid {field}: {reason}"),
            Self::UnknownParent { parent } => write!(f, "no task {parent:?} in the board"),
            Self::AmbiguousParent { parent } => write!(f, "parent {parent:?} is ambiguous"),
            Self::BoardAbsent => f.write_str("task board is absent"),
            Self::BoardUnreadable { reason } => write!(f, "task board is unreadable: {reason}"),
        }
    }
}

/// The per-repository board-freshness index: repo-key -> (last fingerprint,
/// presence, sync report, generation). Aliased to keep the several
/// threading points readable and satisfy `clippy::type_complexity`.
pub(crate) type BoardInstants = std::collections::HashMap<
    String,
    (
        Option<String>,
        crate::task_files::BoardPresence,
        ctx_traits_core::task::provider::SyncReport,
        u64,
    ),
>;

/// Complete, repository-scoped data for one selected board task. This is a
/// separate answer from `BoardWireResult` so list loading never transports
/// every task's full prose or reconstructs every claiming run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "kebab-case")]
#[allow(clippy::large_enum_variant)] // wire result; one instance per response
pub enum TaskDetailWireResult {
    Missing,
    Resolved {
        summary: ctx_traits_core::task::provider::TaskSummary,
        content: String,
        #[serde(default)]
        scope: String,
        #[serde(default)]
        validation: String,
        #[serde(default)]
        open_steps: Vec<ctx_traits_core::task::Step>,
        state: String,
        current_activity: bool,
        #[serde(default)]
        raised: Option<String>,
        #[serde(default)]
        parent: Option<String>,
        #[serde(default)]
        depends_on: Vec<String>,
        #[serde(default)]
        checks: Vec<ctx_traits_core::task::Check>,
        #[serde(default)]
        closure: Option<ctx_traits_core::task::Closure>,
        #[serde(default = "default_close_policy_resolution")]
        close_policy: ClosePolicyResolution,
        claim: TaskClaimWire,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "kebab-case")]
pub enum TaskClaimWire {
    NoClaim,
    Ambiguous(Vec<String>),
    Claim {
        run_id: String,
        trait_id: String,
        progress: Result<ctx_traits_core::procedure::run::RunProgress, String>,
        #[serde(default)]
        state: TaskClaimState,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TaskClaimState {
    #[default]
    Active,
    Pending,
    Terminal,
}

fn default_close_policy_resolution() -> ClosePolicyResolution {
    ClosePolicyResolution::NoneConfigured
}

/// Repository-scoped trait library answer. Its rows and provenance are
/// produced once in the connection worker and are safe for all clients to
/// project without rediscovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LibraryWireResult {
    pub repo_key: String,
    pub repo_path: String,
    pub resolution: crate::library::LibraryResolution,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "kebab-case")]
pub enum StartTarget {
    Trait {
        args: Vec<String>,
        repo_path: String,
    },
    Session {
        session_id: String,
        repo_key: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ControlAction {
    Interrupt,
    Pause,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum StartWireResult {
    Started { session_id: String },
    Exited { code: Option<i32>, stderr: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "kebab-case")]
enum ControlWireResult {
    Acknowledged,
    Missing,
    Ambiguous(Vec<String>),
    NotLive,
    Unverifiable,
    Refused,
}

/// The claimed task's effective `auto-close` policy, resolved by the center
/// at the row's own validated `repo_root` (never the process cwd — see
/// [`ctx_traits_io::harness_config::effective_auto_close_policy`]) and
/// carried beside [`ctx_traits_core::task::provider::ClaimedTask`] rather
/// than folded into it, so that shared core type stays untouched for its
/// other constructors. Three outcomes stay distinguishable end to end: a
/// resolved policy, a resolved absence of one, and a resolution failure —
/// collapsing the latter two would let a config-read error masquerade as
/// "no policy configured".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "kebab-case")]
pub enum ClosePolicyResolution {
    Effective(ctx_traits_core::task::AutoClosePolicy),
    NoneConfigured,
    /// Config resolution failed; carries the error text for diagnostics.
    /// Never defaulted to a plausible policy.
    Unresolved(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "kebab-case")]
enum ClaimedTaskWireResult {
    Missing,
    Ambiguous(Vec<String>),
    Unclaimed,
    Task(
        Box<ctx_traits_core::task::provider::ClaimedTask>,
        ClosePolicyResolution,
    ),
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
    LibraryChanged {
        repo_keys: Vec<String>,
    },
    ConfigChanged {
        repo_keys: Vec<String>,
    },
}

impl CenterDelta {
    /// The row every variant carries, regardless of what it means for that
    /// row's presence in a keyed map.
    pub fn row(&self) -> &CenterPublicRow {
        match self {
            Self::Appeared { row }
            | Self::RowChanged { row }
            | Self::Ended { row }
            | Self::ActivityLine { row, .. } => row,
            Self::LibraryChanged { .. } | Self::ConfigChanged { .. } => {
                panic!("repository change has no run row")
            }
        }
    }

    pub fn ledger_path(&self) -> &str {
        &self.row().ledger_path
    }

    /// The one canonical rule for folding a delta into a `ledger_path`-keyed
    /// row map, shared by every consumer of the center subscription stream.
    ///
    /// A terminal drive is a changed row, not a disappearance: only `Ended`
    /// removes an entry. `ActivityLine` never changes the row map — the
    /// center follows a title activity line with an explicit `RowChanged`.
    /// Returns the affected `ledger_path` regardless of variant, since some
    /// callers (e.g. a selected detail view) care about identity even when
    /// the map itself did not change.
    pub fn apply_to(self, rows: &mut HashMap<String, CenterPublicRow>) -> String {
        match self {
            Self::Appeared { row } | Self::RowChanged { row } => {
                let ledger_path = row.ledger_path.clone();
                rows.insert(ledger_path.clone(), *row);
                ledger_path
            }
            Self::Ended { row } => {
                rows.remove(&row.ledger_path);
                row.ledger_path.clone()
            }
            Self::ActivityLine { row, .. } => row.ledger_path.clone(),
            Self::LibraryChanged { .. } | Self::ConfigChanged { .. } => {
                panic!("repository change cannot update run rows")
            }
        }
    }
}

/// Canonical driver identity supplied by the held driver lock.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriverRegistration {
    pub ledger_path: String,
    pub holder: crate::run_control::DriverHolder,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn_token: Option<String>,
}

/// Read the center-generated start correlation token, if this driver was
/// launched through the center.
pub fn spawn_token_from_env() -> Option<String> {
    std::env::var(SPAWN_TOKEN_ENV).ok()
}

#[derive(Debug, Clone)]
enum DriverEvent {
    Register {
        acknowledged: Option<mpsc::SyncSender<()>>,
    },
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
        Self::new_with(registration, notifier_worker)
    }

    fn new_with(
        registration: DriverRegistration,
        worker: impl FnOnce(DriverRegistration, mpsc::Receiver<DriverEvent>) + Send + 'static,
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel(notifier_queue_capacity());
        let waits_for_registration = registration.spawn_token.is_some();
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
            .spawn(move || worker(registration, receiver))
            .is_err()
        {
            // Drop the receiver so producers retain their non-blocking,
            // failure-isolated behavior through `try_send` below.
        }
        let notifier = Self { sender };
        if waits_for_registration {
            let (acknowledged, registered) = mpsc::sync_channel(1);
            notifier.register_with_acknowledgement(acknowledged);
            // A center-started driver must give the center a chance to correlate
            // its registration before a short run can terminate. Other notifier
            // events remain best effort, and a missing center remains bounded.
            // Registration retries may each consume STREAM_TIMEOUT. Keep a
            // center-started driver gated for the full start correlation lease
            // so a fast exit cannot outrun a later successful retry.
            let _ = registered.recv_timeout(ACTION_TIMEOUT);
        } else {
            notifier.register();
        }
        notifier
    }

    pub fn register(&self) {
        let _ = self
            .sender
            .try_send(DriverEvent::Register { acknowledged: None });
    }

    fn register_with_acknowledgement(&self, acknowledged: mpsc::SyncSender<()>) {
        let _ = self.sender.try_send(DriverEvent::Register {
            acknowledged: Some(acknowledged),
        });
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
        let (wire_event, registration_acknowledged) = match &event {
            // Registration is sent as the first line for every connection.
            // Do not duplicate it merely because this is the initial event.
            DriverEvent::Register { acknowledged } => (None, acknowledged.as_ref()),
            DriverEvent::FrameDone => (Some(("frame-done", None)), None),
            DriverEvent::ActivityLine(record) => {
                (Some(("activity-line", Some(record.clone()))), None)
            }
            DriverEvent::Ended => (Some(("ended", None)), None),
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
            if let Some(acknowledged) = registration_acknowledged {
                let _ = acknowledged.try_send(());
            }
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
    /// Where `run_liveness::read_index`/`upsert_row` recover a repository-
    /// local, no-driver-event ledger (0262 defect 5). Production always
    /// resolves this to the single machine-wide `run_control::runtime_root`,
    /// exactly where every real driver registers, regardless of the
    /// override below — that sharing is what makes recovery possible at
    /// all. The override exists only so an isolated private tuple (a test
    /// fixture) does not read whatever unrelated drivers left behind at the
    /// shared path.
    pub liveness_root: Utf8PathBuf,
}

/// Resolve the one private configuration tuple shared by every client and the
/// sentinel. Partial tuples are always rejected rather than silently mixing a
/// test endpoint with production state.
fn center_paths() -> crate::Result<CenterPaths> {
    let configured = |name: &str| std::env::var(name).ok().map(Utf8PathBuf::from);
    let liveness_root =
        configured("CTX_CENTER_LIVENESS_ROOT").unwrap_or_else(crate::run_control::runtime_root);
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
            liveness_root,
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
        liveness_root: crate::run_control::runtime_root(),
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
    write_raw_line(stream, &line)
}

fn write_raw_line(stream: &mut UnixStream, line: &[u8]) -> crate::Result<()> {
    if line.len() > MAX_LINE_BYTES {
        return Err(protocol_error("line exceeds limit"));
    }
    // O_NONBLOCK lives on the shared file description, not the descriptor, so
    // a concurrent `read_line_bounded` on a `try_clone` of this socket (the
    // subscription EOF watcher) flips this writer into nonblocking mode for
    // as long as that read is parked. `write_all` treats the resulting
    // WouldBlock as fatal and tears the stream down mid-line. Write with an
    // explicit deadline instead: transient WouldBlock retries, while a peer
    // that genuinely stops reading still fails within STREAM_TIMEOUT.
    let deadline = Instant::now() + STREAM_TIMEOUT;
    let mut written = 0;
    while written < line.len() {
        match stream.write(&line[written..]) {
            Ok(0) => {
                return Err(io_error(
                    &Utf8PathBuf::from("center socket"),
                    std::io::Error::from(std::io::ErrorKind::WriteZero),
                ));
            }
            Ok(count) => written += count,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if Instant::now() >= deadline {
                    return Err(io_error(&Utf8PathBuf::from("center socket"), error));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(source) => {
                return Err(io_error(&Utf8PathBuf::from("center socket"), source));
            }
        }
    }
    Ok(())
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
    request_with_timeout(request, STREAM_TIMEOUT)
}

fn request_with_timeout(request: Request, timeout: Duration) -> crate::Result<ResponseResult> {
    request_on(ensure_connected()?, request, timeout)
}

/// Same request/correlate/decode path as [`request_with_timeout`], but against
/// a center that is already serving — it never spawns one. Symmetric with
/// [`subscribe_existing`].
fn request_existing(request: Request, timeout: Duration) -> crate::Result<ResponseResult> {
    request_on(connect_existing_at(&center_paths()?)?, request, timeout)
}

fn request_on(
    mut stream: UnixStream,
    request: Request,
    timeout: Duration,
) -> crate::Result<ResponseResult> {
    let id = request.id().to_owned();
    write_line(&mut stream, &request)?;
    let reply: WireMessage =
        serde_json::from_slice(&read_line_with_deadline(&mut stream, false, timeout)?).map_err(
            |source| crate::parse::Error::JsonDeserialize {
                context: "decode center response".to_string(),
                source,
            },
        )?;
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

#[derive(Debug, Clone, PartialEq)]
pub enum StartResult {
    Started { session_id: String },
    Exited { code: Option<i32>, stderr: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ControlResult {
    Acknowledged,
    Missing,
    Ambiguous(Vec<String>),
    NotLive,
    Unverifiable,
    Refused,
}

impl ControlResult {
    /// Render a center-owned control result for display. Shared by every
    /// consumer of [`control`]/[`control_existing`] so the product wording
    /// exists once. `action` picks the verb noun so the same six outcomes
    /// read correctly for a stop or a pause request.
    pub fn message(&self, action: ControlAction, display_id: &str) -> String {
        let verb = match action {
            ControlAction::Interrupt => "stop",
            ControlAction::Pause => "pause",
        };
        match self {
            ControlResult::Acknowledged => format!("{verb} requested for {display_id}"),
            ControlResult::Missing => format!("{verb} refused: {display_id} is no longer listed"),
            ControlResult::Ambiguous(_) => format!("{verb} refused: {display_id} is ambiguous"),
            ControlResult::NotLive => {
                format!("{verb} not sent for {display_id}; center will settle it")
            }
            ControlResult::Unverifiable => {
                format!("{verb} refused: {display_id}'s live driver cannot be verified")
            }
            ControlResult::Refused => {
                format!("{verb} refused: {display_id}'s driver did not acknowledge the request")
            }
        }
    }
}

fn decode_start(result: ResponseResult) -> crate::Result<StartResult> {
    match result {
        ResponseResult::Start(StartWireResult::Started { session_id }) => {
            Ok(StartResult::Started { session_id })
        }
        ResponseResult::Start(StartWireResult::Exited { code, stderr }) => {
            Ok(StartResult::Exited { code, stderr })
        }
        _ => Err(protocol_error("unexpected start response")),
    }
}

fn trait_start_request(args: &[String], repo_path: &Utf8Path) -> Request {
    Request::Start {
        id: next_id("start"),
        target: StartTarget::Trait {
            args: args.to_vec(),
            repo_path: repo_path.to_string(),
        },
    }
}

pub fn start_trait(args: &[String], repo_path: &Utf8Path) -> crate::Result<StartResult> {
    decode_start(request_with_timeout(
        trait_start_request(args, repo_path),
        ACTION_TIMEOUT,
    )?)
}

/// Same shared spawn capability as [`start_trait`], but only against a center
/// that is already serving — it never spawns one. For a caller whose own
/// executable does not host the center sentinel (`ctx-desktop`), spawn-on-need
/// would fork an unrelated process rather than a center. Symmetric with
/// [`subscribe_existing`]. Connection failure is reported, not retried; a
/// caller wanting bounded retry cadence supplies its own.
pub fn start_trait_existing(args: &[String], repo_path: &Utf8Path) -> crate::Result<StartResult> {
    decode_start(request_existing(
        trait_start_request(args, repo_path),
        ACTION_TIMEOUT,
    )?)
}

fn session_start_request(session_id: &str, repo_key: Option<&str>) -> Request {
    Request::Start {
        id: next_id("start"),
        target: StartTarget::Session {
            session_id: session_id.to_owned(),
            repo_key: repo_key.map(str::to_owned),
        },
    }
}

pub fn start_session(session_id: &str, repo_key: Option<&str>) -> crate::Result<StartResult> {
    decode_start(request_with_timeout(
        session_start_request(session_id, repo_key),
        ACTION_TIMEOUT,
    )?)
}

/// Same shared spawn-by-session-id capability as [`start_session`], but only
/// against a center that is already serving — it never spawns one. For a
/// caller whose own executable does not host the center sentinel
/// (`ctx-desktop`), spawn-on-need would fork an unrelated process rather than
/// a center. Symmetric with [`subscribe_existing`]. Connection failure is
/// reported, not retried; a caller wanting bounded retry cadence supplies its
/// own.
pub fn start_session_existing(
    session_id: &str,
    repo_key: Option<&str>,
) -> crate::Result<StartResult> {
    decode_start(request_existing(
        session_start_request(session_id, repo_key),
        ACTION_TIMEOUT,
    )?)
}

fn control_request(session_id: &str, repo_key: Option<&str>, command: ControlAction) -> Request {
    Request::Control {
        id: next_id("control"),
        session_id: session_id.to_owned(),
        repo_key: repo_key.map(str::to_owned),
        command,
    }
}

fn decode_control(result: ResponseResult) -> crate::Result<ControlResult> {
    match result {
        ResponseResult::Control(ControlWireResult::Acknowledged) => Ok(ControlResult::Acknowledged),
        ResponseResult::Control(ControlWireResult::Missing) => Ok(ControlResult::Missing),
        ResponseResult::Control(ControlWireResult::Ambiguous(ids)) => {
            Ok(ControlResult::Ambiguous(ids))
        }
        ResponseResult::Control(ControlWireResult::NotLive) => Ok(ControlResult::NotLive),
        ResponseResult::Control(ControlWireResult::Unverifiable) => Ok(ControlResult::Unverifiable),
        ResponseResult::Control(ControlWireResult::Refused) => Ok(ControlResult::Refused),
        _ => Err(protocol_error("unexpected control response")),
    }
}

pub fn control(
    session_id: &str,
    repo_key: Option<&str>,
    command: ControlAction,
) -> crate::Result<ControlResult> {
    decode_control(request_with_timeout(
        control_request(session_id, repo_key, command),
        ACTION_TIMEOUT,
    )?)
}

/// Same shared control capability as [`control`], but only against a center
/// that is already serving — it never spawns one. For a caller whose own
/// executable does not host the center sentinel (`ctx-desktop`), spawn-on-need
/// would fork an unrelated process rather than a center. Symmetric with
/// [`subscribe_existing`].
pub fn control_existing(
    session_id: &str,
    repo_key: Option<&str>,
    command: ControlAction,
) -> crate::Result<ControlResult> {
    decode_control(request_existing(
        control_request(session_id, repo_key, command),
        ACTION_TIMEOUT,
    )?)
}

/// What a run's claimed task resolves to.
#[derive(Debug, Clone, PartialEq)]
pub enum ClaimedTaskResult {
    Missing,
    Ambiguous(Vec<String>),
    Unclaimed,
    Task(
        Box<ctx_traits_core::task::provider::ClaimedTask>,
        ClosePolicyResolution,
    ),
}

fn decode_claimed_task(result: ResponseResult) -> crate::Result<ClaimedTaskResult> {
    match result {
        ResponseResult::ClaimedTask(ClaimedTaskWireResult::Missing) => {
            Ok(ClaimedTaskResult::Missing)
        }
        ResponseResult::ClaimedTask(ClaimedTaskWireResult::Ambiguous(ids)) => {
            Ok(ClaimedTaskResult::Ambiguous(ids))
        }
        ResponseResult::ClaimedTask(ClaimedTaskWireResult::Unclaimed) => {
            Ok(ClaimedTaskResult::Unclaimed)
        }
        ResponseResult::ClaimedTask(ClaimedTaskWireResult::Task(task, policy)) => {
            Ok(ClaimedTaskResult::Task(task, policy))
        }
        _ => Err(protocol_error("unexpected claimed-task response")),
    }
}

/// The task a run claims, resolved by session id. Spawn-on-need only — see
/// [`claimed_task_existing`] for the `_existing` sibling `ctx-desktop` uses.
pub fn claimed_task(session_id: &str, repo_key: Option<&str>) -> crate::Result<ClaimedTaskResult> {
    decode_claimed_task(request_with_timeout(
        Request::ClaimedTask {
            id: next_id("claimed-task"),
            session_id: session_id.to_owned(),
            repo_key: repo_key.map(str::to_owned),
        },
        ACTION_TIMEOUT,
    )?)
}

/// Same shared claimed-task capability as [`claimed_task`], but only against
/// a center that is already serving — it never spawns one. `ctx-desktop`'s
/// use case, symmetric with [`control_existing`]/[`subscribe_existing`].
pub fn claimed_task_existing(
    session_id: &str,
    repo_key: Option<&str>,
) -> crate::Result<ClaimedTaskResult> {
    decode_claimed_task(request_existing(
        Request::ClaimedTask {
            id: next_id("claimed-task"),
            session_id: session_id.to_owned(),
            repo_key: repo_key.map(str::to_owned),
        },
        ACTION_TIMEOUT,
    )?)
}

/// Resolve a repository's compact board through an already-serving center.
/// The model owner resolves only identity; the connection worker reads files.
pub fn board_existing(repo_key: &str) -> crate::Result<BoardWireResult> {
    match request_existing(
        Request::Board {
            id: next_id("board"),
            repo_key: repo_key.to_owned(),
        },
        ACTION_TIMEOUT,
    )? {
        ResponseResult::Board(board) => Ok(*board),
        _ => Err(protocol_error("unexpected board response")),
    }
}

fn create_task_request(
    repo_key: &str,
    new_task: ctx_traits_core::task::provider::NewTask,
) -> Request {
    Request::CreateTask {
        id: next_id("create-task"),
        repo_key: repo_key.to_owned(),
        new_task,
    }
}

fn decode_create_task(result: ResponseResult) -> crate::Result<CreateTaskWireResult> {
    match result {
        ResponseResult::CreateTask(result) => Ok(result),
        _ => Err(protocol_error("unexpected create-task response")),
    }
}

/// Create one task through the repository-scoped center writer.
pub fn create_task(
    repo_key: &str,
    new_task: ctx_traits_core::task::provider::NewTask,
) -> crate::Result<CreateTaskWireResult> {
    decode_create_task(request_with_timeout(
        create_task_request(repo_key, new_task),
        ACTION_TIMEOUT,
    )?)
}

/// Same as [`create_task`], but never starts a center from this executable.
pub fn create_task_existing(
    repo_key: &str,
    new_task: ctx_traits_core::task::provider::NewTask,
) -> crate::Result<CreateTaskWireResult> {
    decode_create_task(request_existing(
        create_task_request(repo_key, new_task),
        ACTION_TIMEOUT,
    )?)
}

/// Resolve one selected task through an already-serving center. Unlike the
/// compact board answer this may reconstruct its one chosen claiming run, so
/// it uses the action timeout rather than the stream timeout.
pub fn task_detail_existing(repo_key: &str, task_key: &str) -> crate::Result<TaskDetailWireResult> {
    match request_existing(
        Request::TaskDetail {
            id: next_id("task-detail"),
            repo_key: repo_key.to_owned(),
            task_key: task_key.to_owned(),
        },
        ACTION_TIMEOUT,
    )? {
        ResponseResult::TaskDetail(detail) => Ok(*detail),
        _ => Err(protocol_error("unexpected task-detail response")),
    }
}

pub fn task_detail(repo_key: &str, task_key: &str) -> crate::Result<TaskDetailWireResult> {
    match request_with_timeout(
        Request::TaskDetail {
            id: next_id("task-detail"),
            repo_key: repo_key.to_owned(),
            task_key: task_key.to_owned(),
        },
        ACTION_TIMEOUT,
    )? {
        ResponseResult::TaskDetail(detail) => Ok(*detail),
        _ => Err(protocol_error("unexpected task-detail response")),
    }
}

/// Resolve a repository's trait library through an already-serving center.
/// Like the board endpoint, this never causes a desktop process to spawn one.
pub fn library_existing(repo_key: &str) -> crate::Result<LibraryWireResult> {
    match request_existing(
        Request::Library {
            id: next_id("library"),
            repo_key: repo_key.to_owned(),
        },
        ACTION_TIMEOUT,
    )? {
        ResponseResult::Library(library) => Ok(*library),
        _ => Err(protocol_error("unexpected library response")),
    }
}

pub fn config_existing(scope_path: &Utf8Path) -> crate::Result<ConfigWireResult> {
    match request_existing(
        Request::Config {
            id: next_id("config"),
            scope_path: scope_path.to_string(),
        },
        ACTION_TIMEOUT,
    )? {
        ResponseResult::Config(config) => Ok(*config),
        _ => Err(protocol_error("unexpected config response")),
    }
}

/// Resolve one selected trait's full preview data through the existing center.
pub fn library_detail_existing(
    repo_key: &str,
    selector: crate::library::LibraryDetailSelector,
) -> crate::Result<crate::library::LibraryDetailResolution> {
    match request_existing(
        Request::LibraryDetail {
            id: next_id("library-detail"),
            repo_key: repo_key.to_owned(),
            selector,
        },
        ACTION_TIMEOUT,
    )? {
        ResponseResult::LibraryDetail(detail) => Ok(*detail),
        _ => Err(protocol_error("unexpected library detail response")),
    }
}

/// Notify all subscribers that the repository-scoped library answer changed.
/// This only talks to an already-running center, matching the action clients.
pub fn notify_library_changed(repo_key: &str) -> crate::Result<()> {
    match request_existing(
        Request::LibraryChangedNotice {
            id: next_id("library-changed"),
            repo_key: repo_key.to_owned(),
        },
        ACTION_TIMEOUT,
    )? {
        ResponseResult::Ok => Ok(()),
        _ => Err(protocol_error("unexpected library-changed response")),
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
    BoardChanged {
        repo_key: String,
        board: Box<BoardWireResult>,
    },
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

/// Establish a persistent, repository-scoped snapshot stream against a center
/// hosted by this caller's own executable, spawning one if none is running.
/// Callers whose own executable does not host the center sentinel (a
/// standalone GUI process, for example) must use [`subscribe_existing`]
/// instead — this entry would fork a second copy of the caller.
pub fn subscribe(repo_key: Option<&str>) -> crate::Result<CenterSubscription> {
    subscribe_on(ensure_connected()?, repo_key)
}

/// Establish the same persistent, repository-scoped snapshot stream as
/// [`subscribe`], but only against a center that is already serving — it
/// never spawns one. For callers whose own executable does not host the
/// center sentinel (`__ctx-center`), spawning through their own `current_exe`
/// would fork an unrelated process rather than the center. Connection
/// failure is reported, not retried; a caller wanting bounded retry cadence
/// (e.g. while a matching center is starting up) supplies its own.
pub fn subscribe_existing(repo_key: Option<&str>) -> crate::Result<CenterSubscription> {
    subscribe_on(connect_existing_at(&center_paths()?)?, repo_key)
}

/// Connect to an already-serving center without ever spawning one. A single
/// retry on EOF absorbs the same accept-then-exit handoff race that
/// `ensure_connected_at` retries during spawn arbitration.
fn connect_existing_at(paths: &CenterPaths) -> crate::Result<UnixStream> {
    let mut retried_eof = false;
    loop {
        match UnixStream::connect(paths.socket.as_std_path()) {
            Ok(mut stream) => match handshake(&mut stream) {
                Ok(()) => return Ok(stream),
                Err(error) if !retried_eof && is_handshake_eof(&error) => {
                    retried_eof = true;
                    continue;
                }
                Err(error) => return Err(error),
            },
            Err(error) => return Err(io_error(&paths.socket, error)),
        }
    }
}

/// Subscribe over an already-handshaken stream. The reader owns socket I/O,
/// leaving consumers with a typed channel rather than a protocol stream.
/// Dropping the handle shuts down its reader socket so the server's EOF
/// watcher removes the subscriber without waiting for a later delta.
fn subscribe_on(
    mut stream: UnixStream,
    repo_key: Option<&str>,
) -> crate::Result<CenterSubscription> {
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
                WireMessage::BoardChanged { repo_key, board } => {
                    Some(CenterEvent::BoardChanged { repo_key, board })
                }
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
    // The serving owner lock is the same proof `run_server_at` requires
    // before it will bind. Route the stale-socket unlink through it too: a
    // failed handshake here is inconclusive about a live server that is
    // merely slow to answer, and unlinking its pathname while it still holds
    // the owner lock would shadow it with a socket nothing is listening on
    // (`acquire_owner_lock`'s own recovery only runs for its own process).
    // Acquire momentarily, unlink, then release immediately — the spawned
    // child acquires this same lock itself once it reaches `run_server_at`.
    match acquire_owner_lock_for_unlink(paths)? {
        OwnerLockProbe::Acquired => Ok(false),
        OwnerLockProbe::Contended => {
            // A live owner holds the serving lock even though the handshake
            // above did not succeed. Never unlink or spawn a competitor.
            Ok(true)
        }
    }
}

enum OwnerLockProbe {
    Acquired,
    Contended,
}

/// Momentarily prove exclusive ownership of the socket path, unlink a stale
/// uid-owned socket under that proof, then release — the caller does not
/// retain the lock, unlike `acquire_owner_lock`'s serving-lifetime hold.
fn acquire_owner_lock_for_unlink(paths: &CenterPaths) -> crate::Result<OwnerLockProbe> {
    let lock_path = owner_lock_path(paths);
    let lock = crate::file_lock::open_lock_file_no_follow(&lock_path)
        .map_err(|source| io_error(&lock_path, source))?;
    if !crate::file_lock::try_lock_exclusive(&lock)
        .map_err(|source| io_error(&lock_path, source))?
    {
        return Ok(OwnerLockProbe::Contended);
    }
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
    drop(lock);
    Ok(OwnerLockProbe::Acquired)
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
            ("CTX_CENTER_LIVENESS_ROOT", paths.liveness_root.as_str()),
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
    snapshot_lines: HashMap<Utf8PathBuf, Arc<[u8]>>,
    snapshot_row_serializations: u64,
    db: Connection,
    subscribers: HashMap<u64, Subscriber>,
    // An unverified row is deliberately treated as live for idle purposes:
    // cache data is derived, while an unprobeable driver lock is authoritative.
    uncertain: bool,
    pending_starts: HashMap<String, PendingStart>,
    // A bounded warming scan in progress. `None` when idle; a caller that
    // needs the whole corpus reconciled synchronously (every existing test,
    // and the per-request freshness barrier when no scan is running) keeps
    // using `discover`, which is unaffected by this field. `run_server_at`'s
    // accept loop is the only driver of `warm_step`/`drain_warm_queue`.
    warm: Option<WarmScan>,
    /// Last board answer sent for each repository. This only deduplicates
    /// notifications; board reads remain fresh worker-side operations.
    published_boards: HashMap<String, (Option<String>, crate::task_files::BoardPresence)>,
    board_fingerprints: HashMap<String, crate::task_files::BoardFingerprint>,
}

/// State for one enumerate-then-drain-then-retain scan, spread across many
/// bounded `drain_warm_queue` calls instead of one monolithic pass. Mirrors
/// `discover_inner`'s two phases (flat-store scan, then repository-local
/// reconciliation) so every invariant it already proved stays true; only the
/// pacing changes.
struct WarmScan {
    queue: VecDeque<Utf8PathBuf>,
    // `None` until the primary queue drains — the repository-local
    // reconciliation set can only be computed once `present` is complete.
    pending_unscanned: Option<VecDeque<Utf8PathBuf>>,
    present: HashSet<Utf8PathBuf>,
    unscanned_roots: HashSet<String>,
    incomplete_directory_listing: bool,
    repo_paths: Option<HashMap<String, String>>,
}

/// Ledgers reconciled per `drain_warm_queue` call from the accept loop. Tuned
/// by measurement against the 600MB/2,500-ledger corpus proof; see the
/// work summary for the measured steady-state and cold-start cost at this
/// value.
const WARM_SLICE: usize = 4;
// Wall-time bound per warm slice; keeps the owner loop's accept/credit/query
// servicing responsive regardless of individual ledger parse cost.
const WARM_SLICE_BUDGET: Duration = Duration::from_millis(300);

struct PendingStart {
    notify: mpsc::SyncSender<String>,
    since: Instant,
}

#[derive(Clone)]
struct ResolvedRow {
    session_id: String,
    ledger_path: Utf8PathBuf,
    repo_path: String,
    live: bool,
    holder: Option<crate::run_control::DriverHolder>,
    task_key: Option<String>,
    run_id: String,
    trait_id: String,
}

#[allow(clippy::large_enum_variant)] // transient, one per row resolution
enum RowResolution {
    Missing,
    One(ResolvedRow),
    Ambiguous(Vec<String>),
}

enum RepositoryResolution {
    Missing,
    One {
        root: Utf8PathBuf,
        runs: Vec<ctx_traits_core::task::provider::BoardRun>,
    },
    Ambiguous,
}

struct Subscriber {
    repo_key: Option<String>,
    outbound: mpsc::SyncSender<Outbound>,
    // The writer pulls the next row only after it has written the preceding
    // message, so a large snapshot cannot occupy one queue allocation.
    snapshot: Option<VecDeque<Arc<[u8]>>>,
    // SnapshotEnd has entered the writer queue, but the writer has not yet
    // confirmed it reached the peer. Updates must remain pending until then.
    snapshot_end_queued: bool,
    // Updates racing a streamed snapshot must follow SnapshotEnd. This remains
    // bounded so a peer that cannot consume the snapshot cannot retain model
    // state indefinitely.
    pending: VecDeque<Outbound>,
}

#[derive(Debug)]
enum Outbound {
    Delta(CenterDelta),
    BoardChanged {
        repo_key: String,
        board: Box<BoardWireResult>,
    },
    SnapshotStart(String),
    SnapshotRow(Arc<[u8]>),
    SnapshotEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapshotFrame {
    Start,
    Row,
    End,
    Update,
}

/// Drain queued post-snapshot updates without treating ordinary channel
/// backpressure as a disconnected subscriber. Returns false only on disconnect.
fn send_pending(subscriber: &mut Subscriber) -> bool {
    while let Some(message) = subscriber.pending.pop_front() {
        match subscriber.outbound.try_send(message) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(message)) => {
                subscriber.pending.push_front(message);
                return true;
            }
            Err(mpsc::TrySendError::Disconnected(_)) => return false,
        }
    }
    true
}

impl CenterModel {
    fn cache_error(paths: &CenterPaths, source: impl std::fmt::Display) -> crate::Error {
        protocol_error(format!(
            "center index {} is invalid: {source}; remove {} to rebuild",
            paths.index, paths.index
        ))
    }

    /// SQLite contention (another center starting, or a long-running
    /// maintenance transaction) is not corruption. Route every
    /// SQLite-sourced `open` failure through this classifier so a locked
    /// index is diagnosed as busy — never as invalid data needing removal —
    /// while genuine SQLite-level failures (a garbage, non-database file;
    /// disk errors) still fall through to the removable-index diagnosis.
    fn sqlite_error(paths: &CenterPaths, context: &str, source: rusqlite::Error) -> crate::Error {
        if let rusqlite::Error::SqliteFailure(inner, _) = &source
            && matches!(
                inner.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            )
        {
            return protocol_error(format!(
                "center index {} is busy: {context}: another center is starting or holding it; retry shortly",
                paths.index
            ));
        }
        Self::cache_error(paths, format!("{context}: {source}"))
    }

    fn open(paths: &CenterPaths) -> crate::Result<Self> {
        std::fs::create_dir_all(paths.runs_root.as_std_path())
            .map_err(|source| io_error(&paths.runs_root, source))?;
        let db = Connection::open(paths.index.as_std_path())
            .map_err(|source| Self::sqlite_error(paths, "open failed", source))?;
        db.busy_timeout(Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))
            .map_err(|source| Self::sqlite_error(paths, "configure busy timeout", source))?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS center_meta (version INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS center_projection_meta (version INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS center_rows (ledger TEXT PRIMARY KEY, repo_key TEXT NOT NULL, repo_path TEXT NOT NULL, mtime_secs INTEGER NOT NULL, mtime_nanos INTEGER NOT NULL, size INTEGER NOT NULL, summary TEXT NOT NULL);")
            .map_err(|source| Self::sqlite_error(paths, "initialize schema", source))?;
        let version: Option<i64> = db
            .query_row("SELECT version FROM center_meta LIMIT 1", [], |r| r.get(0))
            .optional()
            .map_err(|source| Self::sqlite_error(paths, "read schema", source))?;
        match version {
            None => {
                db.execute(
                    "INSERT INTO center_meta(version) VALUES (?1)",
                    [CENTER_SCHEMA_VERSION],
                )
                .map_err(|source| Self::sqlite_error(paths, "write schema", source))?;
            }
            Some(CENTER_SCHEMA_VERSION) => {}
            // An earlier 0243.4 build used this shared marker for the widened
            // JSON projection. Downgrade that marker in place: table layout is
            // unchanged and projection_version below retains the v3 meaning.
            Some(CENTER_PROJECTION_VERSION) => {
                db.execute(
                    "UPDATE center_meta SET version = ?1",
                    [CENTER_SCHEMA_VERSION],
                )
                .map_err(|source| {
                    Self::sqlite_error(paths, "restore legacy schema marker", source)
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
            .map_err(|source| Self::sqlite_error(paths, "read projection schema", source))?;
        if projection_version != Some(CENTER_PROJECTION_VERSION) {
            // Either a fresh v1 index (no marker at all — its cached full
            // sessions are the payload goal 1's migration half must reclaim)
            // or a preceding projection version's rows. Either way every row
            // in `center_rows` is stale-shaped: wipe it, drop the payload
            // cache table this version never writes, and reclaim the freed
            // pages so the physical file actually shrinks rather than merely
            // marking pages free. This is marker-gated, so it runs once, not
            // on every open, and after it runs every row surviving in
            // `center_rows` was written by the current projection — making
            // fingerprint match alone a sound `unchanged` signal.
            db.execute_batch(
                "DELETE FROM center_rows; DELETE FROM center_projection_meta; DROP TABLE IF EXISTS center_sessions;",
            )
            .map_err(|source| Self::sqlite_error(paths, "rebuild projection", source))?;
            // VACUUM must run outside a transaction; DELETE/DROP only return
            // pages to the free list, they do not shrink the file. The
            // projection marker is written only after VACUUM succeeds, so a
            // process that dies or fails mid-reclamation leaves no marker
            // behind and the next open retries the whole rebuild rather than
            // recording a migration that never reclaimed its pages.
            db.execute_batch("VACUUM")
                .map_err(|source| Self::sqlite_error(paths, "vacuum rebuilt index", source))?;
            db.execute(
                "INSERT INTO center_projection_meta(version) VALUES (?1)",
                [CENTER_PROJECTION_VERSION],
            )
            .map_err(|source| {
                Self::sqlite_error(paths, "write rebuilt projection schema", source)
            })?;
        }
        let mut rows = HashMap::new();
        let mut statement = db.prepare("SELECT ledger, repo_key, repo_path, mtime_secs, mtime_nanos, size, summary FROM center_rows").map_err(|source| Self::sqlite_error(paths, "prepare rows", source))?;
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
            .map_err(|source| Self::sqlite_error(paths, "read rows", source))?;
        for item in cached {
            let (ledger, repo_key, repo_path, secs, nanos, size, summary) =
                item.map_err(|source| Self::sqlite_error(paths, "read row", source))?;
            let summary = serde_json::from_str(&summary)
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
            rows.insert(
                Utf8PathBuf::from(ledger.clone()),
                CenterRow {
                    summary,
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
            snapshot_lines: HashMap::new(),
            snapshot_row_serializations: 0,
            db,
            subscribers: HashMap::new(),
            uncertain: false,
            pending_starts: HashMap::new(),
            warm: None,
            published_boards: HashMap::new(),
            board_fingerprints: HashMap::new(),
        })
    }

    fn snapshot_line(&mut self, ledger: &Utf8Path) -> crate::Result<Option<Arc<[u8]>>> {
        if let Some(line) = self.snapshot_lines.get(ledger) {
            return Ok(Some(Arc::clone(line)));
        }
        let Some(row) = self.rows.get(ledger) else {
            return Ok(None);
        };
        let mut line = serde_json::to_vec(&WireMessage::SnapshotRow {
            row: Box::new(public_row(row)),
        })
        .map_err(|source| crate::parse::Error::JsonSerialize {
            context: "serialize center snapshot row".to_string(),
            source,
        })?;
        line.push(b'\n');
        let line: Arc<[u8]> = line.into();
        self.snapshot_row_serializations += 1;
        self.snapshot_lines
            .insert(ledger.to_owned(), Arc::clone(&line));
        Ok(Some(line))
    }

    /// Advance the bounded warming scan by up to `limit` ledgers, starting a
    /// new scan if none is in progress. Every ledger interpretation still
    /// flows through `refresh_ledger_inner`; only the pacing across many
    /// calls is new. Reused by `run_server_at`'s accept loop for both the
    /// initial cold-start build and periodic rescans.
    fn warm_step(&mut self, paths: &CenterPaths, limit: usize) -> crate::Result<()> {
        // Count alone does not bound wall time: one multi-megabyte session
        // parse costs hundreds of milliseconds, so a 4-ledger slice over fat
        // ledgers can exceed the connection worker's STREAM_TIMEOUT reply
        // budget and every query during warming dies with a silent close
        // (sampled live 2026-08-30: 40% of owner-loop samples inside one
        // serde parse). Bound every slice by time as well as count.
        self.warm_step_deadline(paths, limit, Instant::now() + WARM_SLICE_BUDGET)
    }

    fn warm_step_deadline(
        &mut self,
        paths: &CenterPaths,
        limit: usize,
        deadline: Instant,
    ) -> crate::Result<()> {
        if self.warm.is_none() {
            self.begin_scan(paths)?;
        }
        self.drain_warm_queue(paths, limit, deadline)
    }

    fn is_warming(&self) -> bool {
        self.warm.is_some()
    }

    /// Enumerate cheaply (no ledger reads) and seed the scan queue. Seeding
    /// from `run_liveness::read_index` first recovers a live driver with no
    /// pending notification and no flat-store presence (a repository-local
    /// ledger) before the flat-store walk even begins — the fix for defect 5.
    fn begin_scan(&mut self, paths: &CenterPaths) -> crate::Result<()> {
        self.uncertain = false;
        let repo_paths = match crate::state::read_repo_index() {
            Ok(repos) => Some(repo_paths(repos)),
            Err(_) => {
                self.uncertain = true;
                None
            }
        };
        let mut queue = VecDeque::new();
        let mut present = HashSet::new();
        let mut unscanned_roots = HashSet::new();
        let mut incomplete_directory_listing = false;

        // No PID/timestamp authority is consulted here or anywhere
        // downstream: seeding only enqueues a ledger for the same
        // `refresh_ledger_inner` -> `run_control::probe` path every other
        // ledger takes. The kernel flock alone decides liveness.
        for row in crate::run_liveness::read_index(&paths.liveness_root).unwrap_or_default() {
            let ledger = Utf8PathBuf::from(row.ledger_path);
            if present.insert(ledger.clone()) {
                queue.push_back(ledger);
            }
        }

        let directories = match std::fs::read_dir(paths.runs_root.as_std_path()) {
            Ok(entries) => Some(entries),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(io_error(&paths.runs_root, error)),
        };
        // A missing runs root has nothing to reconcile against last-good
        // rows either; skip the repository-local reconciliation phase so
        // finalization clears every row immediately, matching a fresh store.
        let pending_unscanned = match directories {
            Some(directories) => {
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
                    let ledgers = match crate::run_session::session_store_paths(Some(root.as_str()))
                    {
                        Ok(ledgers) => ledgers,
                        Err(_) => {
                            self.uncertain = true;
                            unscanned_roots.insert(repo_key);
                            continue;
                        }
                    };
                    for ledger in ledgers {
                        if present.insert(ledger.clone()) {
                            queue.push_back(ledger);
                        }
                    }
                }
                None
            }
            None => Some(VecDeque::new()),
        };

        self.warm = Some(WarmScan {
            queue,
            pending_unscanned,
            present,
            unscanned_roots,
            incomplete_directory_listing,
            repo_paths,
        });
        Ok(())
    }

    /// Reconcile up to `budget` ledgers from the active scan's queue, then
    /// its repository-local reconciliation set once the queue drains.
    /// Finalizes (retain + `persist_all`) only once both are empty — running
    /// `retain` against a partially-warmed model would delete rows the scan
    /// has not reached yet.
    fn drain_warm_queue(
        &mut self,
        paths: &CenterPaths,
        mut budget: usize,
        deadline: Instant,
    ) -> crate::Result<()> {
        // `refresh_ledger_inner` persists each ledger through its own
        // autocommit `persist_row`/`persist_removal` call — correct in
        // isolation, but a fresh fsync-backed commit per ledger made a
        // multi-hundred-ledger slice slow enough to blow the per-request
        // freshness barrier past `STREAM_TIMEOUT`. Wrapping the whole slice
        // in one transaction reduces that to one commit per slice without
        // touching any single-ledger persistence path or its rollback
        // contract — SQLite does not distinguish a plain `BEGIN`/`COMMIT`
        // from a `Transaction` guard once one is open on the connection.
        // Committed before `finalize`'s own `persist_all` transaction opens,
        // since SQLite transactions cannot nest.
        self.db
            .execute_batch("BEGIN")
            .map_err(|source| protocol_error(source.to_string()))?;
        // Deltas are buffered rather than broadcast inline (`emit = false`
        // below) and only published once this slice's `COMMIT` below has
        // actually succeeded — a subscriber must never observe a row a later
        // commit failure in the same slice could still roll back.
        let mut pending_deltas: Vec<(Option<CenterPublicRow>, Option<CenterPublicRow>)> =
            Vec::new();
        // Full pre-mutation rows (not just the public projection used for
        // deltas), so a failed slice commit can restore exactly the model
        // state a subscriber and every other query already relied on before
        // this slice touched anything durable.
        let mut touched: Vec<(Utf8PathBuf, Option<CenterRow>, bool)> = Vec::new();
        let mut confirmed_present: Vec<Utf8PathBuf> = Vec::new();
        loop {
            // Breaking on the deadline is identical to budget exhaustion:
            // the slice commits what it processed and the next tick resumes.
            if budget == 0 || Instant::now() >= deadline {
                break;
            }
            let next = {
                let scan = self
                    .warm
                    .as_mut()
                    .expect("drain_warm_queue requires a scan");
                if let Some(ledger) = scan.queue.pop_front() {
                    Some((ledger, false))
                } else if let Some(pending) = scan.pending_unscanned.as_mut() {
                    pending.pop_front().map(|ledger| (ledger, true))
                } else {
                    None
                }
            };
            match next {
                Some((ledger, is_unscanned)) => {
                    let repo_paths = self.warm.as_ref().unwrap().repo_paths.clone();
                    let previous = self.rows.get(&ledger).cloned();
                    let before = previous.as_ref().map(public_row);
                    let refreshed = self
                        .refresh_ledger_inner(paths, &ledger, false, repo_paths.as_ref(), None)
                        .is_ok();
                    if !refreshed {
                        self.uncertain = true;
                    }
                    let after = self.rows.get(&ledger).map(public_row);
                    if before != after {
                        pending_deltas.push((before, after));
                    }
                    // Deferred until the slice's `COMMIT` actually succeeds —
                    // marking presence early would let a partially-warmed,
                    // never-persisted refresh survive the finalize retain
                    // below even after this slice's writes are rolled back.
                    if is_unscanned && refreshed && self.rows.contains_key(&ledger) {
                        confirmed_present.push(ledger.clone());
                    }
                    touched.push((ledger, previous, is_unscanned));
                    budget -= 1;
                }
                None => {
                    let scan = self
                        .warm
                        .as_mut()
                        .expect("drain_warm_queue requires a scan");
                    if scan.pending_unscanned.is_some() {
                        // Both phases are exhausted; nothing left to compute.
                        break;
                    }
                    // The primary queue just drained: every present ledger
                    // is now known, so the repository-local reconciliation
                    // set (settled-or-deleted local ledgers the flat scan
                    // never enumerates) can finally be computed.
                    let present = scan.present.clone();
                    let unscanned_roots = scan.unscanned_roots.clone();
                    let pending: VecDeque<_> = self
                        .rows
                        .iter()
                        .filter(|(ledger, row)| {
                            !present.contains(*ledger)
                                && !unscanned_roots.contains(&row.repo_key)
                                && (ledger.starts_with(&paths.runs_root)
                                    || ledger_repository_path(ledger).is_some())
                        })
                        .map(|(ledger, _)| ledger.clone())
                        .collect();
                    self.warm.as_mut().unwrap().pending_unscanned = Some(pending);
                }
            }
        }
        if let Err(source) = self.db.execute_batch("COMMIT") {
            // The transaction may still be open (a failed `COMMIT` does not
            // imply SQLite already rolled it back); force it closed so the
            // next slice opens a clean one. Best-effort: a `ROLLBACK`
            // failure here does not change what must happen to in-memory
            // state below.
            let _ = self.db.execute_batch("ROLLBACK");
            // Nothing this slice wrote is durable, so no in-memory mutation
            // it made may survive either — restore every touched row to its
            // pre-slice value and requeue the ledger so the next slice
            // retries it. Deltas were only buffered, never emitted, so
            // nothing has to be un-published.
            for (ledger, previous, is_unscanned) in touched.into_iter().rev() {
                match previous {
                    Some(row) => {
                        self.rows.insert(ledger.clone(), row);
                    }
                    None => {
                        self.rows.remove(&ledger);
                    }
                }
                let scan = self
                    .warm
                    .as_mut()
                    .expect("drain_warm_queue requires a scan");
                if is_unscanned {
                    scan.pending_unscanned
                        .get_or_insert_with(VecDeque::new)
                        .push_front(ledger);
                } else {
                    scan.queue.push_front(ledger);
                }
            }
            return Err(protocol_error(source.to_string()));
        }
        for ledger in confirmed_present {
            self.warm.as_mut().unwrap().present.insert(ledger);
        }
        for (before, after) in pending_deltas {
            self.emit_delta(before, after);
        }
        let done = {
            let scan = self
                .warm
                .as_ref()
                .expect("drain_warm_queue requires a scan");
            scan.queue.is_empty()
                && scan
                    .pending_unscanned
                    .as_ref()
                    .is_some_and(VecDeque::is_empty)
        };
        if done {
            let scan = self.warm.take().expect("scan just proven present");
            if !scan.incomplete_directory_listing {
                // Every surviving row was already persisted individually by
                // `persist_row` during the drain above. Finalizing a
                // multi-hundred-ledger scan must stay bounded too, so only
                // the ledgers retain is about to drop get an explicit
                // `persist_removal` here — never a whole-set `persist_all`
                // rewrite, which would resurrect exactly the monolithic,
                // request-blocking cost this scan exists to avoid.
                let removed: Vec<Utf8PathBuf> = self
                    .rows
                    .keys()
                    .filter(|path| {
                        !scan.present.contains(*path)
                            && !scan.unscanned_roots.contains(
                                &self.rows.get(*path).expect("key from self.rows").repo_key,
                            )
                    })
                    .cloned()
                    .collect();
                self.rows.retain(|path, row| {
                    scan.present.contains(path) || scan.unscanned_roots.contains(&row.repo_key)
                });
                // This finalize can be reached while a per-request freshness
                // slice's raw `BEGIN` is still open on this same connection,
                // so a second raw `BEGIN` here is "cannot start a transaction
                // within a transaction" — the same nesting persist_all's
                // savepoint already guards against. SAVEPOINT is legal both
                // inside and outside an open transaction.
                self.db
                    .execute_batch("SAVEPOINT finalize_removals")
                    .map_err(|source| protocol_error(source.to_string()))?;
                let result: crate::Result<()> = (|| {
                    for ledger in &removed {
                        self.persist_removal(ledger)?;
                    }
                    Ok(())
                })();
                match result {
                    Ok(()) => {
                        self.db
                            .execute_batch("RELEASE finalize_removals")
                            .map_err(|source| protocol_error(source.to_string()))?;
                    }
                    Err(error) => {
                        let _ = self.db.execute_batch(
                            "ROLLBACK TO finalize_removals; RELEASE finalize_removals",
                        );
                        return Err(error);
                    }
                }
            }
        }
        Ok(())
    }

    fn discover(&mut self, paths: &CenterPaths) -> crate::Result<()> {
        let before: HashMap<_, _> = self
            .rows
            .iter()
            .map(|(ledger, row)| (ledger.clone(), public_row(row)))
            .collect();
        if let Err(error) = self.discover_inner(paths) {
            self.snapshot_lines.clear();
            return Err(error);
        }
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
        let result = self.refresh_ledger_inner(paths, ledger, true, repo_paths.as_ref(), None);
        if result.is_err() {
            self.snapshot_lines.clear();
        }
        result
    }

    /// Registration has already authenticated the holder through its lock
    /// guard. Carry it through the refresh transaction so the first persisted
    /// and published row is authoritative rather than being corrected later.
    fn refresh_registered_ledger(
        &mut self,
        paths: &CenterPaths,
        ledger: &Utf8Path,
        holder: &crate::run_control::DriverHolder,
    ) -> crate::Result<()> {
        let repo_paths = match crate::state::read_repo_index() {
            Ok(repos) => Some(repo_paths(repos)),
            Err(_) => {
                self.uncertain = true;
                None
            }
        };
        let result =
            self.refresh_ledger_inner(paths, ledger, true, repo_paths.as_ref(), Some(holder));
        if result.is_err() {
            self.snapshot_lines.clear();
        }
        result
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
        if let Err(error) =
            self.refresh_ledger_inner(paths, ledger, false, repo_paths.as_ref(), None)
        {
            self.snapshot_lines.clear();
            return Err(error);
        }
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
        registered_holder: Option<&crate::run_control::DriverHolder>,
    ) -> crate::Result<()> {
        // Driver notifications may name a repository-local `.ctx/runs` ledger.
        // The center's flat runs root is one discovery source, not an ownership
        // boundary for an authenticated registered driver.
        if !ledger.starts_with(&paths.runs_root) && ledger_repository_path(ledger).is_none() {
            return Err(protocol_error(
                "driver ledger is outside known center storage",
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
                    if let Err(error) = self.persist_removal(ledger) {
                        self.rows.insert(ledger.to_path_buf(), previous);
                        return Err(error);
                    }
                    if emit {
                        self.emit_delta(Some(public_row(&previous)), None);
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
        // A marker-gated rebuild (see `CenterModel::open`) guarantees every
        // row surviving in `center_rows` was written by the current
        // metadata-only projection, so fingerprint match alone is a sound
        // `unchanged` signal — no legacy/incomplete row class exists to
        // distinguish here anymore.
        let unchanged = self
            .rows
            .get(ledger)
            .is_some_and(|row| row.modified == modified && row.size == size);
        let repository = ledger_repository_path(ledger)
            .map(|repo| {
                let canonical = crate::state::canonical_repo_root(&repo)?;
                Ok::<_, crate::Error>((crate::state::repo_key(&canonical), canonical.to_string()))
            })
            .transpose()?;
        let repo_key = repository
            .as_ref()
            .map(|(key, _)| key.clone())
            .unwrap_or_else(|| {
                ledger
                    .parent()
                    .and_then(Utf8Path::file_name)
                    .unwrap_or_default()
                    .to_string()
            });
        // Repository discovery is independent of ledger bytes. A scan supplies
        // one index snapshot for all ledgers, while an unavailable index retains
        // the last verified path rather than publishing a false empty value.
        let repo_path = repository
            .map(|(_, path)| path)
            .or_else(|| repo_paths.and_then(|paths| paths.get(&repo_key).cloned()))
            .or_else(|| previous.as_ref().map(|row| row.repo_path.clone()))
            .unwrap_or_default();
        // Threaded into `refresh_liveness` below so orphan repair, when it
        // fires in the same cycle a changed fingerprint was just parsed,
        // reuses this parse instead of reopening the ledger a second time —
        // preserving "parse at most once per changed fingerprint".
        let mut freshly_parsed: Option<ctx_traits_core::procedure::session::Session> = None;
        if !unchanged {
            let summary = match read_session(ledger) {
                Ok(session) => {
                    let mut summary = crate::run_summary::RunSummary::from_session(&session);
                    if summary.title.is_none() {
                        summary.title = crate::activity_sidecar::read_session_title(ledger);
                    }
                    freshly_parsed = Some(session);
                    summary
                }
                Err(error) => {
                    let session_id = ledger
                        .file_stem()
                        .map(str::to_owned)
                        .unwrap_or_else(|| ledger.to_string());
                    crate::run_summary::RunSummary::unreadable(session_id, error.to_string())
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
        if let Err(error) = self.refresh_liveness(paths, ledger, unchanged, freshly_parsed.as_ref())
        {
            self.restore_verified_row(ledger, previous);
            return Err(error);
        }
        if let Some(holder) = registered_holder
            && let Some(row) = self.rows.get_mut(ledger)
        {
            row.live = true;
            row.live_holder = Some(holder.clone());
        }
        if let Err(error) = self.persist_row(ledger) {
            self.restore_verified_row(ledger, previous);
            return Err(error);
        }
        let after = self.rows.get(ledger).map(public_row);
        if emit {
            self.emit_delta(before, after);
        }
        Ok(())
    }

    /// Publish the one delta a before/after row pair implies, or nothing if
    /// unchanged. Shared by every single-ledger refresh path and by
    /// `drain_warm_queue`, which defers this call until after the slice's
    /// transaction commits so a subscriber never observes a row that a later
    /// commit failure could still roll back.
    fn emit_delta(&mut self, before: Option<CenterPublicRow>, after: Option<CenterPublicRow>) {
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
                return self.persist_all();
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
                    .refresh_ledger_inner(paths, &ledger, false, repo_paths.as_ref(), None)
                    .is_err()
                {
                    self.uncertain = true;
                }
            }
        }
        // Registration can add a repository-local ledger which is not part of
        // the flat store enumeration above. Reconcile every known ledger that
        // was not enumerated: a local ledger can be settled and still must
        // remain queryable, while a deleted one must be removed.
        let unscanned_ledgers: Vec<_> = self
            .rows
            .iter()
            .filter(|(ledger, row)| {
                !present.contains(*ledger)
                    && !unscanned_roots.contains(&row.repo_key)
                    && (ledger.starts_with(&paths.runs_root)
                        || ledger_repository_path(ledger).is_some())
            })
            .map(|(ledger, _)| ledger.clone())
            .collect();
        for ledger in unscanned_ledgers {
            if self
                .refresh_ledger_inner(paths, &ledger, false, repo_paths.as_ref(), None)
                .is_err()
            {
                self.uncertain = true;
            } else if self.rows.contains_key(&ledger) {
                present.insert(ledger);
            }
        }
        if !incomplete_directory_listing {
            self.rows.retain(|path, row| {
                present.contains(path) || unscanned_roots.contains(&row.repo_key)
            });
        }
        self.persist_all()?;
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
        if let Some(repo_key) = repo_key.as_deref()
            && let Some(root) = self
                .rows
                .values()
                .find(|row| row.repo_key == repo_key && !row.repo_path.is_empty())
                .map(|row| Utf8PathBuf::from(row.repo_path.clone()))
            && let Ok(fingerprint) =
                crate::task_files::board_fingerprint(&crate::task_files::repo_board_dir(&root))
        {
            // The first watcher owns the scan baseline. Replacing it for a
            // later watcher could absorb a change owed to the first one.
            self.board_fingerprints
                .entry(repo_key.to_string())
                .or_insert(fingerprint);
        }
        // Baseline setup precedes the first observable snapshot frame.
        outbound
            .try_send(Outbound::SnapshotStart(request_id.clone()))
            .map_err(|_| protocol_error("subscriber outbound queue is full"))?;
        let mut rows: Vec<_> = self
            .rows
            .iter()
            .filter(|(_, row)| repo_key.as_deref().is_none_or(|repo| row.repo_key == repo))
            .collect();
        rows.sort_by(|(_, left), (_, right)| center_row_order(left, right));
        let ledgers: Vec<_> = rows.into_iter().map(|(ledger, _)| ledger.clone()).collect();
        let mut snapshot = VecDeque::with_capacity(ledgers.len());
        for ledger in ledgers {
            if let Some(line) = self.snapshot_line(&ledger)? {
                snapshot.push_back(line);
            }
        }
        self.subscribers.insert(
            id,
            Subscriber {
                repo_key: repo_key.clone(),
                outbound,
                snapshot: Some(snapshot),
                snapshot_end_queued: false,
                pending: VecDeque::new(),
            },
        );
        Ok(())
    }

    // Test-only shorthand for acknowledging the next snapshot frame.
    #[cfg(test)]
    fn advance_snapshot(&mut self, id: u64) {
        let frame = self
            .subscribers
            .get(&id)
            .map(|subscriber| {
                if subscriber.snapshot_end_queued {
                    SnapshotFrame::End
                } else if subscriber.snapshot.as_ref().is_some_and(VecDeque::is_empty) {
                    SnapshotFrame::Row
                } else {
                    SnapshotFrame::Start
                }
            })
            .unwrap_or(SnapshotFrame::Start);
        self.acknowledge_snapshot(id, frame);
    }

    fn acknowledge_snapshot(&mut self, id: u64, frame: SnapshotFrame) {
        let mut remove = false;
        if let Some(subscriber) = self.subscribers.get_mut(&id) {
            if frame == SnapshotFrame::End && subscriber.snapshot_end_queued {
                // This credit follows the completed SnapshotEnd write, so all
                // queued updates now follow its transaction on the wire.
                subscriber.snapshot_end_queued = false;
                subscriber.snapshot = None;
                remove = !send_pending(subscriber);
            } else if !matches!(frame, SnapshotFrame::End | SnapshotFrame::Update)
                && !subscriber.snapshot_end_queued
                && let Some(snapshot) = subscriber.snapshot.as_mut()
            {
                // Fill the bounded outbound channel on every credit instead
                // of releasing one row per credit: a per-row credit costs one
                // owner-loop tick per row (the loop tail sleeps between
                // iterations), which made snapshot latency O(rows × tick) —
                // 93 rows measured at ~2.3s. The channel's capacity stays the
                // backpressure bound; a full channel simply defers the rest
                // to the next write credit, and a stalled peer is still
                // evicted through the pending-delta bound and the writer's
                // deadline.
                loop {
                    let next = match snapshot.pop_front() {
                        Some(line) => Outbound::SnapshotRow(line),
                        None => Outbound::SnapshotEnd,
                    };
                    let completed = matches!(&next, Outbound::SnapshotEnd);
                    match subscriber.outbound.try_send(next) {
                        Ok(()) => {
                            if completed {
                                // Keep the snapshot active until the writer
                                // confirms that SnapshotEnd reached the peer.
                                subscriber.snapshot_end_queued = true;
                                break;
                            }
                        }
                        Err(mpsc::TrySendError::Full(unsent)) => {
                            if let Outbound::SnapshotRow(row) = unsent {
                                snapshot.push_front(row);
                            }
                            break;
                        }
                        Err(mpsc::TrySendError::Disconnected(_)) => {
                            remove = true;
                            break;
                        }
                    }
                }
            } else if subscriber.snapshot.is_none() {
                remove = !send_pending(subscriber);
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
        match &message {
            CenterDelta::Appeared { row }
            | CenterDelta::RowChanged { row }
            | CenterDelta::Ended { row } => {
                self.snapshot_lines.remove(Utf8Path::new(&row.ledger_path));
            }
            CenterDelta::ActivityLine { .. }
            | CenterDelta::LibraryChanged { .. }
            | CenterDelta::ConfigChanged { .. } => {}
        }
        let repo_keys = match &message {
            CenterDelta::Appeared { row }
            | CenterDelta::RowChanged { row }
            | CenterDelta::Ended { row }
            | CenterDelta::ActivityLine { row, .. } => vec![row.repo_key.as_str()],
            CenterDelta::LibraryChanged { repo_keys }
            | CenterDelta::ConfigChanged { repo_keys } => {
                repo_keys.iter().map(String::as_str).collect()
            }
        };
        self.subscribers.retain(|_, subscriber| {
            // A repository-scoped subscriber stays registered when an update
            // belongs to another repository; it simply does not receive it.
            // Removing it here made unrelated activity silently terminate a
            // persistent subscription.
            if !subscriber
                .repo_key
                .as_deref()
                .is_none_or(|scope| repo_keys.contains(&scope))
            {
                return true;
            }
            if subscriber.snapshot.is_some() {
                if subscriber.pending.len() == SUBSCRIBER_QUEUE {
                    return false;
                }
                subscriber
                    .pending
                    .push_back(Outbound::Delta(message.clone()));
                return true;
            }
            subscriber
                .outbound
                .try_send(Outbound::Delta(message.clone()))
                .is_ok()
        });
    }

    fn publish_board(&mut self, repo_key: String, board: Box<BoardWireResult>) {
        let identity = (
            board.resolution.digest.clone(),
            board.resolution.presence.clone(),
        );
        if self.published_boards.get(&repo_key) == Some(&identity) {
            return;
        }
        self.published_boards.insert(repo_key.clone(), identity);
        self.subscribers.retain(|_, subscriber| {
            if !subscriber
                .repo_key
                .as_deref()
                .is_none_or(|scope| scope == repo_key)
            {
                return true;
            }
            if subscriber.snapshot.is_some() {
                if subscriber.pending.len() == SUBSCRIBER_QUEUE {
                    return false;
                }
                subscriber.pending.push_back(Outbound::BoardChanged {
                    repo_key: repo_key.clone(),
                    board: board.clone(),
                });
                return true;
            }
            subscriber
                .outbound
                .try_send(Outbound::BoardChanged {
                    repo_key: repo_key.clone(),
                    board: board.clone(),
                })
                .is_ok()
        });
    }

    fn refresh_liveness(
        &mut self,
        paths: &CenterPaths,
        ledger: &Utf8Path,
        unchanged: bool,
        freshly_parsed: Option<&ctx_traits_core::procedure::session::Session>,
    ) -> crate::Result<()> {
        let probe = crate::run_control::probe(ledger)?;
        self.refresh_after_probe(paths, ledger, unchanged, probe, freshly_parsed)
    }

    fn refresh_after_probe(
        &mut self,
        paths: &CenterPaths,
        ledger: &Utf8Path,
        unchanged: bool,
        probe: crate::run_control::DriverProbe,
        freshly_parsed: Option<&ctx_traits_core::procedure::session::Session>,
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
                // Adoption used to require a retained parsed session; now it
                // reads the same worktree facts off the summary the center
                // already projects, so an unparsed-but-held ledger (no
                // summary yet, or a `parse_error` row) still publishes
                // liveness rather than being silently skipped.
                if let Some(holder) = holder.as_ref() {
                    let worktree_path = row
                        .summary
                        .worktree
                        .as_ref()
                        .and_then(|worktree| worktree.path.clone())
                        .or_else(|| row.summary.worktree_path.clone());
                    let branch = row
                        .summary
                        .worktree
                        .as_ref()
                        .map(|worktree| worktree.branch.clone())
                        .or_else(|| row.summary.worktree_branch.clone());
                    let facts = crate::run_liveness::LiveRunFacts {
                        session_id: row.summary.session_id.clone(),
                        run_id: row.summary.run_id.clone(),
                        repo_key: row.repo_key.clone(),
                        repo_path: row.repo_path.clone(),
                        ledger_path: row.ledger_path.clone(),
                        worktree_path,
                        branch,
                        log_path: None,
                    };
                    let _ = crate::run_liveness::upsert_row(
                        &paths.liveness_root,
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
                        // A settled pause is as complete a projection as a
                        // terminal outcome: `record_interrupted_outcome_in_session`
                        // only fires for a summary that is neither, so an
                        // unchanged settled-pause row needs no reparse either —
                        // only the stale holder metadata needs clearing.
                        if unchanged_under_maintenance
                            && (terminal(&row.summary) || settled_pause(&row.summary))
                        {
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
                        // can be parsed. The center no longer retains a parsed session
                        // across cycles; reuse this cycle's own parse when the
                        // fingerprint just changed, otherwise read it here — after
                        // maintenance ownership and the fingerprint re-check above
                        // have already proven the ledger unheld and reparse-safe.
                        let read_result = match freshly_parsed {
                            Some(session) => Ok(session.clone()),
                            None => read_session(ledger),
                        };
                        let Ok(mut session) = read_result else {
                            row.live = false;
                            row.live_holder = None;
                            return Ok(());
                        };
                        let summary = crate::run_summary::RunSummary::from_session(&session);
                        if !terminal(&summary) && !settled_pause(&summary) {
                            let repo_path = camino::Utf8Path::new(&row.repo_path);
                            let disk_full = crate::harness_config::resolve_config_report_at(
                                repo_path, repo_path,
                            )
                            .ok()
                            .and_then(|report| {
                                let config = report.runtime;
                                let floor_mb = config.worktree.retention.disk_floor_mb.unwrap_or(
                                    crate::harness_config::DEFAULT_WORKTREE_DISK_FLOOR_MB,
                                );
                                let worktree = summary
                                    .worktree_path
                                    .as_deref()
                                    .map(camino::Utf8Path::new)
                                    .unwrap_or(repo_path);
                                crate::environment::dispatch_disk_observation(
                                    Some(worktree),
                                    Some(repo_path),
                                    floor_mb,
                                )
                                .map(|observation| {
                                    ctx_traits_core::procedure::session::DiskFullPark {
                                        floor_mb,
                                        available_bytes: observation.available_bytes,
                                        probed_path: observation.probed_path.to_string(),
                                    }
                                })
                            });
                            if let Some(disk_full) = disk_full {
                                crate::run_session::record_disk_full_outcome_in_session(
                                    ledger,
                                    &mut session,
                                    disk_full,
                                )?;
                            } else {
                                crate::run_session::record_interrupted_outcome_in_session(
                                    ledger,
                                    &mut session,
                                )?;
                            }
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

    fn row_params(row: &CenterRow) -> crate::Result<(i64, i64, i64, String)> {
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
        Ok((
            i64::try_from(elapsed.as_secs())
                .map_err(|_| protocol_error("mtime overflows SQLite"))?,
            elapsed.subsec_nanos() as i64,
            i64::try_from(row.size).map_err(|_| protocol_error("ledger size overflows SQLite"))?,
            summary,
        ))
    }

    /// One row upsert. This is the persistence path for every per-ledger
    /// reconciliation (`refresh_ledger_inner`'s success path) — a cold build
    /// over N ledgers must not rewrite the whole growing table N times.
    fn persist_row(&mut self, ledger: &Utf8Path) -> crate::Result<()> {
        let Some(row) = self.rows.get(ledger) else {
            return Ok(());
        };
        let (secs, nanos, size, summary) = Self::row_params(row)?;
        self.db
            .execute(
                "INSERT OR REPLACE INTO center_rows VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    row.ledger_path.as_str(),
                    row.repo_key,
                    row.repo_path,
                    secs,
                    nanos,
                    size,
                    summary
                ],
            )
            .map_err(|source| protocol_error(source.to_string()))?;
        Ok(())
    }

    /// One row delete. This is the persistence path for a ledger that has
    /// disappeared — `refresh_ledger_inner`'s not-found branch.
    fn persist_removal(&mut self, ledger: &Utf8Path) -> crate::Result<()> {
        self.db
            .execute(
                "DELETE FROM center_rows WHERE ledger = ?1",
                [ledger.as_str()],
            )
            .map_err(|source| protocol_error(source.to_string()))?;
        Ok(())
    }

    /// Whole-set rewrite. Retained only for the paths that genuinely replace
    /// the whole set: a missing runs root, and discovery's post-retain
    /// sweep. Must not run once per ledger.
    fn persist_all(&mut self) -> crate::Result<()> {
        // A savepoint instead of a `Transaction` guard: scan completion can
        // reach this whole-set rewrite while a warm slice's raw `BEGIN` is
        // still open, and `transaction()` then fails with "cannot start a
        // transaction within a transaction" (observed on the production
        // center 2026-08-30 after a hand-settled ledger changed mid-warm).
        // Savepoints are valid both inside and outside an open transaction
        // and keep the same atomicity for this rewrite.
        self.db
            .execute_batch("SAVEPOINT persist_all")
            .map_err(|source| protocol_error(source.to_string()))?;
        let result: crate::Result<()> = (|| {
            self.db
                .execute("DELETE FROM center_rows", [])
                .map_err(|source| protocol_error(source.to_string()))?;
            for row in self.rows.values() {
                let (secs, nanos, size, summary) = Self::row_params(row)?;
                self.db
                    .execute(
                        "INSERT INTO center_rows VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![
                            row.ledger_path.as_str(),
                            row.repo_key,
                            row.repo_path,
                            secs,
                            nanos,
                            size,
                            summary
                        ],
                    )
                    .map_err(|source| protocol_error(source.to_string()))?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => self
                .db
                .execute_batch("RELEASE persist_all")
                .map_err(|source| protocol_error(source.to_string())),
            Err(error) => {
                let _ = self
                    .db
                    .execute_batch("ROLLBACK TO persist_all; RELEASE persist_all");
                Err(error)
            }
        }
    }

    fn has_live(&self) -> bool {
        self.uncertain
            || !self.subscribers.is_empty()
            || !self.pending_starts.is_empty()
            || self.rows.values().any(|row| row.live)
            // A cold center with a large corpus and no client attached must
            // not exit idle before finishing its first build: warming is
            // work, even though `last_work` only advances on accept/job.
            || self.warm.is_some()
    }
}

fn settled_pause(summary: &crate::run_summary::RunSummary) -> bool {
    summary
        .last_drive_outcome
        .as_deref()
        .is_some_and(|outcome| {
            ctx_traits_core::procedure::session::DriveOutcomeKind::from_wire(outcome)
                .is_settled_pause()
        })
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

/// The path of the flock a serving center holds for its entire lifetime,
/// distinct from `spawn_lock` (launch arbitration only, released before
/// serving begins). Deriving it from the socket path means isolated private
/// tuples automatically get isolated owner locks.
fn owner_lock_path(paths: &CenterPaths) -> Utf8PathBuf {
    paths.socket.with_extension("owner")
}

/// Acquire the serving-lifetime owner lock, or prove a live owner already
/// answers `paths.socket` and this process should exit as a no-op. Returns
/// the held lock file on success; the caller must keep it alive for the
/// whole serving lifetime. `Ok(None)` means: do not bind, exit 0.
fn acquire_owner_lock(paths: &CenterPaths) -> crate::Result<Option<std::fs::File>> {
    let lock_path = owner_lock_path(paths);
    let lock = crate::file_lock::open_lock_file_no_follow(&lock_path)
        .map_err(|source| io_error(&lock_path, source))?;
    if crate::file_lock::try_lock_exclusive(&lock).map_err(|source| io_error(&lock_path, source))? {
        // The pathname is unowned: a stale socket left by a prior owner may
        // now safely be unlinked, using the same uid + is_socket guard
        // `prepare_spawn` uses for the identical situation.
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
        return Ok(Some(lock));
    }
    // Contended: a live owner may already be serving this socket. Never
    // unlink or bind while ownership is unproven either way.
    match UnixStream::connect(paths.socket.as_std_path()) {
        Ok(mut stream) => match handshake_with_timeout(&mut stream, STREAM_TIMEOUT) {
            Ok(()) => Ok(None),
            Err(_) => Ok(None),
        },
        Err(_) => Ok(None),
    }
}

fn run_server_at(paths: CenterPaths, idle: Duration, scan_interval: Duration) -> crate::Result<()> {
    if let Ok(marker) = std::env::var("CTX_CENTER_LAUNCH_MARKER") {
        // Private test instrumentation: each sentinel process records exactly
        // one launch before it can publish a listener. The pid and socket
        // let a fixture teardown identify and wait out exactly the launches
        // its own tuple caused, even when this file is a registry shared
        // (via forwarding) across several concurrently running fixtures.
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(marker)
            .and_then(|mut file| {
                file.write_all(
                    format!("center {} {}\n", std::process::id(), paths.socket).as_bytes(),
                )
            })
            .map_err(|source| io_error(&paths.socket, source))?;
    }
    let Some(_owner_lock) = acquire_owner_lock(&paths)? else {
        // Another process already owns this socket path, or may be in the
        // process of proving it. Exit as an idempotent no-op start.
        return Ok(());
    };
    // Opening the index reads only `center_rows`, metadata-only and small
    // after the projection rebuild — not the cold-start cost. Binding ahead
    // of `open` would instead mean a client can handshake a center that then
    // dies on an invalid index, worse than today's failure. The corpus scan
    // itself is the slow part, and it is deliberately left for the accept
    // loop below: a client must be able to handshake and receive a partial
    // answer while it is still running.
    let mut model = CenterModel::open(&paths)?;
    let listener = UnixListener::bind(paths.socket.as_std_path())
        .map_err(|source| io_error(&paths.socket, source))?;
    let _guard = SocketGuard::for_listener(paths.socket.clone())?;
    let _ = std::fs::remove_file(spawn_marker_path(&paths).as_std_path());
    listener
        .set_nonblocking(true)
        .map_err(|source| io_error(&paths.socket, source))?;
    // The corpus build starts only once the listener is bound and can
    // already service a handshake. `begin_scan` is cheap directory
    // enumeration only (no ledger reads); every actual ledger read happens
    // in bounded slices from the accept loop below, never here.
    model.begin_scan(&paths)?;
    let mut last_work = Instant::now();
    let mut last_scan = Instant::now();
    let mut last_board_scan = Instant::now();
    // Connection workers own every potentially slow socket operation. Jobs are
    // executed here, on the sole owner of both the model and SQLite handle.
    let (jobs, job_receiver) = mpsc::sync_channel::<ModelCommand>(MODEL_QUEUE);
    // Board reads happen in connection workers. This memo retains only the
    // timestamp of an unchanged served answer; it is not a board-data cache.
    let board_instants = Arc::new(Mutex::new(HashMap::new()));
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let jobs = jobs.clone();
                let worker_paths = paths.clone();
                let board_instants = board_instants.clone();
                std::thread::spawn(move || {
                    serve_connection_worker_with_board_instants(
                        stream,
                        jobs,
                        worker_paths,
                        board_instants,
                    )
                });
                last_work = Instant::now();
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(io_error(&paths.socket, error)),
        }
        // Advance any in-progress warming scan (cold-start build or a
        // periodic rescan below) by one bounded slice per loop tick, so the
        // pathname stays serviced throughout instead of only at start/end.
        if model.is_warming() {
            if model.warm_step(&paths, WARM_SLICE).is_err() {
                model.uncertain = true;
            }
            if !model.is_warming() {
                // The scan just finished this tick. Measuring the interval
                // from completion (not from when it started) means a scan
                // that runs longer than `scan_interval` does not immediately
                // rearm on its own next tick — which would otherwise keep
                // `is_warming()`, and so `has_live()`, true indefinitely and
                // starve idle exit.
                last_scan = Instant::now();
            }
        }
        while let Ok(job) = job_receiver.try_recv() {
            match job {
                ModelCommand::Request { request, reply } => {
                    // Queries take a center-owned freshness barrier before
                    // reading the model, including unnotified store changes.
                    // While a scan is already warming the model, draining one
                    // more bounded slice keeps the barrier itself bounded —
                    // serving the rows indexed so far is what bind-before-
                    // build means; a full corpus-sized discover here would
                    // block the very first query on the whole corpus.
                    let barrier_result = if snapshot_request(&request) {
                        if model.is_warming() {
                            model.warm_step(&paths, WARM_SLICE)
                        } else {
                            model.discover(&paths)
                        }
                    } else {
                        Ok(())
                    };
                    if let Err(error) = barrier_result {
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
                ModelCommand::SnapshotNext { id, frame } => model.acknowledge_snapshot(id, frame),
                ModelCommand::WatchStart {
                    token,
                    notify,
                    reply,
                } => {
                    model.pending_starts.insert(
                        token,
                        PendingStart {
                            notify,
                            since: Instant::now(),
                        },
                    );
                    let _ = reply.send(Ok(()));
                }
                ModelCommand::ForgetStart { token } => {
                    model.pending_starts.remove(&token);
                }
                ModelCommand::ResolveRow {
                    session_id,
                    repo_key,
                    reply,
                } => {
                    let resolution = match select_row(&model, &session_id, repo_key.as_deref()) {
                        RowSelection::Missing => RowResolution::Missing,
                        RowSelection::One(row) => RowResolution::One(ResolvedRow {
                            session_id: row.summary.session_id.clone(),
                            ledger_path: row.ledger_path.clone(),
                            repo_path: row.repo_path.clone(),
                            live: row.live,
                            holder: row.live_holder.clone(),
                            task_key: row.summary.task_key.clone(),
                            run_id: row.summary.run_id.clone(),
                            trait_id: row.summary.trait_id.clone(),
                        }),
                        RowSelection::Ambiguous(rows) => RowResolution::Ambiguous(
                            rows.into_iter()
                                .map(|row| row.summary.session_id.clone())
                                .collect(),
                        ),
                    };
                    let _ = reply.send(Ok(resolution));
                }
                ModelCommand::ResolveRepository { repo_key, reply } => {
                    let mut roots: Vec<_> = model
                        .rows
                        .values()
                        .filter(|row| row.repo_key == repo_key && !row.repo_path.is_empty())
                        .map(|row| Utf8PathBuf::from(row.repo_path.clone()))
                        .collect();
                    roots.sort();
                    roots.dedup();
                    let resolution = match roots.len() {
                        0 => match crate::state::read_repo_index() {
                            Ok(index) => match index
                                .into_iter()
                                .filter(|entry| entry.key == repo_key)
                                .collect::<Vec<_>>()
                                .as_slice()
                            {
                                [entry] => RepositoryResolution::One {
                                    root: Utf8PathBuf::from(entry.path.clone()),
                                    runs: Vec::new(),
                                },
                                _ => RepositoryResolution::Missing,
                            },
                            Err(error) => {
                                let _ = reply.send(Err(error));
                                continue;
                            }
                        },
                        1 => RepositoryResolution::One {
                            root: roots.remove(0),
                            runs: model
                                .rows
                                .values()
                                .filter(|row| row.repo_key == repo_key)
                                .filter_map(|row| {
                                    let task_key = row.summary.task_key.clone()?;
                                    Some(ctx_traits_core::task::provider::BoardRun::from_run(
                                        row.summary.run_id.clone(),
                                        Some(row.repo_key.clone()),
                                        task_key,
                                        row.live,
                                        &row.summary.status,
                                        row.summary.landing.as_deref(),
                                    ))
                                })
                                .collect(),
                        },
                        _ => RepositoryResolution::Ambiguous,
                    };
                    let _ = reply.send(Ok(resolution));
                }
                ModelCommand::LibraryChanged { repo_key, reply } => {
                    model.broadcast(CenterDelta::LibraryChanged {
                        repo_keys: vec![repo_key],
                    });
                    let _ = reply.send(Ok(()));
                }
                ModelCommand::ResolveRunId {
                    run_id,
                    repo_key,
                    reply,
                } => {
                    let row = model
                        .rows
                        .values()
                        .find(|row| row.repo_key == repo_key && row.summary.run_id == run_id)
                        .map(|row| ResolvedRow {
                            session_id: row.summary.session_id.clone(),
                            ledger_path: row.ledger_path.clone(),
                            repo_path: row.repo_path.clone(),
                            live: row.live,
                            holder: row.live_holder.clone(),
                            task_key: row.summary.task_key.clone(),
                            run_id: row.summary.run_id.clone(),
                            trait_id: row.summary.trait_id.clone(),
                        });
                    let _ = reply.send(Ok(row));
                }
                ModelCommand::PublishBoard { repo_key, board } => {
                    model.publish_board(repo_key, board);
                }
            }
            last_work = Instant::now();
        }
        // A periodic rescan starts a new bounded scan rather than blocking
        // the loop for one corpus-sized pass; already-warming scans (e.g.
        // the cold-start build) are left to keep draining above. `last_scan`
        // only advances once a started scan has actually finished (above,
        // or immediately below for a scan small enough to complete in one
        // slice), never merely because one was started.
        if !model.is_warming() && last_scan.elapsed() >= scan_interval {
            if model.warm_step(&paths, WARM_SLICE).is_err() {
                model.uncertain = true;
            }
            if !model.is_warming() {
                last_scan = Instant::now();
            }
        }
        if last_board_scan.elapsed() >= scan_interval {
            scan_subscribed_boards(&mut model, &jobs, &board_instants);
            last_board_scan = Instant::now();
        }
        if last_work.elapsed() >= idle {
            prune_pending_starts(&mut model);
            // No fresh scan is started here: the periodic rescan above and
            // the cold-start scan already validate the corpus in bounded
            // slices on their own schedule, and `has_live` below already
            // treats an in-progress scan as live, so idle exit cannot fire
            // mid-warm. Starting yet another scan from this branch would
            // make `has_live` observe its own just-started scan and never
            // see a genuinely idle model — the bounded-validation contract
            // is "idle exit waits for warming to settle", not "idle exit
            // triggers one more validation pass first".
            if !model.has_live() {
                return Ok(());
            }
            last_work = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn scan_subscribed_boards(
    model: &mut CenterModel,
    jobs: &mpsc::SyncSender<ModelCommand>,
    board_instants: &Arc<Mutex<BoardInstants>>,
) {
    let repo_keys: HashSet<String> = model
        .subscribers
        .values()
        .filter_map(|subscriber| subscriber.repo_key.clone())
        .collect();
    for repo_key in repo_keys {
        let mut roots: Vec<_> = model
            .rows
            .values()
            .filter(|row| row.repo_key == repo_key && !row.repo_path.is_empty())
            .map(|row| Utf8PathBuf::from(row.repo_path.clone()))
            .collect();
        roots.sort();
        roots.dedup();
        let [root] = roots.as_slice() else { continue };
        let fingerprint =
            match crate::task_files::board_fingerprint(&crate::task_files::repo_board_dir(root)) {
                Ok(fingerprint) => fingerprint,
                Err(_) => continue,
            };
        let changed = model
            .board_fingerprints
            .get(&repo_key)
            .is_some_and(|previous| previous != &fingerprint);
        model
            .board_fingerprints
            .insert(repo_key.clone(), fingerprint);
        if !changed {
            continue;
        }
        let runs = model
            .rows
            .values()
            .filter(|row| row.repo_key == repo_key)
            .filter_map(|row| {
                row.summary.task_key.clone().map(|task_key| {
                    ctx_traits_core::task::provider::BoardRun::from_run(
                        row.summary.run_id.clone(),
                        Some(row.repo_key.clone()),
                        task_key,
                        row.live,
                        &row.summary.status,
                        row.summary.landing.as_deref(),
                    )
                })
            })
            .collect();
        let jobs = jobs.clone();
        let instants = board_instants.clone();
        let root = root.clone();
        std::thread::spawn(move || {
            if let Ok(board) = assemble_board(repo_key.clone(), root, runs, &instants) {
                let _ = jobs.try_send(ModelCommand::PublishBoard {
                    repo_key,
                    board: Box::new(board),
                });
            }
        });
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
        frame: SnapshotFrame,
    },
    WatchStart {
        token: String,
        notify: mpsc::SyncSender<String>,
        reply: mpsc::SyncSender<crate::Result<()>>,
    },
    ForgetStart {
        token: String,
    },
    ResolveRow {
        session_id: String,
        repo_key: Option<String>,
        reply: mpsc::SyncSender<crate::Result<RowResolution>>,
    },
    ResolveRepository {
        repo_key: String,
        reply: mpsc::SyncSender<crate::Result<RepositoryResolution>>,
    },
    ResolveRunId {
        run_id: String,
        repo_key: String,
        reply: mpsc::SyncSender<crate::Result<Option<ResolvedRow>>>,
    },
    PublishBoard {
        repo_key: String,
        board: Box<BoardWireResult>,
    },
    LibraryChanged {
        repo_key: String,
        reply: mpsc::SyncSender<crate::Result<()>>,
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
#[cfg(test)]
fn serve_connection_worker(
    stream: UnixStream,
    jobs: mpsc::SyncSender<ModelCommand>,
    paths: CenterPaths,
) {
    serve_connection_worker_with_board_instants(
        stream,
        jobs,
        paths,
        Arc::new(Mutex::new(HashMap::new())),
    );
}

fn serve_connection_worker_with_board_instants(
    mut stream: UnixStream,
    jobs: mpsc::SyncSender<ModelCommand>,
    paths: CenterPaths,
    board_instants: Arc<Mutex<BoardInstants>>,
) {
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
                    let frame = match &message {
                        Outbound::SnapshotStart(_) => Some(SnapshotFrame::Start),
                        Outbound::SnapshotRow(_) => Some(SnapshotFrame::Row),
                        Outbound::SnapshotEnd => Some(SnapshotFrame::End),
                        Outbound::Delta(_) | Outbound::BoardChanged { .. } => {
                            Some(SnapshotFrame::Update)
                        }
                    };
                    let write = match message {
                        Outbound::Delta(delta) => {
                            write_line(&mut writer_stream, &WireMessage::Delta { delta })
                        }
                        Outbound::BoardChanged { repo_key, board } => write_line(
                            &mut writer_stream,
                            &WireMessage::BoardChanged { repo_key, board },
                        ),
                        Outbound::SnapshotStart(id) => {
                            write_line(&mut writer_stream, &WireMessage::SnapshotStart { id })
                        }
                        Outbound::SnapshotRow(line) => write_raw_line(&mut writer_stream, &line),
                        Outbound::SnapshotEnd => {
                            write_line(&mut writer_stream, &WireMessage::SnapshotEnd)
                        }
                    };
                    if write.is_err() {
                        break;
                    }
                    // Only the worker waits for model capacity. The owner
                    // never waits for this socket and receives the precise
                    // frame identity needed to retain deltas through End.
                    if let Some(frame) = frame
                        && snapshot_credit
                            .send(ModelCommand::SnapshotNext { id, frame })
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
        if let Request::Start { target, .. } = request {
            // A detached child is intentionally independent of its requester,
            // but an abandoned requester must not leave its correlation watch
            // installed until the child eventually registers or times out.
            let (disconnected, requester) = mpsc::sync_channel(1);
            let mut watcher = match stream.try_clone() {
                Ok(stream) => stream,
                Err(_) => return,
            };
            let _ = watcher.set_read_timeout(None);
            std::thread::spawn(move || {
                let mut byte = [0_u8; 1];
                let _ = watcher.read(&mut byte);
                let _ = disconnected.try_send(());
            });
            let result = run_start(&paths, &jobs, target, &requester)
                .map(ResponseResult::Start)
                .unwrap_or_else(|error| ResponseResult::Error {
                    message: error.to_string(),
                });
            let _ = response(&mut stream, id, result);
            return;
        }
        if let Request::Control {
            session_id,
            repo_key,
            command,
            ..
        } = request
        {
            let result = run_control_request(&jobs, session_id, repo_key, command)
                .map(ResponseResult::Control)
                .unwrap_or_else(|error| ResponseResult::Error {
                    message: error.to_string(),
                });
            let _ = response(&mut stream, id, result);
            return;
        }
        if let Request::ClaimedTask {
            session_id,
            repo_key,
            ..
        } = request
        {
            let result = run_claimed_task_request(&jobs, session_id, repo_key)
                .map(ResponseResult::ClaimedTask)
                .unwrap_or_else(|error| ResponseResult::Error {
                    message: error.to_string(),
                });
            let _ = response(&mut stream, id, result);
            return;
        }
        if let Request::Board { repo_key, .. } = request {
            let result = run_board_request(&jobs, repo_key, &board_instants)
                .map(|board| ResponseResult::Board(Box::new(board)))
                .unwrap_or_else(|error| ResponseResult::Error {
                    message: error.to_string(),
                });
            let _ = response(&mut stream, id, result);
            return;
        }
        if let Request::CreateTask {
            repo_key, new_task, ..
        } = request
        {
            let result =
                run_create_task_request(&jobs, &paths, repo_key, new_task, &board_instants)
                    .map(ResponseResult::CreateTask)
                    .unwrap_or_else(|error| ResponseResult::Error {
                        message: error.to_string(),
                    });
            let _ = response(&mut stream, id, result);
            return;
        }
        if let Request::TaskDetail {
            repo_key, task_key, ..
        } = request
        {
            let result = run_task_detail_request(&jobs, repo_key, task_key)
                .map(|detail| ResponseResult::TaskDetail(Box::new(detail)))
                .unwrap_or_else(|error| ResponseResult::Error {
                    message: error.to_string(),
                });
            let _ = response(&mut stream, id, result);
            return;
        }
        if let Request::Library { repo_key, .. } = request {
            let result = run_library_request(&jobs, repo_key)
                .map(|library| ResponseResult::Library(Box::new(library)))
                .unwrap_or_else(|error| ResponseResult::Error {
                    message: error.to_string(),
                });
            let _ = response(&mut stream, id, result);
            return;
        }
        if let Request::Config { scope_path, .. } = request {
            let result = run_config_request(&scope_path)
                .map(|config| ResponseResult::Config(Box::new(config)))
                .unwrap_or_else(|error| ResponseResult::Error {
                    message: error.to_string(),
                });
            let _ = response(&mut stream, id, result);
            return;
        }
        if let Request::LibraryDetail {
            repo_key, selector, ..
        } = request
        {
            let result = run_library_detail_request(&jobs, repo_key, selector)
                .map(|detail| ResponseResult::LibraryDetail(Box::new(detail)))
                .unwrap_or_else(|error| ResponseResult::Error {
                    message: error.to_string(),
                });
            let _ = response(&mut stream, id, result);
            return;
        }
        if let Request::LibraryChangedNotice { repo_key, .. } = request {
            let (reply_sender, reply_receiver) = mpsc::sync_channel(1);
            if jobs
                .try_send(ModelCommand::LibraryChanged {
                    repo_key,
                    reply: reply_sender,
                })
                .is_err()
            {
                return;
            }
            let result = match reply_receiver.recv_timeout(STREAM_TIMEOUT) {
                Ok(Ok(())) => ResponseResult::Ok,
                Ok(Err(error)) => ResponseResult::Error {
                    message: error.to_string(),
                },
                Err(_) => return,
            };
            let _ = response(&mut stream, id, result);
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

struct StartWatch {
    jobs: mpsc::SyncSender<ModelCommand>,
    token: String,
}

impl Drop for StartWatch {
    fn drop(&mut self) {
        let jobs = self.jobs.clone();
        let token = self.token.clone();
        std::thread::spawn(move || {
            forget_start_with_retry(jobs, token);
        });
    }
}

fn forget_start_with_retry(jobs: mpsc::SyncSender<ModelCommand>, token: String) {
    // Retry transient owner backpressure, but leave timed-out entries to the
    // pending-start pruner rather than permanently retaining a cleanup thread.
    let deadline = Instant::now() + STREAM_TIMEOUT;
    while Instant::now() < deadline {
        match jobs.try_send(ModelCommand::ForgetStart {
            token: token.clone(),
        }) {
            Ok(()) | Err(mpsc::TrySendError::Disconnected(_)) => return,
            Err(mpsc::TrySendError::Full(_)) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn resolve_row(
    jobs: &mpsc::SyncSender<ModelCommand>,
    session_id: String,
    repo_key: Option<String>,
) -> crate::Result<RowResolution> {
    let (reply, receiver) = mpsc::sync_channel(1);
    jobs.try_send(ModelCommand::ResolveRow {
        session_id,
        repo_key,
        reply,
    })
    .map_err(|_| protocol_error("model queue unavailable"))?;
    receiver
        .recv_timeout(STREAM_TIMEOUT)
        .map_err(|_| protocol_error("model resolution timed out"))?
}

fn run_control_request(
    jobs: &mpsc::SyncSender<ModelCommand>,
    session_id: String,
    repo_key: Option<String>,
    action: ControlAction,
) -> crate::Result<ControlWireResult> {
    match resolve_row(jobs, session_id, repo_key)? {
        RowResolution::Missing => Ok(ControlWireResult::Missing),
        RowResolution::Ambiguous(ids) => Ok(ControlWireResult::Ambiguous(ids)),
        RowResolution::One(row) if !row.live => Ok(ControlWireResult::NotLive),
        RowResolution::One(row) => {
            let Some(holder) = row.holder else {
                return Ok(ControlWireResult::Unverifiable);
            };
            if holder.session_id != row.session_id {
                return Ok(ControlWireResult::Unverifiable);
            }
            let accepted = match action {
                ControlAction::Interrupt => {
                    crate::run_control::request_interrupt(&row.ledger_path, &holder)?
                }
                ControlAction::Pause => {
                    crate::run_control::request_pause(&row.ledger_path, &holder)?
                }
            };
            Ok(if accepted {
                ControlWireResult::Acknowledged
            } else {
                ControlWireResult::Refused
            })
        }
    }
}

fn run_claimed_task_request(
    jobs: &mpsc::SyncSender<ModelCommand>,
    session_id: String,
    repo_key: Option<String>,
) -> crate::Result<ClaimedTaskWireResult> {
    match resolve_row(jobs, session_id, repo_key)? {
        RowResolution::Missing => Ok(ClaimedTaskWireResult::Missing),
        RowResolution::Ambiguous(ids) => Ok(ClaimedTaskWireResult::Ambiguous(ids)),
        RowResolution::One(row) => {
            let Some(key) = row.task_key.filter(|key| !key.is_empty()) else {
                return Ok(ClaimedTaskWireResult::Unclaimed);
            };
            let repo_root = if row.repo_path.is_empty() {
                ledger_repository_path(&row.ledger_path).ok_or_else(|| {
                    protocol_error(format!(
                        "claimed task {key:?} has no usable repository path for ledger {}",
                        row.ledger_path
                    ))
                })?
            } else {
                Utf8PathBuf::from(row.repo_path)
            };
            if repo_root.as_str().is_empty() || !repo_root.is_absolute() {
                return Err(protocol_error(format!(
                    "claimed task {key:?} requires an absolute repository path, got {repo_root:?}"
                )));
            }
            let dir = crate::task_files::repo_board_dir(&repo_root);
            let board = crate::task_files::FilesTaskBoard::open_read(dir.clone());
            let resolved = board
                .get(&key)
                .map_err(|error| {
                    protocol_error(format!(
                        "claimed task {key:?} could not be read from board {dir}: {error}"
                    ))
                })?
                .ok_or_else(|| {
                    protocol_error(format!("claimed task {key:?} not found in board {dir}"))
                })?;
            // A config-resolution failure degrades only the close-policy
            // outcome to `Unresolved` — it must never fail the whole
            // claimed-task answer, which would break the task row itself.
            let policy = match crate::harness_config::effective_auto_close_policy(
                &repo_root,
                resolved.document.auto_close,
            ) {
                Ok(Some(policy)) => ClosePolicyResolution::Effective(policy),
                Ok(None) => ClosePolicyResolution::NoneConfigured,
                Err(error) => ClosePolicyResolution::Unresolved(error.to_string()),
            };
            Ok(ClaimedTaskWireResult::Task(
                Box::new(ctx_traits_core::task::provider::ClaimedTask::from_document(
                    &resolved.document,
                )),
                policy,
            ))
        }
    }
}

fn run_board_request(
    jobs: &mpsc::SyncSender<ModelCommand>,
    repo_key: String,
    board_instants: &Mutex<BoardInstants>,
) -> crate::Result<BoardWireResult> {
    let (reply, receiver) = mpsc::sync_channel(1);
    jobs.try_send(ModelCommand::ResolveRepository {
        repo_key: repo_key.clone(),
        reply,
    })
    .map_err(|_| protocol_error("model queue unavailable"))?;
    let root = match receiver
        .recv_timeout(STREAM_TIMEOUT)
        .map_err(|_| protocol_error("repository resolution timed out"))??
    {
        RepositoryResolution::One { root, runs } => (root, runs),
        RepositoryResolution::Missing => {
            return Err(protocol_error("repository is not known to center"));
        }
        RepositoryResolution::Ambiguous => {
            return Err(protocol_error("repository has ambiguous roots"));
        }
    };
    let (root, runs) = root;
    assemble_board(repo_key, root, runs, board_instants)
}

fn assemble_board(
    repo_key: String,
    root: Utf8PathBuf,
    runs: Vec<ctx_traits_core::task::provider::BoardRun>,
    board_instants: &Mutex<BoardInstants>,
) -> crate::Result<BoardWireResult> {
    let mut resolution =
        crate::task_files::FilesTaskBoard::open_read(crate::task_files::repo_board_dir(&root))
            .resolve_board()
            .map_err(|error| protocol_error(format!("board for {root}: {error}")))?;
    let signature = (
        resolution.digest.clone(),
        resolution.presence.clone(),
        resolution.sync_report.clone(),
    );
    if let Ok(mut instants) = board_instants.lock() {
        if let Some((digest, presence, report, resolved_at)) = instants.get(&repo_key)
            && (digest.clone(), presence.clone(), report.clone()) == signature
        {
            resolution.resolved_at = *resolved_at;
        } else {
            // The wire's legacy instant is second-granular. Keep it strictly
            // advancing even when two distinct answers resolve in one second.
            if let Some((_, _, _, prior)) = instants.get(&repo_key) {
                resolution.resolved_at = resolution.resolved_at.max(prior.saturating_add(1));
            }
            instants.insert(
                repo_key.clone(),
                (
                    signature.0.clone(),
                    signature.1.clone(),
                    signature.2.clone(),
                    resolution.resolved_at,
                ),
            );
        }
    }
    let joined_runs = resolution
        .rows
        .iter()
        .map(|row| {
            let joined = ctx_traits_core::task::provider::joined_runs(
                &repo_key,
                &row.summary.key,
                runs.iter(),
            )
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
            (row.summary.key.clone(), joined)
        })
        .collect::<BTreeMap<_, _>>();
    let sections = resolution
        .rows
        .iter()
        .map(|row| {
            let joined = joined_runs.get(&row.summary.key).into_iter().flatten();
            (
                row.summary.key.clone(),
                ctx_traits_core::task::provider::section_of(
                    row.summary.derived_status,
                    row.summary.archived,
                    joined,
                ),
            )
        })
        .collect();
    Ok(BoardWireResult {
        resolution,
        joined_runs,
        sections,
    })
}

fn resolve_repository(
    jobs: &mpsc::SyncSender<ModelCommand>,
    repo_key: String,
) -> crate::Result<RepositoryResolution> {
    let (reply, receiver) = mpsc::sync_channel(1);
    jobs.try_send(ModelCommand::ResolveRepository { repo_key, reply })
        .map_err(|_| protocol_error("model queue unavailable"))?;
    receiver
        .recv_timeout(STREAM_TIMEOUT)
        .map_err(|_| protocol_error("repository resolution timed out"))?
}

fn create_lock_path(paths: &CenterPaths, repo_key: &str) -> Utf8PathBuf {
    let safe: String = repo_key
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect();
    paths.index.with_file_name(format!("create-{}.lock", safe))
}

fn wait_for_create_gate() -> crate::Result<()> {
    let Ok(gate) = std::env::var("CTX_CENTER_CREATE_GATE") else {
        return Ok(());
    };
    let gate = Utf8PathBuf::from(gate);
    let waiting = Utf8PathBuf::from(format!("{}.waiting", gate));
    std::fs::write(waiting.as_std_path(), b"waiting")
        .map_err(|source| io_error(&waiting, source))?;
    let deadline = Instant::now() + STREAM_TIMEOUT;
    while !gate.exists() {
        if Instant::now() >= deadline {
            return Err(protocol_error("create gate timed out"));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

fn run_create_task_request(
    jobs: &mpsc::SyncSender<ModelCommand>,
    paths: &CenterPaths,
    repo_key: String,
    new_task: ctx_traits_core::task::provider::NewTask,
    board_instants: &Mutex<BoardInstants>,
) -> crate::Result<CreateTaskWireResult> {
    use ctx_traits_core::task::provider::{TaskProviderMut, WriteError};

    let (root, runs) = match resolve_repository(jobs, repo_key.clone())? {
        RepositoryResolution::One { root, runs } => (root, runs),
        RepositoryResolution::Missing => {
            return Err(protocol_error("repository is not known to center"));
        }
        RepositoryResolution::Ambiguous => {
            return Err(protocol_error("repository has ambiguous roots"));
        }
    };
    let board_dir = crate::task_files::repo_board_dir(&root);
    let read_board = crate::task_files::FilesTaskBoard::open_read(board_dir.clone());
    match read_board.resolve_board() {
        Ok(resolution) => match resolution.presence {
            crate::task_files::BoardPresence::Absent => {
                return Ok(CreateTaskWireResult::BoardAbsent);
            }
            crate::task_files::BoardPresence::Unreadable { reason } => {
                return Ok(CreateTaskWireResult::BoardUnreadable { reason });
            }
            crate::task_files::BoardPresence::Empty | crate::task_files::BoardPresence::Loaded => {}
        },
        Err(error) => {
            return Ok(CreateTaskWireResult::BoardUnreadable {
                reason: error.to_string(),
            });
        }
    }
    let lock_path = create_lock_path(paths, &repo_key);
    let lock = crate::file_lock::open_lock_file_no_follow(&lock_path)
        .map_err(|source| io_error(&lock_path, source))?;
    if !crate::file_lock::try_lock_exclusive(&lock)
        .map_err(|source| io_error(&lock_path, source))?
    {
        return Ok(CreateTaskWireResult::Occupied);
    }
    // The board may have been removed after the optimistic presence check;
    // recheck under the writer lock so `create_dir_all` can never recreate it.
    match read_board.resolve_board() {
        Ok(resolution) => match resolution.presence {
            crate::task_files::BoardPresence::Absent => {
                return Ok(CreateTaskWireResult::BoardAbsent);
            }
            crate::task_files::BoardPresence::Unreadable { reason } => {
                return Ok(CreateTaskWireResult::BoardUnreadable { reason });
            }
            crate::task_files::BoardPresence::Empty | crate::task_files::BoardPresence::Loaded => {}
        },
        Err(error) => {
            return Ok(CreateTaskWireResult::BoardUnreadable {
                reason: error.to_string(),
            });
        }
    }
    wait_for_create_gate()?;
    let provider = crate::task_files::FilesTaskBoard::open_read_write(board_dir);
    let created = match provider.create(new_task) {
        Ok(summary) => summary,
        Err(WriteError::InvalidField { field, reason }) => {
            return Ok(CreateTaskWireResult::InvalidField {
                field: field.to_string(),
                reason,
            });
        }
        Err(WriteError::NotFound(parent)) => {
            return Ok(CreateTaskWireResult::UnknownParent { parent });
        }
        Err(WriteError::AmbiguousKey(parent)) => {
            return Ok(CreateTaskWireResult::AmbiguousParent { parent });
        }
        Err(error) => return Err(protocol_error(format!("create task: {error}"))),
    };
    drop(lock);
    let board = assemble_board(repo_key.clone(), root, runs, board_instants)?;
    jobs.try_send(ModelCommand::PublishBoard {
        repo_key,
        board: Box::new(board),
    })
    .map_err(|_| protocol_error("model queue unavailable"))?;
    Ok(CreateTaskWireResult::Created(created))
}

fn resolve_run_id(
    jobs: &mpsc::SyncSender<ModelCommand>,
    repo_key: String,
    run_id: String,
) -> crate::Result<Option<ResolvedRow>> {
    let (reply, receiver) = mpsc::sync_channel(1);
    jobs.try_send(ModelCommand::ResolveRunId {
        run_id,
        repo_key,
        reply,
    })
    .map_err(|_| protocol_error("model queue unavailable"))?;
    receiver
        .recv_timeout(STREAM_TIMEOUT)
        .map_err(|_| protocol_error("run resolution timed out"))?
}

fn run_task_detail_request(
    jobs: &mpsc::SyncSender<ModelCommand>,
    repo_key: String,
    task_key: String,
) -> crate::Result<TaskDetailWireResult> {
    use ctx_traits_core::task::provider::{ClaimResolution, TaskProvider};

    let (reply, receiver) = mpsc::sync_channel(1);
    jobs.try_send(ModelCommand::ResolveRepository {
        repo_key: repo_key.clone(),
        reply,
    })
    .map_err(|_| protocol_error("model queue unavailable"))?;
    let (root, runs) = match receiver
        .recv_timeout(STREAM_TIMEOUT)
        .map_err(|_| protocol_error("repository resolution timed out"))??
    {
        RepositoryResolution::One { root, runs } => (root, runs),
        RepositoryResolution::Missing => {
            return Err(protocol_error("repository is not known to center"));
        }
        RepositoryResolution::Ambiguous => {
            return Err(protocol_error("repository has ambiguous roots"));
        }
    };
    let board =
        crate::task_files::FilesTaskBoard::open_read(crate::task_files::repo_board_dir(&root));
    let resolution = board
        .resolve_board()
        .map_err(|error| protocol_error(format!("board for {root}: {error}")))?;
    let Some(board_row) = resolution
        .rows
        .iter()
        .find(|row| row.summary.key == task_key)
    else {
        return Ok(TaskDetailWireResult::Missing);
    };
    let task = board
        .get(&task_key)
        .map_err(|error| protocol_error(format!("task {task_key:?} could not be read: {error}")))?
        .ok_or_else(|| {
            protocol_error(format!("task {task_key:?} disappeared during resolution"))
        })?;
    let joined = ctx_traits_core::task::provider::joined_runs(&repo_key, &task_key, runs.iter());
    let state = ctx_traits_core::task::provider::task_state(
        board_row.summary.derived_status,
        joined.iter().copied(),
    );
    let claim = match ctx_traits_core::task::provider::chosen_claim(joined.iter().copied()) {
        ClaimResolution::NoClaim => TaskClaimWire::NoClaim,
        ClaimResolution::Ambiguous(ids) => TaskClaimWire::Ambiguous(ids),
        ClaimResolution::One(run) => {
            let row =
                resolve_run_id(jobs, repo_key.clone(), run.run_id.clone())?.ok_or_else(|| {
                    protocol_error(format!("claiming run {:?} is unavailable", run.run_id))
                })?;
            let session = read_session(&row.ledger_path).map_err(|error| {
                protocol_error(format!(
                    "claiming run {:?} is unreadable: {error}",
                    run.run_id
                ))
            })?;
            let progress = ctx_traits_io_progress(&session);
            TaskClaimWire::Claim {
                run_id: row.run_id,
                trait_id: row.trait_id,
                progress,
                state: if run.live {
                    TaskClaimState::Active
                } else if run.awaiting_owner {
                    TaskClaimState::Pending
                } else {
                    TaskClaimState::Terminal
                },
            }
        }
    };
    // Like the claimed-task request, a bad config is evidence about policy,
    // not a reason to discard the otherwise readable task detail.
    let close_policy =
        match crate::harness_config::effective_auto_close_policy(&root, task.document.auto_close) {
            Ok(Some(policy)) => ClosePolicyResolution::Effective(policy),
            Ok(None) => ClosePolicyResolution::NoneConfigured,
            Err(error) => ClosePolicyResolution::Unresolved(error.to_string()),
        };
    Ok(TaskDetailWireResult::Resolved {
        summary: board_row.summary.clone(),
        content: ctx_traits_core::task::provider::content_lede(&task.document).to_owned(),
        scope: task.document.scope.clone(),
        validation: task.document.validation.clone(),
        open_steps: task
            .document
            .steps
            .iter()
            .filter(|step| !step.done)
            .cloned()
            .collect(),
        state: ctx_traits_core::task::provider::state_word(state).to_owned(),
        current_activity: ctx_traits_core::task::provider::state_is_current_activity(state),
        raised: task.document.raised.clone(),
        parent: task.document.relations.parent.clone(),
        depends_on: task.document.relations.depends_on.clone(),
        checks: task.document.checks.clone(),
        closure: task.document.closure.clone(),
        close_policy,
        claim,
    })
}

fn ctx_traits_io_progress(
    session: &ctx_traits_core::procedure::session::Session,
) -> Result<ctx_traits_core::procedure::run::RunProgress, String> {
    let loaded = crate::run::load_trait_for_session(None, None, session, "task-detail")
        .map_err(|error| error.to_string())?;
    ctx_traits_core::procedure::run::plan_procedure_run(&loaded.trait_ref, session.run_id.clone())
        .map(|plan| ctx_traits_core::procedure::run::run_progress_for(&plan, session))
        .map_err(|error| error.to_string())
}

fn run_library_request(
    jobs: &mpsc::SyncSender<ModelCommand>,
    repo_key: String,
) -> crate::Result<LibraryWireResult> {
    let (reply, receiver) = mpsc::sync_channel(1);
    jobs.try_send(ModelCommand::ResolveRepository {
        repo_key: repo_key.clone(),
        reply,
    })
    .map_err(|_| protocol_error("model queue unavailable"))?;
    let root = match receiver
        .recv_timeout(STREAM_TIMEOUT)
        .map_err(|_| protocol_error("repository resolution timed out"))??
    {
        RepositoryResolution::One { root, .. } => root,
        RepositoryResolution::Missing => {
            return Err(protocol_error("repository is not known to center"));
        }
        RepositoryResolution::Ambiguous => {
            return Err(protocol_error("repository has ambiguous roots"));
        }
    };
    let context = crate::inventory::InventoryContext::at_repo_root(&root)?;
    let document = crate::trust::read_store()?;
    let resolution = crate::library::resolve_library(&context, &document)?;
    Ok(LibraryWireResult {
        repo_key,
        repo_path: root.to_string(),
        resolution,
    })
}

/// Resolves the repository-scoped config answer used by both the center and
/// read-only local consumers.
pub fn run_config_request(scope_path: &str) -> crate::Result<ConfigWireResult> {
    let path = Utf8Path::new(scope_path);
    if scope_path.is_empty() || !path.is_absolute() {
        return Ok(ConfigWireResult {
            repo_key: String::new(),
            repo_path: String::new(),
            resolution: crate::config_view::ConfigResolution::Refused {
                reason: "config scope must be a non-empty absolute path".to_string(),
            },
        });
    }
    if !path.exists() {
        return Ok(ConfigWireResult {
            repo_key: String::new(),
            repo_path: String::new(),
            resolution: crate::config_view::ConfigResolution::Refused {
                reason: "config scope does not exist".to_string(),
            },
        });
    }
    let Some(root) = crate::repository::discover_repo_root_at(path)? else {
        return Ok(ConfigWireResult {
            repo_key: String::new(),
            repo_path: String::new(),
            resolution: crate::config_view::ConfigResolution::Refused {
                reason: "config scope is not inside a Git worktree".to_string(),
            },
        });
    };
    let root = crate::state::canonical_repo_root(&root)?;
    let repo_key = crate::state::repo_key(&root);
    let resolution = (|| {
        let report = crate::harness_config::resolve_config_report_at(&root, &root)?;
        let document = crate::trust::read_store()?;
        let instant_epoch_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        crate::config_view::resolve_config_view(
            &root,
            report,
            &document,
            crate::config_view::ConfigCenterIdentity {
                version: env!("CARGO_PKG_VERSION").to_string(),
                socket: center_paths()?.socket.to_string(),
            },
            instant_epoch_millis,
        )
    })();
    let resolution = match resolution {
        Ok(view) => crate::config_view::ConfigResolution::Resolved(view),
        Err(error) => crate::config_view::ConfigResolution::Failed {
            reason: error.to_string(),
        },
    };
    let result = ConfigWireResult {
        repo_key,
        repo_path: root.to_string(),
        resolution,
    };
    if serde_json::to_vec(&result)
        .map_err(|source| crate::parse::Error::JsonSerialize {
            context: "serialize config".to_string(),
            source,
        })?
        .len()
        + 512
        > MAX_LINE_BYTES
    {
        return Ok(ConfigWireResult {
            repo_key: result.repo_key,
            repo_path: result.repo_path,
            resolution: crate::config_view::ConfigResolution::Refused {
                reason: "config answer exceeds protocol size limit".to_string(),
            },
        });
    }
    Ok(result)
}

fn run_library_detail_request(
    jobs: &mpsc::SyncSender<ModelCommand>,
    repo_key: String,
    selector: crate::library::LibraryDetailSelector,
) -> crate::Result<crate::library::LibraryDetailResolution> {
    let (reply, receiver) = mpsc::sync_channel(1);
    jobs.try_send(ModelCommand::ResolveRepository { repo_key, reply })
        .map_err(|_| protocol_error("model queue unavailable"))?;
    let root = match receiver
        .recv_timeout(STREAM_TIMEOUT)
        .map_err(|_| protocol_error("repository resolution timed out"))??
    {
        RepositoryResolution::One { root, .. } => root,
        RepositoryResolution::Missing => {
            return Err(protocol_error("repository is not known to center"));
        }
        RepositoryResolution::Ambiguous => {
            return Err(protocol_error("repository has ambiguous roots"));
        }
    };
    let context = crate::inventory::InventoryContext::at_repo_root(&root)?;
    let document = crate::trust::read_store()?;
    let detail = crate::library::resolve_library_detail(&context, &document, &selector)?;
    // Refuse before the line writer sees an oversized payload; truncating a
    // served lede would make the desktop fabricate a successful answer.
    if serde_json::to_vec(&detail)
        .map_err(|source| crate::parse::Error::JsonSerialize {
            context: "serialize library detail".to_string(),
            source,
        })?
        .len()
        + 512
        > MAX_LINE_BYTES
    {
        return Ok(crate::library::LibraryDetailResolution::Refused {
            reason: "library detail exceeds protocol size limit".to_string(),
        });
    }
    Ok(detail)
}

fn resume_argv(session_id: &str) -> Vec<String> {
    vec![
        "traits".to_string(),
        "internal".to_string(),
        "drive".to_string(),
        "--session".to_string(),
        session_id.to_string(),
        "--progress".to_string(),
        "none".to_string(),
    ]
}

fn start_log_paths(paths: &CenterPaths, token: &str) -> crate::Result<(Utf8PathBuf, Utf8PathBuf)> {
    let root = paths.runs_root.join("start-logs");
    std::fs::create_dir_all(root.as_std_path()).map_err(|source| io_error(&root, source))?;
    Ok((
        root.join(format!("{token}.stdout.log")),
        root.join(format!("{token}.stderr.log")),
    ))
}

fn truncate_stderr(path: &Utf8Path) -> String {
    let Ok(file) = std::fs::File::open(path.as_std_path()) else {
        return String::new();
    };
    let mut bytes = Vec::with_capacity(START_STDERR_BYTES);
    let mut reader = file.take(START_STDERR_BYTES as u64);
    if reader.read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    // Do not emit a partial UTF-8 scalar in the JSON response. A malformed
    // stderr stream is still bounded at its first malformed sequence.
    match std::str::from_utf8(&bytes) {
        Ok(text) => text.to_string(),
        Err(error) => std::str::from_utf8(&bytes[..error.valid_up_to()])
            .expect("a UTF-8 error's valid prefix is valid UTF-8")
            .to_string(),
    }
}

fn run_start(
    paths: &CenterPaths,
    jobs: &mpsc::SyncSender<ModelCommand>,
    target: StartTarget,
    requester: &mpsc::Receiver<()>,
) -> crate::Result<StartWireResult> {
    let token = format!("{}-{}", std::process::id(), next_id("start"));
    let (args, cwd) = match target {
        StartTarget::Trait { args, repo_path } => {
            if repo_path.is_empty() {
                return Err(protocol_error("start requires a repository path"));
            }
            let mut argv = vec!["traits".to_string(), "run".to_string()];
            argv.extend(args);
            argv.extend(["--progress".to_string(), "none".to_string()]);
            (argv, Utf8PathBuf::from(repo_path))
        }
        StartTarget::Session {
            session_id,
            repo_key,
        } => match resolve_row(jobs, session_id, repo_key)? {
            RowResolution::One(row) => {
                let repo_path = if row.repo_path.is_empty() {
                    ledger_repository_path(&row.ledger_path)
                        .ok_or_else(|| protocol_error("session has no repository path"))?
                } else {
                    Utf8PathBuf::from(row.repo_path)
                };
                (
                    // A bare ID resolves through the driver's default store. A
                    // center row can originate in any repository-local store,
                    // so preserve the resolved ledger identity explicitly.
                    resume_argv(row.ledger_path.as_str()),
                    repo_path,
                )
            }
            RowResolution::Missing => return Err(protocol_error("session is missing")),
            RowResolution::Ambiguous(_) => return Err(protocol_error("session is ambiguous")),
        },
    };
    // A relative path would make Command resolve against the center's own cwd,
    // not the viewer-selected repository. Both start targets share this guard.
    if cwd.as_str().is_empty() || !cwd.is_absolute() {
        return Err(protocol_error("start requires an absolute repository path"));
    }
    let (notify, registered) = mpsc::sync_channel(1);
    let (reply, ready) = mpsc::sync_channel(1);
    jobs.try_send(ModelCommand::WatchStart {
        token: token.clone(),
        notify,
        reply,
    })
    .map_err(|_| protocol_error("model queue unavailable"))?;
    let _watch = StartWatch {
        jobs: jobs.clone(),
        token: token.clone(),
    };
    ready
        .recv_timeout(STREAM_TIMEOUT)
        .map_err(|_| protocol_error("start watch timed out"))??;
    let (stdout, stderr) = start_log_paths(paths, &token)?;
    let executable = center_executable()?;
    let stdout_text = stdout.to_string();
    let token_text = token.clone();
    let mut child = crate::process::spawn_detached_split(
        Utf8Path::from_path(executable.as_path())
            .ok_or_else(|| protocol_error("center executable is not UTF-8"))?,
        &args,
        &cwd,
        &stdout,
        &stderr,
        &[
            (crate::run_liveness::SPAWNED_LOG_PATH_ENV, &stdout_text),
            (SPAWN_TOKEN_ENV, &token_text),
        ],
    )?;
    let deadline = Instant::now() + ACTION_TIMEOUT;
    loop {
        if requester.try_recv().is_ok() {
            return Err(protocol_error("start requester disconnected"));
        }
        if let Ok(session_id) = registered.try_recv() {
            return Ok(StartWireResult::Started { session_id });
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|source| io_error(&stderr, source))?
        {
            if let Ok(session_id) = registered.try_recv() {
                return Ok(StartWireResult::Started { session_id });
            }
            return Ok(StartWireResult::Exited {
                code: status.code(),
                stderr: truncate_stderr(&stderr),
            });
        }
        if Instant::now() >= deadline {
            return Err(protocol_error("start timed out waiting for registration"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn ledger_repository_path(ledger: &Utf8Path) -> Option<Utf8PathBuf> {
    // A relative ledger path's derived repository root is still relative,
    // and canonicalizing it would resolve against the center process's own
    // cwd rather than the ledger's true location. Reject before touching
    // the filesystem so a relative path can never masquerade as absolute.
    if !ledger.is_absolute() {
        return None;
    }
    ledger
        .parent()
        .filter(|runs| runs.file_name() == Some("runs"))
        .and_then(Utf8Path::parent)
        .filter(|ctx| ctx.file_name() == Some(".ctx"))
        .and_then(Utf8Path::parent)
        .map(|repo| {
            std::fs::canonicalize(repo.as_std_path())
                .ok()
                .and_then(|path| Utf8PathBuf::from_path_buf(path).ok())
                .unwrap_or_else(|| repo.to_path_buf())
        })
}

fn prune_pending_starts(model: &mut CenterModel) {
    model
        .pending_starts
        .retain(|_, pending| pending.since.elapsed() < ACTION_TIMEOUT);
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
            model.refresh_registered_ledger(paths, Utf8Path::new(&ledger), &registration.holder)?;
            if let Some(token) = registration.spawn_token
                && let Some(pending) = model.pending_starts.remove(&token)
                && let Some(row) = model.rows.get(Utf8Path::new(&ledger))
            {
                let _ = pending.notify.try_send(row.summary.session_id.clone());
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
            // `select_row` only resolves which ledger answers this request; the
            // center no longer retains a parsed session, so `Get` reads the
            // ledger on demand and a read failure is an error naming the
            // ledger, never a row-resolution `Missing`.
            RowSelection::One(row) => {
                let session = read_session(&row.ledger_path)?;
                Ok(ResponseResult::Get(GetWireResult::Session(Box::new(
                    session,
                ))))
            }
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
                .filter(|row| row.summary.parse_error.is_none())
                .map(|row| run_record(&row.summary))
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
            let facts: Vec<_> = rows
                .into_iter()
                .map(|row| crate::dispatch_preflight::WallFacts {
                    status: row.summary.status.clone(),
                    task_value: row.summary.task_value.clone(),
                    terminal_epoch: row.summary.terminal_epoch,
                    run_id: row.summary.run_id.clone(),
                    park_wall_id: row.summary.park_wall_id.clone(),
                })
                .collect();
            Ok(ResponseResult::StandingWall(
                crate::dispatch_preflight::standing_wall_in_facts(
                    &facts,
                    &wall_id,
                    &dispatched_task,
                ),
            ))
        }
        Request::Subscribe { .. }
        | Request::Start { .. }
        | Request::Control { .. }
        | Request::ClaimedTask { .. }
        | Request::Board { .. }
        | Request::CreateTask { .. }
        | Request::TaskDetail { .. }
        | Request::Library { .. }
        | Request::LibraryDetail { .. }
        | Request::LibraryChangedNotice { .. }
        | Request::Config { .. } => Err(protocol_error(
            "request is handled by the connection worker",
        )),
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

/// Project a [`RunSummary`] into `stats::RunRecord` at the center boundary,
/// so `Stats` never needs a retained `Session`. Only `canonical_digest` needs
/// a lossy-free round trip — every value stored here was produced by
/// [`Digest::parse`] on the way in, so re-parsing it back is infallible in
/// practice; a value that fails to parse is treated as absent rather than as
/// a query failure.
fn run_record(
    summary: &crate::run_summary::RunSummary,
) -> ctx_traits_core::procedure::stats::RunRecord {
    ctx_traits_core::procedure::stats::RunRecord {
        trait_id: summary.trait_id.clone(),
        canonical_digest: summary
            .canonical_digest
            .as_deref()
            .and_then(|digest| ctx_traits_core::digest::Digest::parse(digest).ok()),
        status: summary.status.clone(),
        stop_reason: summary.stop_reason.clone(),
        drive_outcome_kind: summary
            .last_drive_outcome
            .clone()
            .map(ctx_traits_core::procedure::session::DriveOutcomeKind::from_wire),
        outcome: summary.last_drive_outcome.clone(),
        recorded_at_epoch: summary.terminal_epoch,
        work_tokens: summary.work_tokens,
        narrator_tokens: summary.narrator_tokens,
        guide_tokens: summary.guide_tokens,
        tokens_by_model: summary.tokens_by_model.clone(),
        verdict_rounds: summary.verdict_rounds,
    }
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
            // Every unit test gets its own scratch root, so its liveness
            // index must live under that same root too — never the real
            // machine-global `run_control::runtime_root()`, which is shared
            // process-wide state that would otherwise leak unrelated
            // drivers' entries (including stale ones left by an earlier,
            // unrelated test run) into this test's model.
            liveness_root: root.join("liveness"),
        }
    }

    #[test]
    fn pending_start_blocks_idle_exit_until_action_timeout_prunes_it() {
        let root = scratch("pending-start-prune");
        let paths = paths(root.clone());
        let mut model = CenterModel::open(&paths).expect("open center");
        let (notify, _receiver) = mpsc::sync_channel(1);
        model.pending_starts.insert(
            "recent".to_string(),
            PendingStart {
                notify: notify.clone(),
                since: Instant::now()
                    .checked_sub(ACTION_TIMEOUT - Duration::from_secs(1))
                    .expect("monotonic clock predates ACTION_TIMEOUT"),
            },
        );
        model.pending_starts.insert(
            "expired".to_string(),
            PendingStart {
                notify,
                since: Instant::now()
                    .checked_sub(ACTION_TIMEOUT)
                    .expect("monotonic clock predates ACTION_TIMEOUT"),
            },
        );

        assert!(model.has_live(), "a pending start keeps the center alive");
        prune_pending_starts(&mut model);
        assert!(model.pending_starts.contains_key("recent"));
        assert!(!model.pending_starts.contains_key("expired"));
        assert!(model.has_live(), "the recent pending start remains live");
        model.pending_starts.clear();
        assert!(!model.has_live(), "no pending work permits idle exit");
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn start_and_control_requests_round_trip_and_skip_the_snapshot_barrier() {
        let start = Request::Start {
            id: "start".to_string(),
            target: StartTarget::Trait {
                args: vec!["fixture".to_string()],
                repo_path: "/repository".to_string(),
            },
        };
        let control = Request::Control {
            id: "control".to_string(),
            session_id: "session".to_string(),
            repo_key: Some("repository".to_string()),
            command: ControlAction::Pause,
        };
        let claimed_task = Request::ClaimedTask {
            id: "claimed-task".to_string(),
            session_id: "session".to_string(),
            repo_key: Some("repository".to_string()),
        };
        for request in [start, control, claimed_task] {
            let encoded = serde_json::to_string(&request).expect("encode request");
            let decoded: Request = serde_json::from_str(&encoded).expect("decode request");
            assert_eq!(decoded.id(), request.id());
            assert!(!snapshot_request(&decoded));
        }

        let root = scratch("claimed-task-connection-worker-only");
        let paths = paths(root.clone());
        let mut model = CenterModel::open(&paths).expect("open center");
        let error = handle_request(
            &mut model,
            &paths,
            Request::ClaimedTask {
                id: "claimed-task".to_string(),
                session_id: "session".to_string(),
                repo_key: None,
            },
        )
        .expect_err("claimed-task reaching the model owner is a protocol error");
        assert!(error.to_string().contains("connection worker"));
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn claimed_task_wire_result_round_trips() {
        let document = ctx_traits_core::task::TaskDocument {
            schema_version: ctx_traits_core::task::SCHEMA_VERSION.to_string(),
            key: "fixture".to_string(),
            title: "Fixture task".to_string(),
            status: Some(ctx_traits_core::task::TaskStatus::Ready),
            raised: None,
            closed: None,
            wall: None,
            origin: None,
            content: "the description".to_string(),
            scope: String::new(),
            validation: String::new(),
            relations: Default::default(),
            steps: Vec::new(),
            checks: Vec::new(),
            auto_close: Some(ctx_traits_core::task::AutoClosePolicy::Checked),
            closure: None,
        };
        let claimed = ctx_traits_core::task::provider::ClaimedTask::from_document(&document);
        for result in [
            ClaimedTaskWireResult::Missing,
            ClaimedTaskWireResult::Ambiguous(vec!["a".to_string(), "b".to_string()]),
            ClaimedTaskWireResult::Unclaimed,
            ClaimedTaskWireResult::Task(
                Box::new(claimed.clone()),
                ClosePolicyResolution::Effective(ctx_traits_core::task::AutoClosePolicy::Checked),
            ),
            ClaimedTaskWireResult::Task(
                Box::new(claimed.clone()),
                ClosePolicyResolution::NoneConfigured,
            ),
            ClaimedTaskWireResult::Task(
                Box::new(claimed),
                ClosePolicyResolution::Unresolved("malformed runtime.toml".to_string()),
            ),
        ] {
            let response = ResponseResult::ClaimedTask(result.clone());
            let encoded = serde_json::to_string(&response).expect("encode response");
            let decoded: ResponseResult = serde_json::from_str(&encoded).expect("decode response");
            match (response, decoded) {
                (ResponseResult::ClaimedTask(a), ResponseResult::ClaimedTask(b)) => {
                    assert_eq!(
                        serde_json::to_string(&a).unwrap(),
                        serde_json::to_string(&b).unwrap()
                    );
                }
                _ => panic!("expected ClaimedTask round trip"),
            }
        }
    }

    #[test]
    fn ledger_repository_path_rejects_relative_ledgers_even_with_expected_shape() {
        // A relative ledger with the exact `.ctx/runs/<file>` shape must not
        // resolve — canonicalizing it would silently pick up the center
        // process's own cwd as the repository root.
        let relative = Utf8PathBuf::from(".ctx/runs/session.json");
        assert_eq!(ledger_repository_path(&relative), None);

        // An absolute ledger with the same shape still resolves.
        let root = std::env::temp_dir();
        let root = Utf8PathBuf::from_path_buf(root).expect("temp dir is UTF-8");
        let root = root.join(format!("ledger-repo-path-test-{}", next_id("ledger-test")));
        let ctx_runs = root.join(".ctx").join("runs");
        std::fs::create_dir_all(ctx_runs.as_std_path()).expect("create fixture dirs");
        let absolute = ctx_runs.join("session.json");
        assert!(ledger_repository_path(&absolute).is_some());
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn resume_argv_is_exact_and_never_carries_worktree() {
        assert_eq!(
            resume_argv("session"),
            [
                "traits",
                "internal",
                "drive",
                "--session",
                "session",
                "--progress",
                "none"
            ]
        );
        assert!(!resume_argv("session").iter().any(|arg| arg == "--worktree"));
    }

    #[test]
    fn start_watch_retries_cleanup_after_temporary_queue_backpressure() {
        let (jobs, receiver) = mpsc::sync_channel(1);
        jobs.send(ModelCommand::SnapshotNext {
            id: 1,
            frame: SnapshotFrame::Start,
        })
        .expect("fill model queue");
        let watch = StartWatch {
            jobs: jobs.clone(),
            token: "token".to_string(),
        };
        let (done, completed) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            drop(watch);
            done.send(()).expect("report cleanup completion");
        });

        assert!(matches!(
            receiver.recv(),
            Ok(ModelCommand::SnapshotNext { id: 1, .. })
        ));
        assert!(
            matches!(receiver.recv_timeout(STREAM_TIMEOUT), Ok(ModelCommand::ForgetStart { token }) if token == "token")
        );
        completed
            .recv_timeout(STREAM_TIMEOUT)
            .expect("cleanup completes after capacity returns");
    }

    #[test]
    fn start_watch_cleanup_stops_after_sustained_queue_backpressure() {
        let (jobs, receiver) = mpsc::sync_channel(1);
        jobs.send(ModelCommand::SnapshotNext {
            id: 1,
            frame: SnapshotFrame::Start,
        })
        .expect("fill model queue");
        let (done, completed) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            forget_start_with_retry(jobs, "token".to_string());
            done.send(()).expect("report bounded cleanup completion");
        });

        completed
            .recv_timeout(STREAM_TIMEOUT + Duration::from_millis(100))
            .expect("cleanup must stop while model capacity remains unavailable");
        assert!(matches!(
            receiver.recv(),
            Ok(ModelCommand::SnapshotNext { id: 1, .. })
        ));
    }

    #[test]
    fn truncated_stderr_is_bounded_and_never_contains_a_partial_utf8_scalar() {
        let root = scratch("bounded-start-stderr");
        let stderr = root.join("stderr.log");
        let mut contents = vec![b'x'; START_STDERR_BYTES - 1];
        contents.extend_from_slice("€ trailing output".as_bytes());
        std::fs::write(stderr.as_std_path(), contents).expect("write stderr");

        let result = truncate_stderr(&stderr);
        assert_eq!(result.len(), START_STDERR_BYTES - 1);
        assert!(result.is_ascii());
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn start_rejects_a_relative_repository_path() {
        let root = scratch("relative-start-path");
        let paths = paths(root.clone());
        let (jobs, _receiver) = mpsc::sync_channel(1);
        let (_requester, disconnected) = mpsc::sync_channel(1);
        let error = run_start(
            &paths,
            &jobs,
            StartTarget::Trait {
                args: vec!["fixture".to_string()],
                repo_path: "relative/repository".to_string(),
            },
            &disconnected,
        )
        .expect_err("relative cwd must not inherit the center cwd");
        assert!(
            error
                .to_string()
                .contains("start requires an absolute repository path")
        );
        let _ = std::fs::remove_dir_all(root.as_std_path());
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

    fn fixture_session_with_outcome(
        status: &str,
        outcome: &str,
    ) -> ctx_traits_core::procedure::session::Session {
        let mut value = serde_json::to_value(fixture_session(status)).expect("serialize fixture");
        value["last-drive-outcome"] = serde_json::json!({
            "outcome": outcome,
            "recorded-at-epoch": 1,
        });
        serde_json::from_value(value).expect("fixture outcome deserializes")
    }

    fn parked_ask_session_with_outcome(
        outcome: Option<&str>,
    ) -> ctx_traits_core::procedure::session::Session {
        use ctx_traits_core::procedure::runtime::{SequenceFrame, SequenceFrameKind};

        let mut session = match outcome {
            Some(outcome) => fixture_session_with_outcome("waiting-on-human", outcome),
            None => fixture_session("waiting-on-human"),
        };
        session.next_frame = Some(Box::new(SequenceFrame {
            kind: SequenceFrameKind::Ask,
            run_id: "run-fixture".to_string(),
            trait_id: "fixture-trait".to_string(),
            sequence_index: Some(0),
            run_index: Some(0),
            item_id: Some("ask-owner".to_string()),
            position_path: Vec::new(),
            loop_context: None,
            for_each_context: None,
            guard_explanations: Vec::new(),
            signal_payloads: Vec::new(),
            signal_emission_ceiling: 0,
            title: "ask-owner".to_string(),
            frame_text: "What should I do next?".to_string(),
            prompt: None,
            command: None,
            available_inputs: Vec::new(),
            resource_evidence: Vec::new(),
            requested_outputs: Vec::new(),
            assigned_agent: None,
            allowed_signals: Vec::new(),
            derived_signals: Vec::new(),
            call_template: None,
            warnings: Vec::new(),
        }));
        session
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
            spawn_token: None,
            holder: crate::run_control::DriverHolder {
                pid: 1,
                session_id: "session".to_string(),
                run_id: "run".to_string(),
                started_at_epoch_secs: 1,
                control_token: "token".to_string(),
            },
        }
    }

    fn register_fixture(
        model: &mut CenterModel,
        paths: &CenterPaths,
        ledger: &Utf8Path,
        token: Option<&str>,
    ) {
        let mut registration = test_registration();
        registration.ledger_path = ledger.to_string();
        registration.spawn_token = token.map(str::to_string);
        handle_request(
            model,
            paths,
            Request::Register {
                id: next_id("register-fixture"),
                registration,
            },
        )
        .expect("register fixture");
    }

    fn control_result_for_resolution(resolution: RowResolution) -> ControlWireResult {
        let (jobs, receiver) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            run_control_request(
                &jobs,
                "session-fixture".to_string(),
                Some("repository".to_string()),
                ControlAction::Interrupt,
            )
        });
        let ModelCommand::ResolveRow { reply, .. } = receiver.recv().expect("resolve request")
        else {
            panic!("control request must resolve the row first");
        };
        reply
            .send(Ok(resolution))
            .expect("reply to control request");
        worker
            .join()
            .expect("control worker exits")
            .expect("control resolution succeeds")
    }

    #[test]
    fn register_with_a_spawn_token_completes_exactly_one_pending_start() {
        let root = scratch("register-spawn-token");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let mut model = CenterModel::open(&paths).expect("open center");
        let (notify, receiver) = mpsc::sync_channel(1);
        model.pending_starts.insert(
            "spawn-token".to_string(),
            PendingStart {
                notify,
                since: Instant::now(),
            },
        );

        register_fixture(&mut model, &paths, &ledger, Some("spawn-token"));
        assert_eq!(
            receiver
                .recv_timeout(STREAM_TIMEOUT)
                .expect("start completion"),
            "session-fixture"
        );
        assert!(!model.pending_starts.contains_key("spawn-token"));
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn repeated_registration_with_the_same_spawn_token_completes_nothing() {
        let root = scratch("repeated-spawn-token");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let mut model = CenterModel::open(&paths).expect("open center");
        let (notify, receiver) = mpsc::sync_channel(2);
        model.pending_starts.insert(
            "spawn-token".to_string(),
            PendingStart {
                notify,
                since: Instant::now(),
            },
        );

        register_fixture(&mut model, &paths, &ledger, Some("spawn-token"));
        assert!(receiver.try_recv().is_ok());
        register_fixture(&mut model, &paths, &ledger, Some("spawn-token"));
        assert!(
            receiver.try_recv().is_err(),
            "reconnect must be an idempotent no-op"
        );
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn register_without_a_spawn_token_completes_no_pending_start() {
        let root = scratch("tokenless-register");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let mut model = CenterModel::open(&paths).expect("open center");
        let (notify, receiver) = mpsc::sync_channel(1);
        model.pending_starts.insert(
            "spawn-token".to_string(),
            PendingStart {
                notify,
                since: Instant::now(),
            },
        );

        register_fixture(&mut model, &paths, &ledger, None);
        assert!(receiver.try_recv().is_err());
        assert!(model.pending_starts.contains_key("spawn-token"));
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn registered_repository_local_ledger_publishes_authoritative_repo_metadata() {
        let root = scratch("registered-local-repository-metadata");
        let paths = CenterPaths {
            socket: root.join("center.sock"),
            spawn_lock: root.join("center.lock"),
            runs_root: root.join("flat-runs"),
            index: root.join("index.sqlite3"),
            liveness_root: root.join("liveness"),
        };
        std::fs::create_dir_all(paths.runs_root.as_std_path()).expect("create flat runs root");
        let repository = root.join("repository");
        let ledger = repository.join(".ctx/runs/session-fixture.json");
        std::fs::create_dir_all(ledger.parent().expect("ledger parent").as_std_path())
            .expect("create repository-local store");
        crate::run_session::write_run_session(&ledger, &fixture_session("completed"))
            .expect("write ledger");
        let lock_path = crate::run_control::driver_lock_path(&ledger);
        let mut lock = crate::file_lock::open_lock_file_no_follow(&lock_path).expect("open lock");
        assert!(crate::file_lock::try_lock_exclusive(&lock).expect("hold lock"));
        crate::file_lock::write_lock_metadata(&mut lock, &test_registration().holder)
            .expect("write holder");
        let mut model = CenterModel::open(&paths).expect("open center");
        let (outbound, received) = mpsc::sync_channel(1);
        model.subscribers.insert(
            1,
            Subscriber {
                repo_key: None,
                outbound,
                snapshot: None,
                snapshot_end_queued: false,
                pending: VecDeque::new(),
            },
        );

        register_fixture(&mut model, &paths, &ledger, None);
        let canonical =
            crate::state::canonical_repo_root(&repository).expect("canonical repository");
        let expected_key = crate::state::repo_key(&canonical);
        let row = model.rows.get(&ledger).expect("registered row");
        assert_eq!(row.repo_key, expected_key);
        assert_eq!(row.repo_path, canonical);
        assert!(row.live);
        assert_eq!(
            row.live_holder.as_ref().map(|holder| &holder.session_id),
            Some(&"session".to_string())
        );
        let Outbound::Delta(CenterDelta::Appeared { row }) = received
            .recv_timeout(STREAM_TIMEOUT)
            .expect("first registration delta")
        else {
            panic!("registration must publish Appeared");
        };
        assert_eq!(row.repo_key, expected_key);
        assert_eq!(row.repo_path, canonical);
        assert!(row.live);

        drop(model);
        let mut reopened = CenterModel::open(&paths).expect("reopen index");
        reopened
            .discover(&paths)
            .expect("reconcile held registered ledger");
        let row = reopened
            .rows
            .get(&ledger)
            .expect("persisted registered row");
        assert_eq!(row.repo_key, expected_key);
        assert_eq!(row.repo_path, canonical);
        assert!(row.live);
        assert_eq!(
            row.live_holder.as_ref().map(|holder| &holder.session_id),
            Some(&"session".to_string())
        );
        drop(lock);
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn control_resolution_reports_missing_ambiguous_and_not_live_targets() {
        let root = scratch("control-resolution");
        let paths = paths(root.clone());
        let first = write_fixture_ledger(&root, "first", "completed");
        let second = write_fixture_ledger(&root, "second", "completed");
        let mut model = CenterModel::open(&paths).expect("open center");
        model.discover(&paths).expect("discover rows");

        assert!(matches!(
            control_result_for_resolution(RowResolution::Missing),
            ControlWireResult::Missing
        ));
        assert!(matches!(
            control_result_for_resolution(RowResolution::Ambiguous(vec![
                "first".to_string(),
                "second".to_string(),
            ])),
            ControlWireResult::Ambiguous(ids) if ids == ["first", "second"]
        ));
        for ledger in [first, second] {
            let row = model.rows.get(&ledger).expect("discovered row");
            assert!(matches!(
                control_result_for_resolution(RowResolution::One(ResolvedRow {
                    session_id: row.summary.session_id.clone(),
                    ledger_path: row.ledger_path.clone(),
                    repo_path: row.repo_path.clone(),
                    live: false,
                    holder: None,
                    task_key: row.summary.task_key.clone(),
                    run_id: row.summary.run_id.clone(),
                    trait_id: row.summary.trait_id.clone(),
                })),
                ControlWireResult::NotLive
            ));
        }
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn control_resolution_rejects_a_holderless_or_mismatched_live_row_as_unverifiable() {
        let root = scratch("control-unverifiable");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "awaiting-agent-output");
        let mut model = CenterModel::open(&paths).expect("open center");
        model.discover(&paths).expect("discover row");
        let row = model.rows.get_mut(&ledger).expect("row");
        row.live = true;
        row.live_holder = None;
        let row = model.rows.get(&ledger).expect("row");
        assert!(matches!(
            control_result_for_resolution(RowResolution::One(ResolvedRow {
                session_id: row.summary.session_id.clone(),
                ledger_path: row.ledger_path.clone(),
                repo_path: row.repo_path.clone(),
                live: true,
                holder: None,
                task_key: row.summary.task_key.clone(),
                run_id: row.summary.run_id.clone(),
                trait_id: row.summary.trait_id.clone(),
            })),
            ControlWireResult::Unverifiable
        ));

        let row = model.rows.get_mut(&ledger).expect("row");
        row.live_holder = Some(crate::run_control::DriverHolder {
            pid: 1,
            session_id: "another-session".to_string(),
            run_id: "run".to_string(),
            started_at_epoch_secs: 1,
            control_token: "token".to_string(),
        });
        let row = model.rows.get(&ledger).expect("row");
        assert!(matches!(
            control_result_for_resolution(RowResolution::One(ResolvedRow {
                session_id: row.summary.session_id.clone(),
                ledger_path: row.ledger_path.clone(),
                repo_path: row.repo_path.clone(),
                live: true,
                holder: row.live_holder.clone(),
                task_key: row.summary.task_key.clone(),
                run_id: row.summary.run_id.clone(),
                trait_id: row.summary.trait_id.clone(),
            })),
            ControlWireResult::Unverifiable
        ));
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn discovery_preserves_every_settled_pause_outcome_and_clears_stale_metadata() {
        // P0253.4: `awaiting-owner` pairs with `waiting-on-human` status (a
        // parked ask), unlike the other settled pauses, which all park on
        // `awaiting-agent-output` — the real-world shape a summons leaves
        // behind, not an arbitrary substitution.
        for (status, outcome) in [
            ("awaiting-agent-output", "paused"),
            ("awaiting-agent-output", "paused-provider-credits"),
            ("awaiting-agent-output", "paused-budget-exhausted"),
            ("waiting-on-human", "awaiting-owner"),
        ] {
            let root = scratch(&format!("settled-pause-{outcome}"));
            let paths = paths(root.clone());
            let ledger = root.join("repository").join("session-fixture.json");
            std::fs::create_dir_all(ledger.parent().expect("ledger parent").as_std_path())
                .expect("create repository store");
            crate::run_session::write_run_session(
                &ledger,
                &fixture_session_with_outcome(status, outcome),
            )
            .expect("write paused ledger");
            let lock_path = crate::run_control::driver_lock_path(&ledger);
            let mut lock =
                crate::file_lock::open_lock_file_no_follow(&lock_path).expect("open lock");
            crate::file_lock::write_lock_metadata(
                &mut lock,
                &crate::run_control::DriverHolder {
                    pid: 1,
                    session_id: "stale".to_string(),
                    run_id: "stale".to_string(),
                    started_at_epoch_secs: 1,
                    control_token: String::new(),
                },
            )
            .expect("write stale metadata");
            let mut model = CenterModel::open(&paths).expect("open center");
            model.discover(&paths).expect("discover paused ledger");
            let row = model.rows.get(&ledger).expect("paused row");
            assert!(!terminal(&row.summary), "{outcome} remains resumable");
            assert_eq!(row.summary.last_drive_outcome.as_deref(), Some(outcome));
            assert!(!row.live);
            assert!(
                crate::file_lock::read_lock_metadata::<crate::run_control::DriverHolder>(&mut lock)
                    .is_none()
            );
            let _ = std::fs::remove_dir_all(root.as_std_path());
        }
    }

    #[test]
    fn discovery_does_not_interrupt_a_durably_parked_ask() {
        let root = scratch("parked-ask-settled-pause");
        let paths = paths(root.clone());
        let ledger = root.join("repository").join("session-fixture.json");
        std::fs::create_dir_all(ledger.parent().expect("ledger parent").as_std_path())
            .expect("create repository store");
        crate::run_session::write_run_session(
            &ledger,
            &parked_ask_session_with_outcome(Some("awaiting-owner")),
        )
        .expect("write parked ask ledger");

        let mut model = CenterModel::open(&paths).expect("open center");
        model.discover(&paths).expect("discover parked ask");

        let persisted = crate::run_session::read_run_session(&ledger).expect("read parked ask");
        assert!(matches!(
            persisted
                .last_drive_outcome
                .as_ref()
                .map(|outcome| &outcome.outcome),
            Some(ctx_traits_core::procedure::session::DriveOutcomeKind::AwaitingOwner)
        ));
        let row = model.rows.get(&ledger).expect("parked ask row");
        assert!(
            row.summary
                .session_state
                .is_some_and(ctx_traits_core::procedure::activity::SessionState::is_resumable)
        );
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn discovery_repair_keeps_unheld_parked_asks_resumable() {
        for (name, outcome) in [("cleared", None), ("interrupted", Some("interrupted"))] {
            let root = scratch(&format!("parked-ask-{name}"));
            let paths = paths(root.clone());
            let ledger = root.join("repository").join("session-fixture.json");
            std::fs::create_dir_all(ledger.parent().expect("ledger parent").as_std_path())
                .expect("create repository store");
            crate::run_session::write_run_session(
                &ledger,
                &parked_ask_session_with_outcome(outcome),
            )
            .expect("write parked ask ledger");

            let mut model = CenterModel::open(&paths).expect("open center");
            model.discover(&paths).expect("discover parked ask");

            let persisted =
                crate::run_session::read_run_session(&ledger).expect("read repaired ask");
            assert!(matches!(
                persisted
                    .last_drive_outcome
                    .as_ref()
                    .map(|outcome| &outcome.outcome),
                Some(ctx_traits_core::procedure::session::DriveOutcomeKind::Interrupted)
            ));
            let row = model.rows.get(&ledger).expect("parked ask row");
            assert!(
                row.summary
                    .session_state
                    .is_some_and(ctx_traits_core::procedure::activity::SessionState::is_resumable)
            );
            let _ = std::fs::remove_dir_all(root.as_std_path());
        }
    }

    #[test]
    fn unchanged_settled_pause_fingerprint_does_not_reparse_on_second_scan() {
        for outcome in [
            "paused",
            "paused-provider-credits",
            "paused-budget-exhausted",
        ] {
            let root = scratch(&format!("unchanged-settled-pause-{outcome}"));
            let paths = paths(root.clone());
            let ledger = root.join("repository").join("session-fixture.json");
            std::fs::create_dir_all(ledger.parent().expect("ledger parent").as_std_path())
                .expect("create repository store");
            crate::run_session::write_run_session(
                &ledger,
                &fixture_session_with_outcome("awaiting-agent-output", outcome),
            )
            .expect("write paused ledger");
            let mut model = CenterModel::open(&paths).expect("open index");
            model.discover(&paths).expect("initial discovery");
            assert!(!terminal(
                &model.rows.get(&ledger).expect("paused row").summary
            ));
            LEDGER_READS.with(|reads| reads.set(0));
            model.discover(&paths).expect("unchanged discovery");
            LEDGER_READS.with(|reads| {
                assert_eq!(
                    reads.get(),
                    0,
                    "{outcome}: unchanged settled pause must not reopen the ledger"
                )
            });
            assert_eq!(
                model
                    .rows
                    .get(&ledger)
                    .expect("cached row")
                    .summary
                    .last_drive_outcome
                    .as_deref(),
                Some(outcome)
            );
            let _ = std::fs::remove_dir_all(root.as_std_path());
        }
    }

    #[test]
    fn run_record_carries_non_default_projected_stats_facts_through_a_stats_query() {
        let root = scratch("stats-non-default");
        let paths = paths(root.clone());
        let mut model = CenterModel::open(&paths).expect("open index");
        let mut summary = crate::run_summary::RunSummary::unreadable(
            "session-fixture".to_string(),
            "unused".to_string(),
        );
        summary.parse_error = None;
        summary.trait_id = "fixture-trait".to_string();
        summary.status = ctx_traits_core::procedure::session::Status::Blocked;
        summary.stop_reason = Some(ctx_traits_core::procedure::runtime::StopReason {
            reason: "max-iterations-exhausted".to_string(),
            at: Vec::new(),
            last_check: None,
            message: None,
        });
        summary.tokens_by_model =
            std::collections::BTreeMap::from([("fixture-model".to_string(), 42)]);
        summary.verdict_rounds = Some(3);
        model.rows.insert(
            root.join("session-fixture.json"),
            CenterRow {
                summary,
                repo_key: "repository".to_string(),
                repo_path: "/repository".to_string(),
                ledger_path: root.join("session-fixture.json"),
                modified: SystemTime::now(),
                size: 0,
                live_holder: None,
                live: false,
            },
        );

        let report = match handle_request(
            &mut model,
            &paths,
            Request::Stats {
                id: next_id("test"),
                since_epoch: None,
                trait_id: None,
                repo_key: None,
            },
        )
        .expect("stats request succeeds")
        {
            ResponseResult::Stats(report) => report,
            other => panic!("expected Stats response, got {other:?}"),
        };
        assert_eq!(
            report.outcomes.exhausted_unapproved, 1,
            "stop_reason must classify a blocked+max-iterations row as exhausted-unapproved"
        );
        assert_eq!(
            report.tokens_by_model.get("fixture-model").copied(),
            Some(42),
            "tokens_by_model must survive the projection into the aggregate"
        );
        let _ = std::fs::remove_dir_all(root.as_std_path());
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
    fn token_correlated_notifier_waits_for_registration_ack_before_returning() {
        let (client, mut server) = UnixStream::pair().expect("notification pair");
        let (returned, finished) = mpsc::sync_channel(1);
        let driver = std::thread::spawn(move || {
            let mut client = Some(client);
            let mut registration = test_registration();
            registration.spawn_token = Some("spawn-token".to_string());
            let _notifier =
                DriverNotifier::new_with(registration, move |registration, receiver| {
                    notifier_worker_with(registration, receiver, move || {
                        Ok(client.take().expect("one notification connection"))
                    });
                });
            returned.send(()).expect("report notifier return");
        });

        let request: Request =
            serde_json::from_slice(&read_line(&mut server).expect("read token registration"))
                .expect("decode token registration");
        assert!(matches!(request, Request::Register { .. }));
        assert!(matches!(
            finished.recv_timeout(Duration::from_millis(25)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        write_line(
            &mut server,
            &WireMessage::Response {
                id: request.id().to_owned(),
                result: ResponseResult::Ok,
            },
        )
        .expect("acknowledge token registration");
        finished
            .recv_timeout(STREAM_TIMEOUT)
            .expect("token-correlated notifier return");
        driver.join().expect("driver completes");
    }

    #[test]
    fn token_correlated_notifier_waits_for_retry_acknowledgement_before_returning() {
        let (first_client, mut first_server) = UnixStream::pair().expect("first pair");
        let (second_client, mut second_server) = UnixStream::pair().expect("second pair");
        let (returned, finished) = mpsc::sync_channel(1);
        let driver = std::thread::spawn(move || {
            let mut streams = std::collections::VecDeque::from([first_client, second_client]);
            let mut registration = test_registration();
            registration.spawn_token = Some("spawn-token".to_string());
            let _notifier =
                DriverNotifier::new_with(registration, move |registration, receiver| {
                    notifier_worker_with(registration, receiver, move || {
                        Ok(streams.pop_front().expect("next notification connection"))
                    });
                });
            returned.send(()).expect("report notifier return");
        });

        let first: Request = serde_json::from_slice(
            &read_line(&mut first_server).expect("read first token registration"),
        )
        .expect("decode first token registration");
        assert!(matches!(first, Request::Register { .. }));
        assert!(matches!(
            finished.recv_timeout(Duration::from_millis(25)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));

        // Do not acknowledge the first registration. Its stream timeout forces
        // the notifier to reconnect and retry the retained registration event.
        std::thread::sleep(STREAM_TIMEOUT + NOTIFIER_BACKOFF_MIN);
        let second: Request = serde_json::from_slice(
            &read_line(&mut second_server).expect("read retried token registration"),
        )
        .expect("decode retried token registration");
        assert!(matches!(second, Request::Register { .. }));
        assert!(matches!(
            finished.recv_timeout(Duration::from_millis(25)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        write_line(
            &mut second_server,
            &WireMessage::Response {
                id: second.id().to_owned(),
                result: ResponseResult::Ok,
            },
        )
        .expect("acknowledge retried token registration");
        finished
            .recv_timeout(STREAM_TIMEOUT)
            .expect("token-correlated notifier return after retry acknowledgement");
        driver.join().expect("driver completes");
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
        let worker = std::thread::spawn(move || {
            serve_connection_worker(
                server,
                jobs,
                paths(scratch("notifications-require-registration")),
            )
        });
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
        let worker = std::thread::spawn(move || {
            serve_connection_worker(
                server,
                jobs,
                paths(scratch("registration-requires-holder-bad")),
            )
        });
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
        let worker = std::thread::spawn(move || {
            serve_connection_worker(server, jobs, paths(scratch("registration-requires-holder")))
        });
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
                    spawn_token: None,
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
            move || serve_connection_worker(stalled_server, jobs, paths(scratch("stalled-peer")))
        });

        let (mut client, server) = UnixStream::pair().expect("request pair");
        let request_worker = std::thread::spawn(move || {
            serve_connection_worker(server, jobs, paths(scratch("typed-request")))
        });
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
        let worker = std::thread::spawn(move || {
            serve_connection_worker(server, jobs, paths(scratch("handshaken-worker")))
        });
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
                spawn_token: None,
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
    fn interrupted_migration_retries_the_full_rebuild_instead_of_recording_completion() {
        let root = scratch("interrupted-migration");
        let paths = paths(root.clone());
        let model = CenterModel::open(&paths).expect("open index");
        // Simulate a crash between the rebuild's DELETE/DROP and its
        // marker write (which now happens only after VACUUM succeeds): the
        // rows and payload table are already gone, but no projection marker
        // was ever recorded for this rebuild.
        model
            .db
            .execute_batch("DELETE FROM center_rows; DELETE FROM center_projection_meta; DROP TABLE IF EXISTS center_sessions;")
            .expect("simulate the pre-marker half of a rebuild");
        drop(model);

        // Retrying `open` after this crash must not error — `DROP TABLE IF
        // EXISTS` and `DELETE` are idempotent — and it must still end with
        // the current marker recorded, proving a failed/interrupted
        // reclamation is retried rather than silently treated as done.
        let model = CenterModel::open(&paths).expect("retry the interrupted rebuild");
        let projection_version: i64 = model
            .db
            .query_row("SELECT version FROM center_projection_meta", [], |row| {
                row.get(0)
            })
            .expect("read projection marker after retry");
        assert_eq!(projection_version, CENTER_PROJECTION_VERSION);
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
        let index_size_before = std::fs::metadata(paths.index.as_std_path())
            .expect("stat v1 index")
            .len();
        drop(db);

        // A v1-shaped index has no projection marker at all. `open` wipes
        // the stale-shaped rows, drops the payload `center_sessions` table,
        // and reclaims the freed pages; the metadata-only row is rebuilt by
        // the next discovery pass reading the authoritative ledger.
        let mut model = CenterModel::open(&paths).expect("reopen and rebuild");
        assert!(
            model.rows.is_empty(),
            "marker-gated rebuild must not carry over stale-shaped rows"
        );
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
        let no_payload_table: i64 = model
            .db
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'center_sessions'",
                [],
                |row| row.get(0),
            )
            .expect("query sqlite_master");
        assert_eq!(no_payload_table, 0, "payload table must be dropped");
        let index_size_after = std::fs::metadata(paths.index.as_std_path())
            .expect("stat rebuilt index")
            .len();
        assert!(
            index_size_after <= index_size_before,
            "VACUUM must reclaim, not grow, the physical index: before={index_size_before} after={index_size_after}"
        );

        model
            .discover(&paths)
            .expect("rebuild projection from ledger");
        let row = model.rows.get(&ledger).expect("reprojected row");
        assert_eq!(row.summary.run_id, "run-fixture");
        assert_eq!(row.summary.task_key.as_deref(), Some("0243.4"));
        assert!(row.summary.parse_error.is_none());
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
            .refresh_ledger_inner(&paths, &second, false, Some(&HashMap::new()), None)
            .expect("cache second ledger");
        model
            .refresh_ledger_inner(&paths, &first, false, Some(&HashMap::new()), None)
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
                .is_some_and(|row| row.summary.parse_error.is_some())
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
    fn a_scan_longer_than_the_idle_and_scan_intervals_still_permits_idle_exit() {
        // `WARM_SLICE` ledgers drain per accept-loop tick, so 40 ledgers take
        // several ticks (well over both durations below) to finish even a
        // fully-cached validate-only pass. Before the fix this regresses,
        // measuring the periodic-scan interval from when a scan *started*
        // let a scan that outran the interval rearm on its very next tick —
        // keeping `is_warming()`, and so `has_live()`, true for effectively
        // the whole run and starving idle exit.
        let root = scratch("scan-outruns-idle");
        let paths = paths(root.clone());
        for repo in 0..40 {
            write_fixture_ledger(&root, &format!("repo-{repo}"), "completed");
        }
        let server_paths = paths.clone();
        let (completed, result) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = completed.send(run_server_at(
                server_paths,
                Duration::from_millis(30),
                Duration::from_millis(10),
            ));
        });
        assert!(
            matches!(result.recv_timeout(Duration::from_secs(5)), Ok(Ok(()))),
            "a corpus too large to warm inside one idle window must still exit once quiescent"
        );
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
                snapshot_end_queued: false,
                pending: VecDeque::new(),
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
    fn warm_scan_publishes_deltas_only_after_its_slice_commits() {
        // `drain_warm_queue` buffers each ledger's before/after row and only
        // calls `emit_delta` once the slice's `COMMIT` has actually
        // succeeded (center.rs `drain_warm_queue`) — a subscriber must never
        // observe a row a later commit failure in the same slice could
        // still roll back. This drives the real accept-loop path
        // (`begin_scan` + `warm_step`) rather than `refresh_ledger_inner`
        // directly, so it exercises the buffering, not just the emission.
        let root = scratch("warm-scan-buffered-emit");
        let paths = paths(root.clone());
        let _first = write_fixture_ledger(&root, "repo-a", "completed");
        let _second = write_fixture_ledger(&root, "repo-b", "completed");
        let mut model = CenterModel::open(&paths).expect("open center");
        let (outbound, receiver) = mpsc::sync_channel(8);
        model.subscribers.insert(
            1,
            Subscriber {
                repo_key: None,
                outbound,
                snapshot: None,
                snapshot_end_queued: false,
                pending: VecDeque::new(),
            },
        );
        model.begin_scan(&paths).expect("begin warm scan");
        assert!(model.is_warming(), "a fresh scan starts warming");
        // No delta may reach the subscriber before a slice large enough to
        // reconcile every ledger and finalize the scan has actually run.
        assert!(
            receiver.try_recv().is_err(),
            "enumeration alone must not publish anything"
        );
        while model.is_warming() {
            model.warm_step(&paths, 8).expect("drain warm queue");
        }
        let mut appeared = 0;
        while let Ok(Outbound::Delta(CenterDelta::Appeared { .. })) = receiver.try_recv() {
            appeared += 1;
        }
        assert_eq!(
            appeared, 2,
            "both ledgers must publish exactly one Appeared each, after the scan finalizes"
        );
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn warm_slice_commit_failure_emits_no_delta_and_restores_rows() {
        // SQLite's classic upgrade hazard: every write inside a deferred
        // transaction only needs a RESERVED lock, compatible with another
        // connection's open read transaction, but COMMIT must upgrade to
        // EXCLUSIVE — which that reader blocks. This reproduces a slice
        // whose per-ledger writes all succeed and only the final `COMMIT`
        // fails, exercising the rollback path a trigger-based per-row
        // failure (see `persistence_failure_restores_the_previous_verified_row`)
        // never reaches.
        let root = scratch("warm-scan-commit-failure");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repo-a", "completed");
        let mut model = CenterModel::open(&paths).expect("open center");
        let (outbound, receiver) = mpsc::sync_channel(8);
        model.subscribers.insert(
            1,
            Subscriber {
                repo_key: None,
                outbound,
                snapshot: None,
                snapshot_end_queued: false,
                pending: VecDeque::new(),
            },
        );
        model.begin_scan(&paths).expect("begin warm scan");
        assert!(model.is_warming(), "a fresh scan starts warming");

        let blocker = Connection::open(paths.index.as_std_path()).expect("open blocking reader");
        blocker
            .busy_timeout(Duration::from_millis(0))
            .expect("blocker never waits");
        blocker
            .execute_batch("BEGIN; SELECT count(*) FROM center_rows;")
            .expect("hold a shared read lock across the slice's commit");
        model
            .db
            .busy_timeout(Duration::from_millis(0))
            .expect("slice never waits either");

        assert!(
            model.warm_step(&paths, 8).is_err(),
            "commit must fail while a reader holds the database lock"
        );
        assert!(
            !model.rows.contains_key(&ledger),
            "a failed slice commit must not retain a row nothing durable backs"
        );
        assert!(
            receiver.try_recv().is_err(),
            "no delta may be published for a rolled-back slice"
        );
        assert!(
            model.is_warming(),
            "the scan stays open so the ledger is retried, not dropped"
        );

        blocker
            .execute_batch("ROLLBACK;")
            .expect("release the blocking read transaction");
        drop(blocker);

        while model.is_warming() {
            model
                .warm_step(&paths, 8)
                .expect("the retried slice commits once the blocker releases");
        }
        assert!(
            model.rows.contains_key(&ledger),
            "the requeued ledger is indexed once the retry succeeds"
        );
        match receiver.recv().expect("delta after the successful retry") {
            Outbound::Delta(CenterDelta::Appeared { row }) => {
                assert_eq!(row.ledger_path, ledger);
            }
            other => panic!("expected an appeared delta after the retry, got {other:?}"),
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
            .refresh_ledger_inner(&paths, &ledger, false, Some(&HashMap::new()), None)
            .expect("cache readable ledger");
        let (outbound, receiver) = mpsc::sync_channel(4);
        model.subscribers.insert(
            1,
            Subscriber {
                repo_key: None,
                outbound,
                snapshot: None,
                snapshot_end_queued: false,
                pending: VecDeque::new(),
            },
        );
        std::thread::sleep(Duration::from_millis(2));
        std::fs::write(ledger.as_std_path(), "not json").expect("corrupt ledger");
        model
            .refresh_ledger_inner(&paths, &ledger, true, Some(&HashMap::new()), None)
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
            .refresh_ledger_inner(&paths, &ledger, false, Some(&HashMap::new()), None)
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
                .refresh_ledger_inner(&paths, &ledger, false, Some(&HashMap::new()), None)
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
                repo_key: "repo".to_string(),
                repo_path: "/repo".to_string(),
                ledger_path: ledger.clone(),
                modified: UNIX_EPOCH + Duration::new(42, 7),
                size: 99,
                live_holder: None,
                live: false,
            },
        );
        model.persist_row(&ledger).expect("persist row");
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
    fn persistence_removes_only_deleted_ledger_rows() {
        let root = scratch("row-scoped-retention");
        let paths = paths(root.clone());
        let removed = write_fixture_ledger(&root, "removed", "completed");
        let retained = write_fixture_ledger(&root, "retained", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("populate cache");
        assert_eq!(
            model
                .db
                .query_row("SELECT COUNT(*) FROM center_rows", [], |row| row
                    .get::<_, i64>(0))
                .expect("count cached rows"),
            2
        );

        std::fs::remove_file(removed.as_std_path()).expect("remove ledger");
        model.discover(&paths).expect("reconcile deletion");
        assert_eq!(
            model
                .db
                .query_row(
                    "SELECT COUNT(*) FROM center_rows WHERE ledger = ?1",
                    params![removed.as_str()],
                    |row| row.get::<_, i64>(0),
                )
                .expect("deleted row is gone"),
            0
        );
        assert_eq!(
            model
                .db
                .query_row(
                    "SELECT COUNT(*) FROM center_rows WHERE ledger = ?1",
                    params![retained.as_str()],
                    |row| row.get::<_, i64>(0),
                )
                .expect("surviving row remains"),
            1
        );
        drop(model);

        let reopened = CenterModel::open(&paths).expect("reopen index");
        assert!(
            reopened.rows.contains_key(&retained),
            "the surviving row remains queryable after restart"
        );
        assert!(!reopened.rows.contains_key(&removed));
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
    fn prepare_spawn_never_unlinks_a_socket_while_its_serving_owner_lock_is_held() {
        let root = Utf8PathBuf::from(format!(
            "/tmp/ctx-center-owner-unlink-{}-{}",
            std::process::id(),
            epoch_millis()
        ));
        let _ = std::fs::remove_dir_all(root.as_std_path());
        std::fs::create_dir_all(root.as_std_path()).expect("create scratch directory");
        let paths = paths(root.clone());
        // A dangling socket with nothing listening: `connect` fails with
        // ECONNREFUSED, which is the fallthrough case `prepare_spawn` reaches
        // when deciding whether the pathname is safe to reclaim.
        drop(UnixListener::bind(paths.socket.as_std_path()).expect("bind dangling socket"));
        // Simulate a live owner: `run_server_at` holds this same lock for its
        // whole serving lifetime.
        let owner_lock_file = crate::file_lock::open_lock_file_no_follow(&owner_lock_path(&paths))
            .expect("open owner lock file");
        assert!(
            crate::file_lock::try_lock_exclusive(&owner_lock_file).expect("attempt owner lock"),
            "test must hold the owner lock itself to simulate a live owner"
        );

        assert!(
            prepare_spawn(&paths, STREAM_TIMEOUT).expect("prepare_spawn under a held owner lock"),
            "a held owner lock must be treated as an already-running center"
        );
        assert!(
            paths.socket.exists(),
            "prepare_spawn must never unlink a socket while its owner lock is held"
        );
        drop(owner_lock_file);
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn prepare_spawn_unlinks_a_dangling_socket_once_the_owner_lock_is_free() {
        let root = Utf8PathBuf::from(format!(
            "/tmp/ctx-center-owner-free-{}-{}",
            std::process::id(),
            epoch_millis()
        ));
        let _ = std::fs::remove_dir_all(root.as_std_path());
        std::fs::create_dir_all(root.as_std_path()).expect("create scratch directory");
        let paths = paths(root.clone());
        drop(UnixListener::bind(paths.socket.as_std_path()).expect("bind dangling socket"));

        assert!(
            !prepare_spawn(&paths, STREAM_TIMEOUT).expect("prepare_spawn with no owner"),
            "no owner lock is held, so spawning a new center is safe"
        );
        assert!(
            !paths.socket.exists(),
            "an unowned dangling socket must be reclaimed once no owner lock is held"
        );
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
    fn discovery_retains_an_unscanned_registered_ledger_until_it_is_deleted() {
        let root = scratch("unscanned-registered-live-row");
        let paths = CenterPaths {
            socket: root.join("center.sock"),
            spawn_lock: root.join("center.lock"),
            runs_root: root.join("flat-runs"),
            index: root.join("index.sqlite3"),
            liveness_root: root.join("liveness"),
        };
        std::fs::create_dir_all(paths.runs_root.as_std_path()).expect("create flat runs root");
        let ledger = root
            .join("repository")
            .join(".ctx")
            .join("runs")
            .join("session-fixture.json");
        std::fs::create_dir_all(ledger.parent().expect("ledger parent").as_std_path())
            .expect("create repository-local store");
        crate::run_session::write_run_session(
            &ledger,
            &fixture_session_with_outcome("awaiting-agent-output", "paused"),
        )
        .expect("write ledger");
        let lock_path = crate::run_control::driver_lock_path(&ledger);
        let mut lock = crate::file_lock::open_lock_file_no_follow(&lock_path).expect("open lock");
        assert!(crate::file_lock::try_lock_exclusive(&lock).expect("lock ledger"));
        crate::file_lock::write_lock_metadata(
            &mut lock,
            &crate::run_control::DriverHolder {
                pid: 1,
                session_id: "session-fixture".to_string(),
                run_id: "run-fixture".to_string(),
                started_at_epoch_secs: 1,
                control_token: "token".to_string(),
            },
        )
        .expect("write holder");
        let mut model = CenterModel::open(&paths).expect("open center");
        register_fixture(&mut model, &paths, &ledger, None);
        assert!(model.rows.get(&ledger).is_some_and(|row| row.live));

        drop(lock);
        model
            .discover(&paths)
            .expect("reprobe absent registered ledger");
        assert!(model.rows.get(&ledger).is_some_and(|row| {
            !row.live && row.summary.last_drive_outcome.as_deref() == Some("paused")
        }));
        assert!(matches!(
            select_row(&model, "session-fixture", None),
            RowSelection::One(_)
        ));
        assert!(
            !model.has_live(),
            "released unscanned row must not pin idle exit"
        );
        std::fs::remove_file(ledger.as_std_path()).expect("delete repository-local ledger");
        model.discover(&paths).expect("reconcile deleted ledger");
        assert!(!model.rows.contains_key(&ledger));
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn a_fresh_model_recovers_a_live_index_seeded_ledger_with_no_registration_or_frame() {
        // Defect 5's actual mechanism: `begin_scan` seeds its warm queue from
        // `run_liveness::read_index` before the flat-store walk, so a
        // repository-local ledger the flat enumeration can never reach is
        // still recovered as live from a fresh model — no `Request::Register`
        // (unlike `discovery_retains_an_unscanned_registered_ledger_until_it_is_deleted`,
        // which drives the same row through registration instead) and no
        // `DriverNotifier` frame ever reaches this process.
        let root = scratch("live-index-seeded-no-registration");
        let paths = CenterPaths {
            socket: root.join("center.sock"),
            spawn_lock: root.join("center.lock"),
            runs_root: root.join("flat-runs"),
            index: root.join("index.sqlite3"),
            liveness_root: root.join("liveness"),
        };
        std::fs::create_dir_all(paths.runs_root.as_std_path()).expect("create flat runs root");
        let ledger = root
            .join("repository")
            .join(".ctx")
            .join("runs")
            .join("session-pointer-only.json");
        std::fs::create_dir_all(ledger.parent().expect("ledger parent").as_std_path())
            .expect("create repository-local store");
        crate::run_session::write_run_session(
            &ledger,
            &fixture_session_with_outcome("awaiting-agent-output", "paused"),
        )
        .expect("write ledger");

        // Hold the genuine kernel flock a driver would hold — the sole
        // liveness authority — without ever registering with a center.
        let lock_path = crate::run_control::driver_lock_path(&ledger);
        let lock = crate::file_lock::open_lock_file_no_follow(&lock_path).expect("open lock");
        assert!(crate::file_lock::try_lock_exclusive(&lock).expect("lock ledger"));

        // Write only the liveness pointer row, exactly as `run_control` does
        // on lock acquisition — never a `Request::Register` and never a
        // `DriverNotifier` connection.
        crate::run_liveness::upsert_row(
            &paths.liveness_root,
            &crate::run_liveness::LiveRunFacts {
                session_id: "session-pointer-only".to_string(),
                run_id: "run-pointer-only".to_string(),
                repo_key: "repository".to_string(),
                repo_path: root.to_string(),
                ledger_path: ledger.clone(),
                worktree_path: None,
                branch: None,
                log_path: None,
            },
            std::process::id(),
            1,
        )
        .expect("write liveness pointer row");

        let mut model = CenterModel::open(&paths).expect("open fresh center");
        model
            .begin_scan(&paths)
            .expect("begin scan seeded from liveness index");
        while model.is_warming() {
            model.warm_step(&paths, 8).expect("drain warm queue");
        }

        assert!(
            model.rows.get(&ledger).is_some_and(|row| row.live),
            "a fresh model must recover liveness for a pointer-only, unregistered ledger"
        );
        assert!(
            model.has_live(),
            "a genuinely held driver lock must keep the model live"
        );
        drop(lock);
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
    fn only_get_reopens_a_constructed_ledger() {
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
        // `Get` is the one query in this list that reads its selected
        // ledger on demand; every other metadata-only query stays at zero.
        LEDGER_READS.with(|reads| assert_eq!(reads.get(), 1));
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn metadata_only_queries_serve_every_unchanged_pass_without_ledger_reads() {
        let root = scratch("restart-query-cache");
        let paths = paths(root.clone());
        let _ledger = write_fixture_ledger(&root, "repository", "completed");
        let mut initial = CenterModel::open(&paths).expect("open index");
        initial.discover(&paths).expect("populate index");
        drop(initial);

        let mut reopened = CenterModel::open(&paths).expect("reopen cached index");
        assert!(!reopened.rows.is_empty());
        LEDGER_READS.with(|reads| reads.set(0));
        // A second discovery pass over an unchanged store: the marker-gated
        // rebuild guarantees every cached row was written by the current
        // projection, so fingerprint match alone is enough — no ledger read.
        reopened.discover(&paths).expect("unchanged discovery");
        LEDGER_READS
            .with(|reads| assert_eq!(reads.get(), 0, "unchanged discovery reopened a ledger"));
        for request in [
            Request::List {
                id: next_id("test"),
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
            handle_request(&mut reopened, &paths, request).expect("metadata-only query");
        }
        LEDGER_READS
            .with(|reads| assert_eq!(reads.get(), 0, "metadata-only query reopened a ledger"));

        // `Get` is the one query that always reads the selected ledger.
        handle_request(
            &mut reopened,
            &paths,
            Request::Get {
                id: next_id("test"),
                session_id: "session-fixture".to_string(),
                repo_key: Some("repository".to_string()),
            },
        )
        .expect("get reads the ledger");
        LEDGER_READS.with(|reads| assert_eq!(reads.get(), 1));
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
        model.advance_snapshot(1);
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
    fn snapshot_serialization_is_reused_until_a_row_changes() {
        fn drain_snapshot(
            model: &mut CenterModel,
            id: u64,
            receiver: &mpsc::Receiver<Outbound>,
            rows: usize,
        ) {
            assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotStart(_))));
            for _ in 0..rows {
                model.advance_snapshot(id);
                assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotRow(_))));
            }
            model.advance_snapshot(id);
            assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotEnd)));
            model.advance_snapshot(id);
        }

        let root = scratch("snapshot-serialization-cache");
        let paths = paths(root.clone());
        let first = write_fixture_ledger(&root, "first", "completed");
        let _second = write_fixture_ledger(&root, "second", "completed");
        let _third = write_fixture_ledger(&root, "third", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("construct model");
        let rows = model.rows.len();

        let (first_sender, first_receiver) = mpsc::sync_channel(8);
        model
            .subscribe(1, "first".to_string(), None, first_sender)
            .expect("first subscribe");
        drain_snapshot(&mut model, 1, &first_receiver, rows);
        assert_eq!(model.snapshot_row_serializations, rows as u64);

        let (second_sender, second_receiver) = mpsc::sync_channel(8);
        model
            .subscribe(2, "second".to_string(), None, second_sender)
            .expect("second subscribe");
        drain_snapshot(&mut model, 2, &second_receiver, rows);
        assert_eq!(model.snapshot_row_serializations, rows as u64);

        model.broadcast_activity(first.as_str(), test_activity());
        let (third_sender, third_receiver) = mpsc::sync_channel(8);
        model
            .subscribe(3, "third".to_string(), None, third_sender)
            .expect("third subscribe");
        drain_snapshot(&mut model, 3, &third_receiver, rows);
        assert_eq!(model.snapshot_row_serializations, rows as u64 + 1);

        let (fourth_sender, fourth_receiver) = mpsc::sync_channel(8);
        model
            .subscribe(4, "fourth".to_string(), None, fourth_sender)
            .expect("fourth subscribe");
        drain_snapshot(&mut model, 4, &fourth_receiver, rows);
        assert_eq!(model.snapshot_row_serializations, rows as u64 + 1);
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
        model.advance_snapshot(1);
        assert!(matches!(
            receiver.recv(),
            Ok(Outbound::Delta(CenterDelta::ActivityLine { .. }))
        ));
        assert_eq!(model.subscribers.len(), 1);
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn board_change_racing_snapshot_follows_snapshot_end() {
        let root = scratch("board-snapshot-race");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let board_dir = crate::task_files::repo_board_dir(&root);
        std::fs::create_dir_all(board_dir.as_std_path()).expect("create board");
        std::fs::write(
            board_dir.join("board.toml").as_std_path(),
            "schema-version = \"0.2\"\nkey = \"board\"\ntitle = \"Board\"\nstatus = \"ready\"\n",
        )
        .expect("write board");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("construct model");
        let (sender, receiver) = mpsc::sync_channel(4);
        model
            .subscribe(1, "test-subscription".to_string(), None, sender)
            .expect("subscribe");
        assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotStart(_))));

        let instants = Mutex::new(HashMap::new());
        let board = assemble_board(
            "repository".to_string(),
            root.clone(),
            Vec::new(),
            &instants,
        )
        .expect("assemble board");
        model.publish_board("repository".to_string(), Box::new(board));
        model.advance_snapshot(1);
        assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotRow(_))));
        model.advance_snapshot(1);
        assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotEnd)));
        model.advance_snapshot(1);
        assert!(matches!(
            receiver.recv(),
            Ok(Outbound::BoardChanged { repo_key, .. }) if repo_key == "repository"
        ));
        assert_eq!(
            model.rows.len(),
            1,
            "fixture keeps a snapshot row in flight"
        );
        let _ = std::fs::remove_file(ledger.as_std_path());
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn snapshot_end_credit_retains_a_full_pending_board_event() {
        let root = scratch("board-snapshot-full");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let board_dir = crate::task_files::repo_board_dir(&root);
        std::fs::create_dir_all(board_dir.as_std_path()).expect("create board");
        std::fs::write(
            board_dir.join("board.toml").as_std_path(),
            "schema-version = \"0.2\"\nkey = \"board\"\ntitle = \"Board\"\nstatus = \"ready\"\n",
        )
        .expect("write board");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("construct model");
        let (sender, receiver) = mpsc::sync_channel(1);
        model
            .subscribe(1, "test-subscription".to_string(), None, sender)
            .expect("subscribe");
        assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotStart(_))));

        let instants = Mutex::new(HashMap::new());
        let board = assemble_board(
            "repository".to_string(),
            root.clone(),
            Vec::new(),
            &instants,
        )
        .expect("assemble board");
        model.publish_board("repository".to_string(), Box::new(board));
        model.advance_snapshot(1);
        assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotRow(_))));
        model.advance_snapshot(1);
        assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotEnd)));

        // A different queued message consumes the sole outbound slot while the
        // writer's SnapshotEnd credit arrives. The board change must remain
        // pending rather than disconnecting the subscriber.
        let subscriber = model.subscribers.get(&1).expect("subscriber remains");
        subscriber
            .outbound
            .try_send(Outbound::Delta(CenterDelta::ActivityLine {
                row: Box::new(public_rows(&model, None).remove(0)),
                activity: test_activity(),
            }))
            .expect("fill outbound queue");
        model.advance_snapshot(1);
        assert_eq!(model.subscribers.len(), 1, "full queue retains subscriber");
        assert!(matches!(receiver.recv(), Ok(Outbound::Delta(_))));
        model.advance_snapshot(1);
        assert!(matches!(
            receiver.recv(),
            Ok(Outbound::BoardChanged { repo_key, .. }) if repo_key == "repository"
        ));
        let _ = std::fs::remove_file(ledger.as_std_path());
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn post_snapshot_updates_drain_in_fifo_order_after_transient_fullness() {
        let root = scratch("snapshot-pending-fifo");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let board_dir = crate::task_files::repo_board_dir(&root);
        std::fs::create_dir_all(board_dir.as_std_path()).expect("create board");
        std::fs::write(
            board_dir.join("board.toml").as_std_path(),
            "schema-version = \"0.2\"\nkey = \"board\"\ntitle = \"Board\"\nstatus = \"ready\"\n",
        )
        .expect("write board");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("construct model");
        let (sender, receiver) = mpsc::sync_channel(1);
        model
            .subscribe(1, "test-subscription".to_string(), None, sender)
            .expect("subscribe");
        assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotStart(_))));

        let instants = Mutex::new(HashMap::new());
        let board = assemble_board(
            "repository".to_string(),
            root.clone(),
            Vec::new(),
            &instants,
        )
        .expect("assemble board");
        let pending = &mut model.subscribers.get_mut(&1).expect("subscriber").pending;
        pending.push_back(Outbound::Delta(CenterDelta::LibraryChanged {
            repo_keys: vec!["first".to_string()],
        }));
        pending.push_back(Outbound::BoardChanged {
            repo_key: "repository".to_string(),
            board: Box::new(board),
        });
        pending.push_back(Outbound::Delta(CenterDelta::LibraryChanged {
            repo_keys: vec!["last".to_string()],
        }));
        assert_eq!(model.subscribers[&1].pending.len(), 3);

        model.advance_snapshot(1);
        assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotRow(_))));
        model.advance_snapshot(1);
        assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotEnd)));

        // The End credit encounters an occupied outbound queue. It must retain
        // all post-snapshot updates, then each completed update credits the next.
        model.subscribers[&1]
            .outbound
            .try_send(Outbound::Delta(CenterDelta::LibraryChanged {
                repo_keys: vec!["filler".to_string()],
            }))
            .expect("fill outbound queue");
        model.advance_snapshot(1);
        assert_eq!(model.subscribers[&1].pending.len(), 3);
        assert!(matches!(
            receiver.recv(),
            Ok(Outbound::Delta(CenterDelta::LibraryChanged { .. }))
        ));

        model.acknowledge_snapshot(1, SnapshotFrame::Update);
        assert!(matches!(
            receiver.recv(),
            Ok(Outbound::Delta(CenterDelta::LibraryChanged { repo_keys })) if repo_keys == vec!["first"]
        ));
        model.acknowledge_snapshot(1, SnapshotFrame::Update);
        assert!(matches!(
            receiver.recv(),
            Ok(Outbound::BoardChanged { repo_key, .. }) if repo_key == "repository"
        ));
        model.acknowledge_snapshot(1, SnapshotFrame::Update);
        assert!(matches!(
            receiver.recv(),
            Ok(Outbound::Delta(CenterDelta::LibraryChanged { repo_keys })) if repo_keys == vec!["last"]
        ));
        model.acknowledge_snapshot(1, SnapshotFrame::Update);
        assert!(model.subscribers[&1].pending.is_empty());

        let _ = std::fs::remove_file(ledger.as_std_path());
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn row_credit_cannot_release_board_change_before_snapshot_end() {
        let root = scratch("board-snapshot-frame-credit");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let board_dir = crate::task_files::repo_board_dir(&root);
        std::fs::create_dir_all(board_dir.as_std_path()).expect("create board");
        std::fs::write(
            board_dir.join("board.toml").as_std_path(),
            "schema-version = \"0.2\"\nkey = \"board\"\ntitle = \"Board\"\nstatus = \"ready\"\n",
        )
        .expect("write board");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("construct model");
        let (sender, receiver) = mpsc::sync_channel(4);
        model
            .subscribe(1, "test-subscription".to_string(), None, sender)
            .expect("subscribe");
        assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotStart(_))));
        let line = model
            .snapshot_line(&ledger)
            .expect("serialize snapshot row")
            .expect("fixture row");
        model.subscribers.get_mut(&1).expect("subscriber").snapshot =
            Some(VecDeque::from([Arc::clone(&line), line]));

        let instants = Mutex::new(HashMap::new());
        let board = assemble_board(
            "repository".to_string(),
            root.clone(),
            Vec::new(),
            &instants,
        )
        .expect("assemble board");
        model.publish_board("repository".to_string(), Box::new(board));
        assert_eq!(model.subscribers[&1].pending.len(), 1);
        model.acknowledge_snapshot(1, SnapshotFrame::Start);
        assert!(model.subscribers[&1].snapshot_end_queued);
        assert_eq!(model.subscribers[&1].pending.len(), 1);
        assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotRow(_))));

        // SnapshotEnd is queued behind this row. Its earlier credit must not
        // release the pending BoardChanged before End reaches the peer.
        model.acknowledge_snapshot(1, SnapshotFrame::Row);
        assert!(model.subscribers[&1].snapshot.is_some());
        assert!(model.subscribers[&1].snapshot_end_queued);
        assert_eq!(model.subscribers[&1].pending.len(), 1);
        assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotRow(_))));
        assert!(matches!(receiver.recv(), Ok(Outbound::SnapshotEnd)));
        match receiver.try_recv() {
            Err(mpsc::TryRecvError::Empty) => {}
            other => panic!("row acknowledgement released pending output: {other:?}"),
        }

        model.acknowledge_snapshot(1, SnapshotFrame::End);
        assert!(matches!(
            receiver.recv(),
            Ok(Outbound::BoardChanged { repo_key, .. }) if repo_key == "repository"
        ));
        let _ = std::fs::remove_file(ledger.as_std_path());
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn later_subscriber_preserves_existing_board_fingerprint_baseline() {
        let root = scratch("board-fingerprint-baseline");
        let paths = paths(root.clone());
        let repository = root.join("repository");
        let _ledger = write_fixture_ledger(&root, "repository", "completed");
        let board_dir = crate::task_files::repo_board_dir(&repository);
        std::fs::create_dir_all(board_dir.as_std_path()).expect("create board");
        std::fs::write(
            board_dir.join("first.toml").as_std_path(),
            "schema-version = \"0.2\"\nkey = \"first\"\ntitle = \"First\"\nstatus = \"ready\"\n",
        )
        .expect("write first board");
        let mut model = CenterModel::open(&paths).expect("open index");
        model.discover(&paths).expect("construct model");
        let canonical =
            crate::state::canonical_repo_root(&repository).expect("canonical repository");
        let repo_key = crate::state::repo_key(&canonical);
        let row = model.rows.values_mut().next().expect("fixture row");
        row.repo_key = repo_key.clone();
        row.repo_path = canonical.to_string();
        let (first_sender, _first_receiver) = mpsc::sync_channel(2);
        model
            .subscribe(1, "first".to_string(), Some(repo_key.clone()), first_sender)
            .expect("first subscriber");
        let baseline = model
            .board_fingerprints
            .get(&repo_key)
            .expect("first subscription sets baseline")
            .clone();
        std::fs::write(
            board_dir.join("second.toml").as_std_path(),
            "schema-version = \"0.2\"\nkey = \"second\"\ntitle = \"Second\"\nstatus = \"ready\"\n",
        )
        .expect("mutate watched board");
        let (second_sender, _second_receiver) = mpsc::sync_channel(2);
        model
            .subscribe(
                2,
                "second".to_string(),
                Some(repo_key.clone()),
                second_sender,
            )
            .expect("second subscriber");
        assert_eq!(
            model.board_fingerprints.get(&repo_key),
            Some(&baseline),
            "a later subscription cannot absorb an existing watcher's change"
        );
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
        model.advance_snapshot(1);
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
            model.advance_snapshot(id);
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
            .refresh_ledger_inner(&paths, &ledger, false, Some(&original), None)
            .expect("cache ledger");
        let moved = HashMap::from([("repository".to_string(), "/new/path".to_string())]);
        LEDGER_READS.with(|reads| reads.set(0));
        model
            .refresh_ledger_inner(&paths, &ledger, true, Some(&moved), None)
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
            .refresh_ledger_inner(&paths, &ledger, false, Some(&indexed), None)
            .expect("cache ledger");
        LEDGER_READS.with(|reads| reads.set(0));
        model.uncertain = true;
        model
            .refresh_ledger_inner(&paths, &ledger, true, None, None)
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
                &paths,
                &ledger,
                true,
                crate::run_control::DriverProbe::Unheld {
                    stale_metadata: None,
                },
                None,
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
    fn locked_index_is_diagnosed_as_contention_not_corruption() {
        let root = scratch("busy-index");
        let paths = paths(root.clone());
        // Establish the schema first so the busy connection below contends
        // on an already-initialized database rather than racing `open`'s
        // own schema creation.
        drop(CenterModel::open(&paths).expect("initialize index"));

        let contender =
            Connection::open(paths.index.as_std_path()).expect("open contending connection");
        contender
            .execute_batch("BEGIN EXCLUSIVE")
            .expect("hold an exclusive transaction");

        let error = match CenterModel::open(&paths) {
            Ok(_) => panic!("busy index accepted as immediately openable"),
            Err(error) => error,
        };
        let message = error.to_string();
        assert!(message.contains(paths.index.as_str()));
        assert!(message.contains("busy"));
        assert!(!message.contains("invalid"));
        assert!(!message.contains("corrupt"));
        assert!(!message.contains("remove"));

        drop(contender);
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn a_non_sqlite_file_still_names_the_removable_index() {
        let root = scratch("garbage-index");
        let paths = paths(root.clone());
        std::fs::create_dir_all(paths.runs_root.as_std_path()).expect("create runs root");
        std::fs::write(paths.index.as_std_path(), b"not a sqlite database at all")
            .expect("write garbage index");

        let error = match CenterModel::open(&paths) {
            Ok(_) => panic!("garbage file accepted as a valid index"),
            Err(error) => error,
        };
        let message = error.to_string();
        assert!(message.contains(paths.index.as_str()));
        assert!(message.contains("remove"));
        let _ = std::fs::remove_dir_all(root.as_std_path());
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

    fn public_row_fixture(ledger_path: &str) -> Box<CenterPublicRow> {
        Box::new(CenterPublicRow {
            summary: crate::run_summary::RunSummary::unreadable(
                "session".to_string(),
                "fixture".to_string(),
            ),
            repo_key: "repo".to_string(),
            repo_path: "/repo".to_string(),
            ledger_path: ledger_path.to_string(),
            live: false,
            modified_epoch_secs: 0,
        })
    }

    #[test]
    fn apply_to_inserts_on_appeared_and_row_changed() {
        let mut rows = HashMap::new();
        let ledger_path = CenterDelta::Appeared {
            row: public_row_fixture("/repo/a.json"),
        }
        .apply_to(&mut rows);
        assert_eq!(ledger_path, "/repo/a.json");
        assert!(rows.contains_key("/repo/a.json"));

        let ledger_path = CenterDelta::RowChanged {
            row: public_row_fixture("/repo/a.json"),
        }
        .apply_to(&mut rows);
        assert_eq!(ledger_path, "/repo/a.json");
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn apply_to_removes_on_ended() {
        let mut rows = HashMap::new();
        CenterDelta::Appeared {
            row: public_row_fixture("/repo/a.json"),
        }
        .apply_to(&mut rows);
        let ledger_path = CenterDelta::Ended {
            row: public_row_fixture("/repo/a.json"),
        }
        .apply_to(&mut rows);
        assert_eq!(ledger_path, "/repo/a.json");
        assert!(rows.is_empty());
    }

    #[test]
    fn apply_to_ended_for_an_absent_key_is_a_no_op() {
        let mut rows = HashMap::new();
        let ledger_path = CenterDelta::Ended {
            row: public_row_fixture("/repo/never-there.json"),
        }
        .apply_to(&mut rows);
        assert_eq!(ledger_path, "/repo/never-there.json");
        assert!(rows.is_empty());
    }

    #[test]
    fn apply_to_activity_line_leaves_the_map_untouched() {
        let mut rows = HashMap::new();
        CenterDelta::Appeared {
            row: public_row_fixture("/repo/a.json"),
        }
        .apply_to(&mut rows);
        let before = rows.clone();
        let ledger_path = CenterDelta::ActivityLine {
            row: public_row_fixture("/repo/a.json"),
            activity: test_activity(),
        }
        .apply_to(&mut rows);
        assert_eq!(ledger_path, "/repo/a.json");
        assert_eq!(rows, before);
    }
}
