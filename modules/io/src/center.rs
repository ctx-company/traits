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
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(test)]
thread_local! {
    static LEDGER_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn read_summary(ledger: &Utf8Path) -> crate::Result<crate::run_summary::RunSummary> {
    #[cfg(test)]
    LEDGER_READS.with(|reads| reads.set(reads.get() + 1));
    crate::run_summary::read_summary_or_ledger(ledger)
}

pub const CENTER_PROCESS_SENTINEL: &str = "__ctx-center";
const MAX_LINE_BYTES: usize = 4096;
const STREAM_TIMEOUT: Duration = Duration::from_secs(2);
// This covers the complete bounded spawn-readiness lease as well as normal
// connection retries. A caller that lost arbitration must not give up while
// the winner is still exclusively bringing its child online.
const CONNECT_RETRIES: usize = 220;
const RETRY_DELAY: Duration = Duration::from_millis(50);
// Opening the shared index can wait for another version's short SQLite
// transaction. Keep the spawn lease through that bounded startup window.
const SPAWN_READY_TIMEOUT: Duration = Duration::from_secs(10);
const SCAN_INTERVAL: Duration = Duration::from_secs(2);
const IDLE_TIMEOUT: Duration = Duration::from_secs(15);
const SQLITE_BUSY_TIMEOUT_MS: u64 = 2_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CenterPaths {
    pub socket: Utf8PathBuf,
    pub spawn_lock: Utf8PathBuf,
    pub runs_root: Utf8PathBuf,
    pub index: Utf8PathBuf,
}

/// Version-scoped socket paths prevent two installed versions from speaking an
/// incompatible protocol. The durable index is deliberately shared.
pub fn production_paths() -> crate::Result<CenterPaths> {
    let uid = unsafe { libc::getuid() };
    let stem = format!("/tmp/ctx-{uid}-{}", env!("CARGO_PKG_VERSION"));
    let runs_root = crate::state::global_runs_family_root()?;
    Ok(CenterPaths {
        socket: Utf8PathBuf::from(format!("{stem}.sock")),
        spawn_lock: Utf8PathBuf::from(format!("{stem}.spawn.lock")),
        index: runs_root.join("index.sqlite3"),
        runs_root,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Hello<'a> {
    kind: &'a str,
    id: String,
}

#[derive(Debug, Deserialize)]
struct Ready {
    kind: String,
    id: String,
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
    stream
        .set_nonblocking(true)
        .map_err(|source| io_error(&Utf8PathBuf::from("center socket"), source))?;
    let result = (|| {
        let mut line = Vec::new();
        let deadline = Instant::now() + STREAM_TIMEOUT;
        loop {
            if line.len() == MAX_LINE_BYTES {
                return Err(protocol_error("line exceeds limit"));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(protocol_error("read timed out"));
            }
            let mut byte = [0u8; 1];
            match stream.read(&mut byte) {
                Ok(0) if line.is_empty() => return Err(protocol_error("unexpected EOF")),
                Ok(0) => return Err(protocol_error("unterminated line")),
                Ok(_) if byte[0] == b'\n' => return Ok(line),
                Ok(_) => line.push(byte[0]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(remaining.min(Duration::from_millis(5)));
                }
                Err(source) => return Err(io_error(&Utf8PathBuf::from("center socket"), source)),
            }
        }
    })();
    let reset = stream
        .set_nonblocking(false)
        .map_err(|source| io_error(&Utf8PathBuf::from("center socket"), source));
    reset?;
    result
}

fn handshake(stream: &mut UnixStream) -> crate::Result<()> {
    stream
        .set_read_timeout(Some(STREAM_TIMEOUT))
        .map_err(|source| io_error(&Utf8PathBuf::from("center socket"), source))?;
    stream
        .set_write_timeout(Some(STREAM_TIMEOUT))
        .map_err(|source| io_error(&Utf8PathBuf::from("center socket"), source))?;
    let id = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    write_line(
        stream,
        &Hello {
            kind: "hello",
            id: id.clone(),
        },
    )?;
    let ready: Ready = serde_json::from_slice(&read_line(stream)?).map_err(|source| {
        crate::parse::Error::JsonDeserialize {
            context: "decode center ready line".to_string(),
            source,
        }
    })?;
    if ready.kind != "ready" || ready.id != id {
        return Err(protocol_error("unexpected ready response"));
    }
    Ok(())
}

/// Connect to the center, launching at most one detached center across all
/// simultaneous callers. A complete handshake retries once after EOF.
pub fn ensure_connected() -> crate::Result<UnixStream> {
    ensure_connected_at(
        &production_paths()?,
        &std::env::current_exe()
            .map_err(|source| io_error(&Utf8PathBuf::from("current executable"), source))?,
    )
}

fn ensure_connected_at(
    paths: &CenterPaths,
    executable: &std::path::Path,
) -> crate::Result<UnixStream> {
    let mut retried_eof = false;
    let mut spawned = false;
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
                Err(error) => return Err(error),
            },
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                if !spawned {
                    try_spawn(paths, executable)?;
                    spawned = true;
                }
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
}

fn try_spawn(paths: &CenterPaths, executable: &std::path::Path) -> crate::Result<()> {
    let lock = crate::file_lock::open_lock_file_no_follow(&paths.spawn_lock)
        .map_err(|source| io_error(&paths.spawn_lock, source))?;
    if !crate::file_lock::try_lock_exclusive(&lock)
        .map_err(|source| io_error(&paths.spawn_lock, source))?
    {
        return Ok(());
    }
    match UnixStream::connect(paths.socket.as_std_path()) {
        Ok(mut stream) => {
            // A pathname accepting a connection is not sufficient evidence that it
            // is our usable center; verify the correlated protocol under the lock.
            if handshake(&mut stream).is_ok() {
                return Ok(());
            }
            return Err(protocol_error(format!(
                "active socket at {} did not complete the center handshake",
                paths.socket
            )));
        }
        Err(error)
            if !matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Err(io_error(&paths.socket, error));
        }
        Err(_) => {}
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
    let exe = Utf8PathBuf::from_path_buf(executable.to_path_buf())
        .map_err(|path| protocol_error(format!("non-UTF-8 executable {}", path.display())))?;
    let cwd = Utf8PathBuf::from_path_buf(
        std::env::current_dir().map_err(|source| io_error(&paths.socket, source))?,
    )
    .map_err(|path| protocol_error(format!("non-UTF-8 current directory {}", path.display())))?;
    let log = paths.socket.with_extension("log");
    let mut child = crate::process::spawn_detached(
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
    )?;
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
                return abort_spawn(&mut child, io_error(&paths.socket, source));
            }
        };
        if let Some(status) = child_status {
            return Err(protocol_error(format!(
                "spawned center exited before readiness with {status}"
            )));
        }
        match UnixStream::connect(paths.socket.as_std_path()) {
            Ok(mut stream) => match handshake(&mut stream) {
                Ok(()) => return Ok(()),
                Err(_) => {
                    return abort_spawn(
                        &mut child,
                        protocol_error("spawned center did not complete the handshake"),
                    );
                }
            },
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                std::thread::sleep(RETRY_DELAY)
            }
            Err(error) => return abort_spawn(&mut child, io_error(&paths.socket, error)),
        }
    }
    // Never drop the arbitration lock while a timed-out detached child is
    // still alive: that would permit a later caller to spawn a second center
    // before the slow child binds. Terminate and reap it before releasing the
    // lock; its possible socket pathname is then recovered by the next winner.
    abort_spawn(
        &mut child,
        protocol_error(format!(
            "spawned center did not become ready at {} within {:?}",
            paths.socket, SPAWN_READY_TIMEOUT
        )),
    )
}

