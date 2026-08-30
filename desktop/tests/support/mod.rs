//! Shared scratch/env/fake-peer helpers for the desktop's failure-path
//! integration tests. Compiled into each test target that declares
//! `mod support;`, hence the blanket allow: a helper unused by one target is
//! not dead code, it is dead in that target only.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::{SystemTime, UNIX_EPOCH};

use camino::Utf8Path;
use ctx_traits_core::procedure::session::Session;
use ctx_traits_io::center::CenterPublicRow;
use ctx_traits_io::run_summary::RunSummary;

pub struct ScratchGuard(pub std::path::PathBuf);

impl Drop for ScratchGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn scratch(label: &str) -> ScratchGuard {
    let dir = std::path::PathBuf::from("/tmp").join(format!(
        "ctx-desktop-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create scratch root");
    ScratchGuard(dir)
}

/// All four `CTX_CENTER_*` vars must be set together or none
/// (`center.rs:613`) — a partial tuple is rejected as configuration, not
/// silently mixed with production paths.
///
/// SAFETY: callers of this function own the whole process environment for
/// the life of the test process — each integration file that calls it stays
/// the only test in its target for exactly this reason.
pub unsafe fn install_center_env(root: &std::path::Path) -> std::path::PathBuf {
    let socket = root.join("center.sock");
    unsafe {
        std::env::set_var("CTX_CENTER_SOCKET", &socket);
        std::env::set_var("CTX_CENTER_SPAWN_LOCK", root.join("center.lock"));
        std::env::set_var("CTX_CENTER_RUNS_ROOT", root);
        std::env::set_var("CTX_CENTER_INDEX", root.join("index.sqlite3"));
        std::env::set_var("CTX_CENTER_LIVENESS_ROOT", root.join("liveness"));
    }
    socket
}

/// Write a session ledger with the same writer production uses
/// (`write_run_session`, atomic), and return the fixture `Session` so a test
/// can assert a later read reproduces it exactly. `live` selects a
/// non-terminal status with no terminal drive outcome (a live baseline) vs.
/// a completed status (a finished baseline).
pub fn write_session_ledger(
    path: &Utf8Path,
    session_id: &str,
    run_id: &str,
    live: bool,
) -> Session {
    let status = if live {
        "awaiting-agent-output"
    } else {
        "completed"
    };
    let session: Session = serde_json::from_value(serde_json::json!({
        "schema-version": "0.1.0",
        "session-id": session_id,
        "run-id": run_id,
        "trait-id": "desktop-detail-fixture-trait",
        "current-run-index": 0,
        "status": status,
        "provenance": {
            "started-by": {"surface": "test", "caller": "desktop-detail-fixture"},
            "state-source": "test",
        },
        "ledger": {
            "run-id": run_id,
            "trait-id": "desktop-detail-fixture-trait",
            "current-run-index": 0,
            "final-state": if live { "running" } else { "completed" },
        },
        "state-digest": format!("sha256:desktop-detail-{run_id}"),
    }))
    .expect("fixture session");
    ctx_traits_io::run_session::write_run_session(path, &session).expect("write ledger");
    session
}

/// Write a session ledger with a nested loop-body sequence status (one
/// top-level loop container plus two body items in one iteration), using
/// the same production writer `write_session_ledger` uses. The loop's
/// current frame is the second body item's `ready` status, so a caller can
/// exercise "current node" projection alongside the nested hierarchy.
pub fn write_nested_session_ledger(path: &Utf8Path, session_id: &str, run_id: &str) -> Session {
    let session: Session = serde_json::from_value(serde_json::json!({
        "schema-version": "0.1.0",
        "session-id": session_id,
        "run-id": run_id,
        "trait-id": "desktop-detail-nested-fixture-trait",
        "current-run-index": 2,
        "status": "awaiting-agent-output",
        "provenance": {
            "started-by": {"surface": "test", "caller": "desktop-detail-nested-fixture"},
            "state-source": "test",
        },
        "active-path": [
            {"kind": "procedure", "id": "the-loop", "index": 0},
            {"kind": "loop", "id": "the-loop-body", "index": 1, "iteration": 0},
            {"kind": "item", "id": "second-item", "index": 1, "iteration": 0},
        ],
        "ledger": {
            "run-id": run_id,
            "trait-id": "desktop-detail-nested-fixture-trait",
            "current-run-index": 2,
            "final-state": "running",
            "sequence-statuses": [
                {
                    "sequence-index": 0,
                    "run-index": 0,
                    "item-id": "the-loop",
                    "title": "The loop",
                    "status": "pending",
                    "reason": "",
                    "position-path": [],
                },
                {
                    "sequence-index": 1,
                    "run-index": 1,
                    "item-id": "first-item",
                    "title": "First item",
                    "status": "accepted",
                    "reason": "",
                    "position-path": [
                        {"kind": "procedure", "id": "the-loop", "index": 0},
                        {"kind": "loop", "id": "the-loop-body", "index": 0, "iteration": 0},
                        {"kind": "item", "id": "first-item", "index": 0, "iteration": 0},
                    ],
                },
                {
                    "sequence-index": 2,
                    "run-index": 2,
                    "item-id": "second-item",
                    "title": "Second item",
                    "status": "ready",
                    "reason": "",
                    "position-path": [
                        {"kind": "procedure", "id": "the-loop", "index": 0},
                        {"kind": "loop", "id": "the-loop-body", "index": 1, "iteration": 0},
                        {"kind": "item", "id": "second-item", "index": 1, "iteration": 0},
                    ],
                },
            ],
        },
        "state-digest": format!("sha256:desktop-detail-nested-{run_id}"),
    }))
    .expect("nested fixture session");
    ctx_traits_io::run_session::write_run_session(path, &session).expect("write nested ledger");
    session
}

/// Append a small activity sidecar next to `ledger_path`: one activity
/// event and one narration line for `"second-item"`, plus a deliberately
/// truncated trailing line simulating a process killed mid-write. Returns
/// nothing — a caller reads it back through `ctx_traits_io::activity_sidecar`
/// exactly as production does.
pub fn write_activity_sidecar(ledger_path: &Utf8Path) {
    use ctx_traits_core::procedure::activity::{ActivityEvent, ActivityKind};
    use ctx_traits_io::activity_sidecar::ActivitySidecarWriter;

    let mut writer = ActivitySidecarWriter::open(ledger_path);
    writer.append_activity(ActivityEvent {
        sequence: 1,
        frame_id: "second-item".to_string(),
        kind: ActivityKind::RunningTool,
        text: Some("editing the file".to_string()),
        tool: Some("edit".to_string()),
        tokens: None,
        rate_limit: None,
    });
    writer.append_narration("second-item".to_string(), "working on it".to_string());
    drop(writer);

    let sidecar_path = ctx_traits_io::activity_sidecar::activity_path(ledger_path);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(sidecar_path.as_std_path())
        .expect("open sidecar for truncated append");
    file.write_all(b"{\"record\":\"activity\",\"at-ep")
        .expect("write truncated trailing line");
}

pub fn wire_row(repo_key: &str, ledger_path: &str, run_id: &str) -> CenterPublicRow {
    CenterPublicRow {
        summary: RunSummary {
            run_id: run_id.to_string(),
            ..RunSummary::unreadable(run_id.to_string(), "fixture".to_string())
        },
        repo_key: repo_key.to_string(),
        repo_path: format!("/{repo_key}"),
        ledger_path: ledger_path.to_string(),
        live: true,
        modified_epoch_secs: 0,
    }
}

/// A fake center peer: binds a `UnixListener` at the configured socket and
/// lets a test drive the wire protocol by hand, to exercise failure paths a
/// real in-process `run_server()` cannot be made to fail on cue (there is no
/// way to stop it, and a live subscriber keeps it alive past its idle
/// timeout). Mirrors `modules/cli/src/app/test_support.rs`'s `CenterPeer`,
/// which is `pub(crate)` to the CLI and therefore not importable here.
///
/// The four envelope tags below (`ready`, `snapshot-start`, `snapshot-row`,
/// `snapshot-end`, `delta`) are hand-written duplicates of the io crate's
/// private `WireMessage` (`modules/io/src/center.rs`), which is the source
/// of truth for this wire shape. Payloads are serialized from the public
/// `CenterPublicRow`/`CenterDelta`, so only the envelope tags themselves can
/// drift.
pub struct FakePeer {
    listener: UnixListener,
}

impl FakePeer {
    pub fn bind(socket: &std::path::Path) -> Self {
        let listener = UnixListener::bind(socket).expect("bind fake center peer socket");
        Self { listener }
    }

