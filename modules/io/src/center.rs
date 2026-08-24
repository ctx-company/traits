//! Machine-wide run center: a small, disposable index over run ledgers.
//!
//! The SQLite database is only a restart cache. Ledgers and their driver
//! flocks remain authoritative, so a deleted database is rebuilt on the next
//! scan and a crashed center never affects a driver.

use camino::{Utf8Path, Utf8PathBuf};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const CENTER_PROCESS_SENTINEL: &str = "__ctx-center";
const MAX_LINE_BYTES: usize = 4096;
const STREAM_TIMEOUT: Duration = Duration::from_secs(2);
const CONNECT_RETRIES: usize = 20;
const RETRY_DELAY: Duration = Duration::from_millis(50);
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
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    let read = reader
        .read_until(b'\n', &mut line)
        .map_err(|source| io_error(&Utf8PathBuf::from("center socket"), source))?;
    if read == 0 {
        return Err(protocol_error("unexpected EOF"));
    }
    if line.len() > MAX_LINE_BYTES {
        return Err(protocol_error("line exceeds limit"));
    }
    if line.pop() != Some(b'\n') {
        return Err(protocol_error("unterminated line"));
    }
    Ok(line)
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
        Instant::now().elapsed().as_nanos()
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
    for attempt in 0..CONNECT_RETRIES {
        match UnixStream::connect(paths.socket.as_std_path()) {
            Ok(mut stream) => match handshake(&mut stream) {
                Ok(()) => return Ok(stream),
                Err(_) if attempt == 0 => continue,
                Err(error) => return Err(error),
            },
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                if attempt == 0 {
                    try_spawn(paths, executable)?;
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

fn try_spawn(paths: &CenterPaths, executable: &std::path::Path) -> crate::Result<()> {
    let lock = crate::file_lock::open_lock_file_no_follow(&paths.spawn_lock)
        .map_err(|source| io_error(&paths.spawn_lock, source))?;
    if !crate::file_lock::try_lock_exclusive(&lock)
        .map_err(|source| io_error(&paths.spawn_lock, source))?
    {
        return Ok(());
    }
    if UnixStream::connect(paths.socket.as_std_path()).is_ok() {
        return Ok(());
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
    crate::process::spawn_detached(
        &exe,
        &[CENTER_PROCESS_SENTINEL.to_string()],
        &cwd,
        &log,
        &[],
    )?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct CenterRow {
    pub summary: crate::run_summary::RunSummary,
    pub repo_key: String,
    pub repo_path: String,
    pub ledger_path: Utf8PathBuf,
    pub modified: SystemTime,
    pub size: u64,
    pub live_holder: Option<crate::run_control::DriverHolder>,
    pub live: bool,
}

pub struct CenterModel {
    rows: HashMap<Utf8PathBuf, CenterRow>,
    db: Connection,
}

impl CenterModel {
    pub fn open(paths: &CenterPaths) -> crate::Result<Self> {
        std::fs::create_dir_all(paths.runs_root.as_std_path())
            .map_err(|source| io_error(&paths.runs_root, source))?;
        let db = Connection::open(paths.index.as_std_path()).map_err(|source| {
            protocol_error(format!(
                "open disposable center index {}: {source}; remove {} to rebuild",
                paths.index, paths.index
            ))
        })?;
        db.busy_timeout(Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))
            .map_err(|source| {
                protocol_error(format!("configure center index {}: {source}", paths.index))
            })?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS center_meta (version INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS center_rows (ledger TEXT PRIMARY KEY, repo_key TEXT NOT NULL, repo_path TEXT NOT NULL, mtime_secs INTEGER NOT NULL, mtime_nanos INTEGER NOT NULL, size INTEGER NOT NULL, summary TEXT NOT NULL);")
            .map_err(|source| protocol_error(format!("initialize center index {}: {source}; remove {} to rebuild", paths.index, paths.index)))?;
        let version: Option<i64> = db
            .query_row("SELECT version FROM center_meta LIMIT 1", [], |r| r.get(0))
            .optional()
            .map_err(|source| {
                protocol_error(format!(
                    "read center index {}: {source}; remove {} to rebuild",
                    paths.index, paths.index
                ))
            })?;
        match version {
            None => {
                db.execute("INSERT INTO center_meta(version) VALUES (1)", [])
                    .map_err(|source| protocol_error(source.to_string()))?;
            }
            Some(1) => {}
            Some(_) => {
                return Err(protocol_error(format!(
                    "incompatible center index {}; remove it to rebuild",
                    paths.index
                )));
            }
        }
        let mut rows = HashMap::new();
        let mut statement = db.prepare("SELECT ledger, repo_key, repo_path, mtime_secs, mtime_nanos, size, summary FROM center_rows").map_err(|source| protocol_error(format!("read center index {}: {source}; remove {} to rebuild", paths.index, paths.index)))?;
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
            .map_err(|source| {
                protocol_error(format!(
                    "read center index {}: {source}; remove {} to rebuild",
                    paths.index, paths.index
                ))
            })?;
        for item in cached {
            let (ledger, repo_key, repo_path, secs, nanos, size, summary) =
                item.map_err(|source| {
                    protocol_error(format!(
                        "read center index {}: {source}; remove {} to rebuild",
                        paths.index, paths.index
                    ))
                })?;
            let summary = serde_json::from_str(&summary).map_err(|source| {
                protocol_error(format!(
                    "corrupt center index {}: {source}; remove {} to rebuild",
                    paths.index, paths.index
                ))
            })?;
            rows.insert(
                Utf8PathBuf::from(ledger.clone()),
                CenterRow {
                    summary,
                    repo_key,
                    repo_path,
                    ledger_path: Utf8PathBuf::from(ledger),
                    modified: UNIX_EPOCH + Duration::new(secs as u64, nanos as u32),
                    size: size as u64,
                    live_holder: None,
                    live: false,
                },
            );
        }
        drop(statement);
        Ok(Self { rows, db })
    }

    pub fn rows(&self) -> &HashMap<Utf8PathBuf, CenterRow> {
        &self.rows
    }

    pub fn discover(&mut self, paths: &CenterPaths) -> crate::Result<()> {
        let mut present = HashSet::new();
        for repo in crate::state::read_repo_index()? {
            let root = paths.runs_root.join(&repo.key);
            for ledger in crate::run_session::session_store_paths(Some(root.as_str()))? {
                let metadata = match std::fs::metadata(ledger.as_std_path()) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                let modified = match metadata.modified() {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                let size = metadata.len();
                present.insert(ledger.clone());
                let unchanged = self
                    .rows
                    .get(&ledger)
                    .is_some_and(|row| row.modified == modified && row.size == size);
                if !unchanged {
                    let Ok(summary) = crate::run_summary::read_summary_or_ledger(&ledger) else {
                        continue;
                    };
                    self.rows.insert(
                        ledger.clone(),
                        CenterRow {
                            summary,
                            repo_key: repo.key.clone(),
                            repo_path: repo.path.clone(),
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
                let _ = self.refresh_liveness(&ledger);
            }
        }
        self.rows.retain(|path, _| present.contains(path));
        self.persist()?;
        Ok(())
    }

    fn refresh_liveness(&mut self, ledger: &Utf8Path) -> crate::Result<()> {
        let Some(row) = self.rows.get_mut(ledger) else {
            return Ok(());
        };
        if terminal(&row.summary) {
            row.live = false;
            row.live_holder = None;
            return Ok(());
        }
        match crate::run_control::probe(ledger)? {
            crate::run_control::DriverProbe::Held(holder) => {
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
                        // A driver cannot appear until this guard drops. Re-read under it
                        // before repairing the orphan found by the first probe.
                        if let Ok(summary) = crate::run_summary::read_summary_or_ledger(ledger)
                            && !terminal(&summary)
                        {
                            crate::run_session::record_interrupted_outcome(ledger)?;
                            maintenance.clear_stale_metadata()?;
                            if let (Ok(summary), Ok(metadata)) = (
                                crate::run_summary::read_summary_or_ledger(ledger),
                                std::fs::metadata(ledger.as_std_path()),
                            ) && let Ok(modified) = metadata.modified()
                            {
                                row.summary = summary;
                                row.modified = modified;
                                row.size = metadata.len();
                            }
                        }
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
            let elapsed = row.modified.duration_since(UNIX_EPOCH).unwrap_or_default();
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
                        elapsed.as_secs() as i64,
                        elapsed.subsec_nanos() as i64,
                        row.size as i64,
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
        self.rows.values().any(|row| row.live)
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

struct SocketGuard(Utf8PathBuf);
impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0.as_std_path());
    }
}

/// Entry point for the private sentinel. It intentionally has no command API:
/// accepted connections only prove that the shared center is alive.
pub fn run_server() -> crate::Result<()> {
    run_server_at(production_paths()?, IDLE_TIMEOUT, SCAN_INTERVAL)
}

fn run_server_at(paths: CenterPaths, idle: Duration, scan_interval: Duration) -> crate::Result<()> {
    if paths.socket.exists() {
        return Err(protocol_error(format!(
            "center socket already exists: {}",
            paths.socket
        )));
    }
    let listener = UnixListener::bind(paths.socket.as_std_path())
        .map_err(|source| io_error(&paths.socket, source))?;
    let _guard = SocketGuard(paths.socket.clone());
    listener
        .set_nonblocking(true)
        .map_err(|source| io_error(&paths.socket, source))?;
    let mut model = CenterModel::open(&paths)?;
    model.discover(&paths)?;
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
            model.discover(&paths)?;
            last_scan = Instant::now();
        }
        if last_work.elapsed() >= idle {
            model.discover(&paths)?;
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
        let paths = CenterPaths {
            socket: root.join("center.sock"),
            spawn_lock: root.join("center.lock"),
            runs_root: root.clone(),
            index: root.join("index.sqlite3"),
        };
        let model = CenterModel::open(&paths).expect("open index");
        assert!(model.rows().is_empty());
        drop(model);
        let reopened = CenterModel::open(&paths).expect("reopen index");
        assert!(reopened.rows().is_empty());
        let _ = std::fs::remove_dir_all(root.as_std_path());
    }
}