/// Do not release spawn arbitration while a child from this attempt survives.
/// The caller still holds the spawn lock when this is called.
fn abort_spawn(child: &mut std::process::Child, failure: crate::Error) -> crate::Result<()> {
    let deadline = Instant::now() + SPAWN_READY_TIMEOUT;
    let mut kill_error = None;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return match kill_error {
                    Some(source) => Err(io_error(&Utf8PathBuf::from("center child"), source)),
                    None => Err(failure),
                };
            }
            Ok(None) | Err(_) => {
                // A failed observation is not proof that the child cannot yet
                // publish a listener. Keep the arbitration lease while asking
                // it to exit, but never make cleanup itself an unbounded wait.
                if kill_error.is_none() {
                    kill_error = child
                        .kill()
                        .err()
                        .filter(|error| error.kind() != std::io::ErrorKind::InvalidInput);
                }
            }
        }
        if Instant::now() >= deadline {
            return Err(protocol_error(format!(
                "could not confirm spawned center exit within {:?}",
                SPAWN_READY_TIMEOUT
            )));
        }
        std::thread::sleep(RETRY_DELAY);
    }
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
    db: Connection,
    // An unverified row is deliberately treated as live for idle purposes:
    // cache data is derived, while an unprobeable driver lock is authoritative.
    uncertain: bool,
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
        db.execute_batch("CREATE TABLE IF NOT EXISTS center_meta (version INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS center_rows (ledger TEXT PRIMARY KEY, repo_key TEXT NOT NULL, repo_path TEXT NOT NULL, mtime_secs INTEGER NOT NULL, mtime_nanos INTEGER NOT NULL, size INTEGER NOT NULL, summary TEXT NOT NULL);")
            .map_err(|source| Self::cache_error(paths, format!("initialize schema: {source}")))?;
        let version: Option<i64> = db
            .query_row("SELECT version FROM center_meta LIMIT 1", [], |r| r.get(0))
            .optional()
            .map_err(|source| Self::cache_error(paths, format!("read schema: {source}")))?;
        match version {
            None => {
                db.execute("INSERT INTO center_meta(version) VALUES (1)", [])
                    .map_err(|source| {
                        Self::cache_error(paths, format!("write schema: {source}"))
                    })?;
            }
            Some(1) => {}
            Some(_) => {
                return Err(Self::cache_error(paths, "incompatible schema version"));
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
            db,
            uncertain: false,
        })
    }

    fn discover(&mut self, paths: &CenterPaths) -> crate::Result<()> {
        let mut present = HashSet::new();
        let mut unscanned_roots = HashSet::new();
        let mut incomplete_directory_listing = false;
        self.uncertain = false;
        let indexed: HashMap<_, _> = match crate::state::read_repo_index() {
            Ok(index) => index
                .into_iter()
                .map(|repo| (repo.key.clone(), repo.path))
                .collect(),
            Err(_) => {
                self.uncertain = true;
                HashMap::new()
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
            let repo_path = indexed.get(&repo_key).cloned().unwrap_or_default();
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
                let metadata = match std::fs::metadata(ledger.as_std_path()) {
                    Ok(value) => value,
                    Err(_) => {
                        self.uncertain = true;
                        let _ = self.refresh_liveness(&ledger, false);
                        continue;
                    }
                };
                let modified = match metadata.modified() {
                    Ok(value) => value,
                    Err(_) => {
                        self.uncertain = true;
                        let _ = self.refresh_liveness(&ledger, false);
                        continue;
                    }
                };
                let size = metadata.len();
                let unchanged = self
                    .rows
                    .get(&ledger)
                    .is_some_and(|row| row.modified == modified && row.size == size);
                if let Some(row) = self.rows.get_mut(&ledger) {
                    // Repository metadata is independent of ledger content,
                    // including a changed ledger we cannot currently parse.
                    row.repo_key = repo_key.clone();
                    row.repo_path = repo_path.clone();
                }
                if !unchanged {
                    let Ok(summary) = read_summary(&ledger) else {
                        // Do not advance the fingerprint for unreadable content:
                        // the retained row is probed and retried next scan.
                        self.uncertain = true;
                        let _ = self.refresh_liveness(&ledger, false);
                        continue;
                    };
                    self.rows.insert(
                        ledger.clone(),
                        CenterRow {
                            summary,
                            repo_key: repo_key.clone(),
                            repo_path: repo_path.clone(),
                            ledger_path: ledger.clone(),
                            modified,
                            size,
                            live_holder: None,
                            live: false,
                        },
                    );
                }
                // A damaged lock or a ledger that changes under us must not
                // hide the rest of the machine-wide inventory. Keep the last
                // good cached row and try this path again on the next pass.
                if self.refresh_liveness(&ledger, unchanged).is_err() {
                    self.uncertain = true;
                    if let Some(row) = self.rows.get_mut(&ledger) {
                        row.live = true;
                        row.live_holder = None;
                    }
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

    fn refresh_liveness(&mut self, ledger: &Utf8Path, unchanged: bool) -> crate::Result<()> {
        let Some(row) = self.rows.get_mut(ledger) else {
            return Ok(());
        };
        match crate::run_control::probe(ledger)? {
            crate::run_control::DriverProbe::Held(holder) => {
                // The kernel lock remains authoritative even if the ledger has
                // already reached a terminal state. A finishing driver must
                // prevent idle exit until it has released the lock.
                row.live = true;
                row.live_holder = holder;
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
                        let unchanged_under_maintenance = unchanged
                            && std::fs::metadata(ledger.as_std_path())
                                .and_then(|metadata| {
                                    metadata.modified().map(|modified| {
                                        modified == row.modified && metadata.len() == row.size
                                    })
                                })
                                .map_err(|source| io_error(ledger, source))?;
                        if unchanged_under_maintenance && terminal(&row.summary) {
                            maintenance.clear_stale_metadata()?;
                            row.live = false;
                            row.live_holder = None;
                            return Ok(());
                        }
                        // A driver cannot appear until this guard drops. Re-read under it
                        // before repairing the orphan found by the first probe.
                        let summary = read_summary(ledger)?;
                        if !terminal(&summary) {
                            crate::run_session::record_interrupted_outcome(ledger)?;
                        }
                        // Clear stale display metadata for both terminal and repaired
                        // ledgers while maintenance ownership still excludes a driver.
                        maintenance.clear_stale_metadata()?;
                        // The second read happens under maintenance ownership,
                        // so terminal races and repaired ledgers both refresh.
                        let summary = read_summary(ledger)?;
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
        }
        transaction
            .commit()
            .map_err(|source| protocol_error(source.to_string()))?;
        Ok(())
    }

    fn has_live(&self) -> bool {
        self.uncertain || self.rows.values().any(|row| row.live)
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
    let configured = |name: &str| std::env::var(name).ok().map(Utf8PathBuf::from);
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
    match (
        configured("CTX_CENTER_SOCKET"),
        configured("CTX_CENTER_SPAWN_LOCK"),
        configured("CTX_CENTER_RUNS_ROOT"),
        configured("CTX_CENTER_INDEX"),
    ) {
        (Some(socket), Some(spawn_lock), Some(runs_root), Some(index)) => run_server_at(
            CenterPaths {
                socket,
                spawn_lock,
                runs_root,
                index,
            },
            idle,
            scan_interval,
        ),
        (None, None, None, None) => run_server_at(production_paths()?, idle, scan_interval),
        _ => Err(protocol_error("incomplete private center configuration")),
    }
}

fn run_server_at(paths: CenterPaths, idle: Duration, scan_interval: Duration) -> crate::Result<()> {
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
    listener
        .set_nonblocking(true)
        .map_err(|source| io_error(&paths.socket, source))?;
    let mut last_work = Instant::now();
    let mut last_scan = Instant::now();
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = serve_handshake(&mut stream);
                last_work = Instant::now();
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(io_error(&paths.socket, error)),
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

fn serve_handshake(stream: &mut UnixStream) -> crate::Result<()> {
    stream
        .set_read_timeout(Some(STREAM_TIMEOUT))
        .map_err(|source| io_error(&Utf8PathBuf::from("center socket"), source))?;
    stream
        .set_write_timeout(Some(STREAM_TIMEOUT))
        .map_err(|source| io_error(&Utf8PathBuf::from("center socket"), source))?;
    let hello: serde_json::Value =
        serde_json::from_slice(&read_line(stream)?).map_err(|source| {
            crate::parse::Error::JsonDeserialize {
                context: "decode center hello line".to_string(),
                source,
            }
        })?;
    let id = hello
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| protocol_error("hello is missing id"))?;
    if hello.get("kind").and_then(serde_json::Value::as_str) != Some("hello") {
        return Err(protocol_error("expected hello"));
    }
    write_line(stream, &serde_json::json!({"kind":"ready", "id":id}))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn cached_summary() -> String {
        r#"{"session_id":"session","run_id":"run","trait_id":"trait","status":"awaiting-input","has_merge_frames":false}"#.to_string()
    }

    fn fixture_session(status: &str) -> ctx_traits_core::procedure::session::Session {
        serde_json::from_value(serde_json::json!({
            "schema-version": "0.1.0",
            "session-id": "session-fixture",
            "run-id": "run-fixture",
            "trait-id": "fixture-trait",
            "current-run-index": 0,
            "status": status,
            "provenance": {
                "started-by": {"surface": "test", "caller": "center-fixture"},
                "state-source": "test",
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

    #[test]
    fn version_scoped_paths_are_distinct() {
        let root = scratch("paths");
        let a = CenterPaths {
            socket: root.join("ctx-1.sock"),
            spawn_lock: root.join("ctx-1.lock"),
            runs_root: root.clone(),
            index: root.join("index.sqlite3"),
        };
        let b = CenterPaths {
            socket: root.join("ctx-2.sock"),
            spawn_lock: root.join("ctx-2.lock"),
            runs_root: root.clone(),
            index: root.join("index.sqlite3"),
        };
        assert_ne!(a.socket, b.socket);
        assert_ne!(a.spawn_lock, b.spawn_lock);
        assert_eq!(a.index, b.index);
        let _ = std::fs::remove_dir_all(root.as_std_path());
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
    fn incompatible_schema_names_the_disposable_index() {
        let root = scratch("schema");
        let paths = paths(root.clone());
        let model = CenterModel::open(&paths).expect("open index");
        model
            .db
            .execute("UPDATE center_meta SET version = 999", [])
            .expect("make schema incompatible");
        drop(model);
        let error = match CenterModel::open(&paths) {
            Ok(_) => panic!("incompatible schema accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains(paths.index.as_str()));
        assert!(error.to_string().contains("remove"));
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
    fn changed_terminal_cache_is_reread_and_repaired() {
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
        LEDGER_READS.with(|reads| assert!(reads.get() >= 2));
        assert!(
            crate::run_session::read_run_session(&ledger)
                .expect("read repaired ledger")
                .last_drive_outcome
                .is_some()
        );
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }

    #[test]
    fn terminal_cache_changed_after_initial_stat_is_reread_under_maintenance() {
        let root = scratch("terminal-post-stat-change");
        let paths = paths(root.clone());
        let ledger = write_fixture_ledger(&root, "repository", "completed");
        let mut model = CenterModel::open(&paths).expect("open index");

        model.discover(&paths).expect("initial discovery");
        // This is the state discovery would have observed before a finishing
        // driver releases its lock. The stale `unchanged` argument must not
        // bypass the maintenance-owned re-stat below.
        crate::run_session::write_run_session(&ledger, &fixture_session("awaiting-agent-output"))
            .expect("replace with nonterminal ledger");
        LEDGER_READS.with(|reads| reads.set(0));
        model
            .refresh_liveness(&ledger, true)
            .expect("maintenance rereads changed ledger");

        let row = model.rows.get(&ledger).expect("repaired row");
        assert!(terminal(&row.summary));
        LEDGER_READS.with(|reads| assert!(reads.get() >= 2));
        assert!(
            crate::run_session::read_run_session(&ledger)
                .expect("read repaired ledger")
                .last_drive_outcome
                .is_some()
        );
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