    /// Accept one client connection and complete the hello/ready handshake.
    pub fn accept(&self) -> FakePeerConnection {
        let (stream, _) = self.listener.accept().expect("accept fake center client");
        let mut connection = FakePeerConnection { stream };
        connection.complete_hello();
        connection
    }
}

pub struct FakePeerConnection {
    stream: UnixStream,
}

impl FakePeerConnection {
    fn complete_hello(&mut self) {
        let hello = self.read_line();
        let id = hello["id"].as_str().expect("hello id");
        self.write_line(&serde_json::json!({"kind": "ready", "id": id}));
    }

    /// Read the client's `Subscribe` request and return its correlation id.
    pub fn read_subscribe_id(&mut self) -> String {
        let request = self.read_line();
        assert_eq!(request["kind"], "subscribe", "expected a subscribe request");
        request["id"]
            .as_str()
            .expect("subscribe request id")
            .to_string()
    }

    pub fn send_snapshot_start(&mut self, id: &str) {
        self.write_line(&serde_json::json!({"kind": "snapshot-start", "id": id}));
    }

    pub fn send_snapshot_row(&mut self, row: &CenterPublicRow) {
        self.write_line(&serde_json::json!({"kind": "snapshot-row", "row": row}));
    }

    pub fn send_snapshot_end(&mut self) {
        self.write_line(&serde_json::json!({"kind": "snapshot-end"}));
    }

