//! Independent integration test proving the task's "done when": a run
//! started entirely outside the desktop — no file scan, no center query,
//! nothing but the subscription channel — becomes visible through the same
//! ordered snapshot-plus-delta stream. Runs in its own target for the same
//! process-global env reason `center_snapshot.rs` documents.

use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ctx_traits_desktop::center_link::LinkUpdate;
use ctx_traits_desktop::dashboard::Dashboard;
use ctx_traits_desktop::run_row::RepoScope;

struct ScratchGuard(std::path::PathBuf);

impl Drop for ScratchGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch() -> ScratchGuard {
    let dir = std::path::PathBuf::from("/tmp").join(format!(
        "ctx-desktop-live-deltas-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create scratch root");
    ScratchGuard(dir)
}

fn write_ledger(root: &std::path::Path, repo_dir: &str, run_id: &str) {
    let ledger = camino::Utf8PathBuf::from_path_buf(root.join(repo_dir).join("session.json"))
        .expect("UTF-8 ledger path");
    let session = serde_json::from_value(serde_json::json!({
        "schema-version": "0.1.0",
        "session-id": format!("{run_id}-session"),
        "run-id": run_id,
        "trait-id": "desktop-live-deltas-trait",
        "current-run-index": 0,
        "status": "completed",
        "provenance": {
            "started-by": {"surface": "test", "caller": "desktop-live-deltas"},
            "state-source": "test",
            "started-at-epoch": 1000,
        },
        "ledger": {
            "run-id": run_id,
            "trait-id": "desktop-live-deltas-trait",
            "current-run-index": 0,
            "final-state": "completed",
        },
        "state-digest": format!("sha256:desktop-live-deltas-{run_id}"),
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
fn a_run_started_outside_the_desktop_becomes_visible_with_no_polling() {
    let guard = scratch();
    let root = &guard.0;
    let socket = root.join("center.sock");
    let index = root.join("index.sqlite3");
    let liveness_root = root.join("liveness");

    write_ledger(root, "repo-a", "seeded-run");

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
    let snapshot_rows = loop {
        match updates.try_recv() {
            Ok(LinkUpdate::Snapshot(rows)) => break rows,
            Ok(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(_) => panic!("snapshot never arrived before the deadline"),
            Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("no snapshot arrived before the deadline: {error}"),
        }
    };

    let mut dashboard = Dashboard::from_snapshot(snapshot_rows, RepoScope::All);

    // A run exposed entirely outside the desktop: written directly to the
    // runs root, never through any desktop-owned path.
    write_ledger(root, "repo-b", "externally-started-run");

    // Consume only the link channel — no file scan, no center query — until
    // the externally started run's `Appeared` delta arrives.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match updates.try_recv() {
            Ok(LinkUpdate::Delta(delta)) => {
                let is_the_externally_started_run =
                    delta.row().summary.run_id == "externally-started-run";
                let applied = dashboard.apply(delta);
                if applied && is_the_externally_started_run {
                    break;
                }
            }
            Ok(LinkUpdate::Snapshot(_)) => {
                panic!("a second snapshot arrived; the stream must stay exclusive")
            }
            Ok(LinkUpdate::Unavailable(message)) => {
                panic!("center link reported unavailable: {message}")
            }
            Err(async_channel::TryRecvError::Empty) => {
                if Instant::now() >= deadline {
                    panic!("the externally started run never appeared before the deadline");
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("link channel closed before the delta arrived: {error}"),
        }
    }

    assert_eq!(dashboard.len(), 2, "both rows must be present exactly once");
    let mut repo_keys: Vec<_> = dashboard
        .rows()
        .iter()
        .map(|row| row.repo_key.clone())
        .collect();
    repo_keys.sort();
    assert_eq!(repo_keys, vec!["repo-a".to_string(), "repo-b".to_string()]);
}
