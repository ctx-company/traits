//! Independent integration test proving repository identity survives the
//! wire end to end: two ledgers under one runs root, carrying the same
//! run-id and session-id, land as two distinct rows once the center scans
//! them and the desktop's projection separates them by `repo_key`. Runs in
//! its own target for the same process-global env reason `center_snapshot.rs`
//! documents.

use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ctx_traits_desktop::run_row::{self, RepoScope};

struct ScratchGuard(std::path::PathBuf);

impl Drop for ScratchGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch() -> ScratchGuard {
    let dir = std::path::PathBuf::from("/tmp").join(format!(
        "ctx-desktop-repository-separation-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create scratch root");
    ScratchGuard(dir)
}

fn write_ledger(root: &std::path::Path, repo_dir: &str) {
    let ledger = camino::Utf8PathBuf::from_path_buf(root.join(repo_dir).join("session.json"))
        .expect("UTF-8 ledger path");
    let session = serde_json::from_value(serde_json::json!({
        "schema-version": "0.1.0",
        "session-id": "shared-session-id",
        "run-id": "shared-run-id",
        "trait-id": "desktop-repository-separation-trait",
        "current-run-index": 0,
        "status": "completed",
        "provenance": {
            "started-by": {"surface": "test", "caller": "desktop-repository-separation"},
            "state-source": "test",
            "started-at-epoch": 1000,
        },
        "ledger": {
            "run-id": "shared-run-id",
            "trait-id": "desktop-repository-separation-trait",
            "current-run-index": 0,
            "final-state": "completed",
        },
        "state-digest": "sha256:desktop-repository-separation",
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
fn repository_identity_survives_the_wire_and_the_projection() {
    let guard = scratch();
    let root = &guard.0;
    let socket = root.join("center.sock");
    let index = root.join("index.sqlite3");
    let liveness_root = root.join("liveness");

    write_ledger(root, "repo-a");
    write_ledger(root, "repo-b");

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
    let rows = loop {
        match updates.try_recv() {
            Ok(ctx_traits_desktop::center_link::LinkUpdate::Snapshot(rows)) if rows.len() >= 2 => {
                break rows;
            }
            Ok(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(_) => panic!("snapshot never reached both seeded rows before the deadline"),
            Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("no snapshot arrived before the deadline: {error}"),
        }
    };

    let mut repo_keys: Vec<_> = rows.iter().map(|row| row.repo_key.clone()).collect();
    repo_keys.sort();
    assert_eq!(
        repo_keys,
        vec!["repo-a".to_string(), "repo-b".to_string()],
        "the runs-root scan must key each ledger by its parent directory"
    );

    let all = run_row::project(&rows, &RepoScope::All);
    assert_eq!(
        all.len(),
        2,
        "both rows must survive an all-repository view"
    );

    let scoped = run_row::project(&rows, &RepoScope::Repo("repo-a".to_string()));
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].repo_key, "repo-a");
}