    pub fn send_delta(&mut self, delta: &ctx_traits_io::center::CenterDelta) {
        self.write_line(&serde_json::json!({"kind": "delta", "delta": delta}));
    }

    /// Serve a complete snapshot in one call: start, each row, end.
    pub fn serve_snapshot(&mut self, id: &str, rows: &[CenterPublicRow]) {
        self.send_snapshot_start(id);
        for row in rows {
            self.send_snapshot_row(row);
        }
        self.send_snapshot_end();
    }

    /// The wire shape of both a center exit and a backpressure eviction
    /// (`center.rs`'s subscriber queue overflow): a bare socket shutdown,
    /// with no close frame. The client cannot distinguish the two, and must
    /// not.
    pub fn shutdown(self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }

    /// Block until the client closes its side of the connection (EOF), or
    /// panic if that has not happened within `timeout`. Used to prove the
    /// link drops an otherwise-idle subscription promptly when its UI
    /// consumer goes away, rather than leaking it for the life of the
    /// process.
    pub fn wait_for_client_eof(&mut self, timeout: std::time::Duration) {
        self.stream
            .set_read_timeout(Some(timeout))
            .expect("set fake peer read timeout");
        let mut buf = [0u8; 1];
        match std::io::Read::read(&mut self.stream, &mut buf) {
            Ok(0) => {}
            Ok(_) => panic!("expected EOF from an idle client, got data"),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                panic!("client did not close within {timeout:?}")
            }
            Err(error) => panic!("unexpected error waiting for client EOF: {error}"),
        }
    }

    fn read_line(&mut self) -> serde_json::Value {
        let mut line = String::new();
        BufReader::new(self.stream.try_clone().expect("clone fake peer stream"))
            .read_line(&mut line)
            .expect("read fake peer line");
        serde_json::from_str(&line).expect("decode fake peer line")
    }

    fn write_line(&mut self, value: &serde_json::Value) {
        writeln!(self.stream, "{value}").expect("write fake peer line");
    }
}
