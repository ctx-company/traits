//! Independent integration test proving the desktop's background link
//! reaches an in-process center and delivers one coherent initial snapshot
//! to a channel receiver without further center traffic. Runs alone in its
//! own target so the process-global env mutation below has no concurrent
//! observer within this crate.

use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct ScratchGuard(std::path::PathBuf);

impl Drop for ScratchGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch() -> ScratchGuard {
    let dir = std::path::PathBuf::from("/tmp").join(format!(
        "ctx-desktop-center-snapshot-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create scratch root");
    ScratchGuard(dir)
}

fn write_running_ledger(root: &std::path::Path) {
    let ledger = camino::Utf8PathBuf::from_path_buf(root.join("repository/session.json"))
        .expect("UTF-8 ledger path");
    let session = serde_json::from_value(serde_json::json!({
        "schema-version": "0.1.0",
        "session-id": "desktop-snapshot-session",
        "run-id": "desktop-snapshot-run",
        "trait-id": "desktop-snapshot-trait",
        "current-run-index": 0,
        "status": "awaiting-agent-output",
        "provenance": {
            "started-by": {"surface": "test", "caller": "desktop-center-snapshot"},
            "state-source": "test",
            "started-at-epoch": 1000,
            "task-key": "desktop-snapshot-task",
            "task-digest": "sha256:desktop-snapshot-task-digest",
            "session-title": {"state": "resolved", "attempts": 1, "title": "Desktop snapshot title"},
            "worktree": {
                "id": "desktop-snapshot-worktree",
                "branch": "ctx/run/desktop-snapshot-run",
                "path": "/tmp/desktop-snapshot-worktree",
            },
            "merge-frames": [{
                "stage": "landing",
                "status": "merged",
                "evidence": ["desktop-snapshot-commit"],
            }],
        },
        "ledger": {
            "run-id": "desktop-snapshot-run",
            "trait-id": "desktop-snapshot-trait",
            "current-run-index": 0,
            "final-state": "running",
        },
        "state-digest": "sha256:desktop-snapshot",
    }))
    .expect("fixture session");
    ctx_traits_io::run_session::write_run_session(&ledger, &session).expect("write ledger");
}

fn await_socket(socket: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match UnixStream::connect(socket) {
            Ok(_) => return,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => panic!("center did not become ready at {socket:?}: {error}"),
        }
    }
}

#[test]
fn link_delivers_one_coherent_snapshot_and_then_stays_quiet() {
    let guard = scratch();
    let root = &guard.0;
    let socket = root.join("center.sock");
    let index = root.join("index.sqlite3");
    let liveness_root = root.join("liveness");

    write_running_ledger(root);

    // SAFETY: this test owns the whole process environment for the duration
    // of the call below (it is the only test in this target) and restores
    // nothing because the process exits with it.
    unsafe {
        std::env::set_var("CTX_CENTER_SOCKET", &socket);
        std::env::set_var("CTX_CENTER_SPAWN_LOCK", root.join("center.lock"));
        std::env::set_var("CTX_CENTER_RUNS_ROOT", root);
        std::env::set_var("CTX_CENTER_INDEX", &index);
        std::env::set_var("CTX_CENTER_LIVENESS_ROOT", &liveness_root);
        std::env::set_var("CTX_CENTER_SCAN_MS", "20");
        std::env::set_var("CTX_CENTER_IDLE_MS", "30000");
        std::env::set_var("HOME", root);
    }

    std::thread::spawn(|| {
        let _ = ctx_traits_io::center::run_server();
    });
    await_socket(&socket);

    let updates = ctx_traits_desktop::center_link::start(None);
    let deadline = Instant::now() + Duration::from_secs(10);
    let update = loop {
        match updates.try_recv() {
            Ok(update) => break update,
            Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("no snapshot arrived before the deadline: {error}"),
        }
    };
    let rows = match update {
        ctx_traits_desktop::center_link::LinkUpdate::Snapshot(rows) => rows,
        ctx_traits_desktop::center_link::LinkUpdate::Unavailable(message) => {
            panic!("center link reported unavailable: {message}")
        }
    };
    assert!(
        rows.iter()
            .any(|row| row.summary.run_id == "desktop-snapshot-run"),
        "snapshot must contain the seeded run"
    );

    assert!(
        matches!(updates.try_recv(), Err(async_channel::TryRecvError::Empty)),
        "nothing further should arrive once the initial snapshot is complete"
    );
    assert!(!updates.is_closed(), "the channel must still be open");

    drop(updates);
}
