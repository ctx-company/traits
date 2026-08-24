//! Process-boundary smoke proof for the private center sentinel.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use camino::Utf8PathBuf;

// These proofs launch the same binary and compete for CPU during test-suite
// startup. Serializing their short-lived sentinels removes scheduler-dependent
// readiness failures without changing production arbitration behavior.
static SENTINEL_TEST_LOCK: Mutex<()> = Mutex::new(());
const PROCESS_DEADLINE: Duration = Duration::from_secs(10);

struct ChildGuard(std::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn scratch(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from("/tmp").join(format!(
        "ctx-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ))
}

fn await_socket(socket: &std::path::Path) -> UnixStream {
    let deadline = Instant::now() + PROCESS_DEADLINE;
    loop {
        match UnixStream::connect(socket) {
            Ok(stream) => return stream,
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!(
                "center listener did not become ready at {}: {error}",
                socket.display()
            ),
        }
    }
}

fn await_exit(child: &mut std::process::Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + PROCESS_DEADLINE;
    loop {
        match child.try_wait().expect("poll child") {
            Some(status) => return status,
            None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("center did not exit before {:?}", PROCESS_DEADLINE);
            }
        }
    }
}

fn await_socket_removal(socket: &std::path::Path) {
    let deadline = Instant::now() + PROCESS_DEADLINE;
    while socket.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !socket.exists(),
        "center socket was not removed before {PROCESS_DEADLINE:?}: {}",
        socket.display()
    );
}

struct CenterEnvironment {
    names: [&'static str; 8],
    previous: Vec<Option<std::ffi::OsString>>,
}

impl CenterEnvironment {
    fn install(root: &std::path::Path) -> Self {
        let names = [
            "CTX_CENTER_SOCKET",
            "CTX_CENTER_SPAWN_LOCK",
            "CTX_CENTER_RUNS_ROOT",
            "CTX_CENTER_INDEX",
            "CTX_CENTER_EXECUTABLE",
            "CTX_CENTER_IDLE_MS",
            "CTX_CENTER_SCAN_MS",
            "CTX_CENTER_LAUNCH_MARKER",
        ];
        let previous = names.iter().map(std::env::var_os).collect();
        // Environment mutation is serialized by SENTINEL_TEST_LOCK for the
        // whole proof, so no concurrently executing test can observe it.
        unsafe {
            std::env::set_var("CTX_CENTER_SOCKET", root.join("center.sock"));
            std::env::set_var("CTX_CENTER_SPAWN_LOCK", root.join("center.lock"));
            std::env::set_var("CTX_CENTER_RUNS_ROOT", root);
            std::env::set_var("CTX_CENTER_INDEX", root.join("index.sqlite3"));
            std::env::set_var("CTX_CENTER_EXECUTABLE", env!("CARGO_BIN_EXE_ctx"));
            std::env::set_var("CTX_CENTER_IDLE_MS", "100");
            std::env::set_var("CTX_CENTER_SCAN_MS", "20");
            std::env::set_var("CTX_CENTER_LAUNCH_MARKER", root.join("launches"));
        }
        Self { names, previous }
    }
}

impl Drop for CenterEnvironment {
    fn drop(&mut self) {
        unsafe {
            for (name, previous) in self.names.iter().zip(self.previous.iter()) {
                match previous {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}

fn write_running_ledger(root: &std::path::Path) -> Utf8PathBuf {
    let root = Utf8PathBuf::from_path_buf(root.to_path_buf()).expect("UTF-8 scratch root");
    let ledger = root.join("repository/session.json");
    let session = serde_json::from_value(serde_json::json!({
        "schema-version": "0.1.0",
        "session-id": "center-proof-session",
        "run-id": "center-proof-run",
        "trait-id": "center-proof-trait",
        "current-run-index": 0,
        "status": "awaiting-agent-output",
        "provenance": {
            "started-by": {"surface": "test", "caller": "proof-center"},
            "state-source": "test",
            "started-at-epoch": 1000,
        },
        "ledger": {
            "run-id": "center-proof-run",
            "trait-id": "center-proof-trait",
            "current-run-index": 0,
            "final-state": "running",
        },
        "state-digest": "sha256:center-proof",
    }))
    .expect("fixture session");
    ctx_traits_io::run_session::write_run_session(&ledger, &session).expect("write ledger");
    ledger
}

fn spawn_sentinel(
    root: &std::path::Path,
    socket: &std::path::Path,
    index: &std::path::Path,
    idle_ms: &str,
) -> ChildGuard {
    ChildGuard(
        std::process::Command::new(env!("CARGO_BIN_EXE_ctx"))
            .arg("__ctx-center")
            .env("CTX_CENTER_SOCKET", socket)
            .env("CTX_CENTER_SPAWN_LOCK", root.join("center.lock"))
            .env("CTX_CENTER_RUNS_ROOT", root)
            .env("CTX_CENTER_INDEX", index)
            .env("CTX_CENTER_IDLE_MS", idle_ms)
            .env("CTX_CENTER_SCAN_MS", "20")
            .spawn()
            .expect("spawn private sentinel"),
    )
}

#[test]
fn center_sentinel_is_not_a_supported_clap_command() {
    // The sentinel is consumed before Clap by the binary entry point. Keeping
    // it absent from help prevents it becoming a supported user command.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ctx"))
        .arg("--help")
        .output()
        .expect("run ctx help");
    assert!(output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("__ctx-center"));
}

#[test]
fn private_sentinel_serves_only_the_center_handshake() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    // AF_UNIX paths have a small kernel limit; use /tmp instead of the
    // platform's potentially deep temporary directory.
    let root = scratch("center");
    std::fs::create_dir_all(&root).expect("create scratch root");
    let socket = root.join("center.sock");
    let mut child = ChildGuard(
        std::process::Command::new(env!("CARGO_BIN_EXE_ctx"))
            .arg("__ctx-center")
            .env("CTX_CENTER_SOCKET", &socket)
            .env("CTX_CENTER_SPAWN_LOCK", root.join("center.lock"))
            .env("CTX_CENTER_RUNS_ROOT", &root)
            .env("CTX_CENTER_INDEX", root.join("index.sqlite3"))
            .spawn()
            .expect("spawn private sentinel"),
    );
    let mut stream = await_socket(&socket);
    stream
        .write_all(b"{\"kind\":\"hello\",\"id\":\"proof\"}\n")
        .expect("write hello");
    let mut ready = String::new();
    BufReader::new(&stream)
        .read_line(&mut ready)
        .expect("read ready");
    assert_eq!(ready, "{\"id\":\"proof\",\"kind\":\"ready\"}\n");
    child.0.kill().expect("stop private sentinel");
    child.0.wait().expect("reap private sentinel");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn private_sentinel_exits_after_its_bounded_idle_period() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("idle");
    std::fs::create_dir_all(&root).expect("create scratch root");
    let mut child = ChildGuard(
        std::process::Command::new(env!("CARGO_BIN_EXE_ctx"))
            .arg("__ctx-center")
            .env("CTX_CENTER_SOCKET", root.join("center.sock"))
            .env("CTX_CENTER_SPAWN_LOCK", root.join("center.lock"))
            .env("CTX_CENTER_RUNS_ROOT", &root)
            .env("CTX_CENTER_INDEX", root.join("index.sqlite3"))
            .env("CTX_CENTER_IDLE_MS", "100")
            .env("CTX_CENTER_SCAN_MS", "20")
            .spawn()
            .expect("spawn private sentinel"),
    );
    let status = await_exit(&mut child.0);
    assert!(status.success());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn held_driver_lock_survives_center_idle_period() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("held-lock");
    std::fs::create_dir_all(&root).expect("create scratch root");
    let ledger = write_running_ledger(&root);
    let lock_path = ctx_traits_io::run_control::driver_lock_path(&ledger);
    let lock =
        ctx_traits_io::file_lock::open_lock_file_no_follow(&lock_path).expect("open driver lock");
    ctx_traits_io::file_lock::lock_exclusive_blocking(&lock).expect("hold driver lock");
    let mut child = ChildGuard(
        std::process::Command::new(env!("CARGO_BIN_EXE_ctx"))
            .arg("__ctx-center")
            .env("CTX_CENTER_SOCKET", root.join("center.sock"))
            .env("CTX_CENTER_SPAWN_LOCK", root.join("center.lock"))
            .env("CTX_CENTER_RUNS_ROOT", &root)
            .env("CTX_CENTER_INDEX", root.join("index.sqlite3"))
            .env("CTX_CENTER_IDLE_MS", "100")
            .env("CTX_CENTER_SCAN_MS", "20")
            .spawn()
            .expect("spawn private sentinel"),
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(child.0.try_wait().expect("poll center").is_none());
    drop(lock);
    assert!(await_exit(&mut child.0).success());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn concurrent_ensure_calls_share_one_auto_spawned_center() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("ensure-concurrent");
    std::fs::create_dir_all(&root).expect("create scratch root");
    let _environment = CenterEnvironment::install(&root);
    let callers: Vec<_> = (0..2)
        .map(|_| std::thread::spawn(ctx_traits_io::center::ensure_connected))
        .collect();
    for caller in callers {
        let stream = caller
            .join()
            .expect("ensure caller thread")
            .expect("ensure connection");
        drop(stream);
    }
    let launches = std::fs::read_to_string(root.join("launches")).expect("read launch marker");
    assert_eq!(
        launches.lines().count(),
        1,
        "only the lock winner may launch"
    );
    await_socket_removal(&root.join("center.sock"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ensure_retries_eof_then_spawns_the_private_center() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("ensure-eof");
    std::fs::create_dir_all(&root).expect("create scratch root");
    let socket = root.join("center.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind eof listener");
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept hello");
        std::fs::remove_file(&socket).expect("remove eof socket");
        drop(stream);
    });
    let _environment = CenterEnvironment::install(&root);
    let stream = ctx_traits_io::center::ensure_connected().expect("retry and spawn center");
    drop(stream);
    server.join().expect("eof server");
    await_socket_removal(&root.join("center.sock"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn sigkill_center_leaves_held_driver_and_restart_reconstructs_it() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("sigkill-rebuild");
    std::fs::create_dir_all(&root).expect("create scratch root");
    let ledger = write_running_ledger(&root);
    let lock_path = ctx_traits_io::run_control::driver_lock_path(&ledger);
    let lock =
        ctx_traits_io::file_lock::open_lock_file_no_follow(&lock_path).expect("open driver lock");
    ctx_traits_io::file_lock::lock_exclusive_blocking(&lock).expect("hold driver lock");

    let socket = root.join("center.sock");
    let index = root.join("index.sqlite3");
    let mut first = spawn_sentinel(&root, &socket, &index, "100");
    let stream = await_socket(&root.join("center.sock"));
    drop(stream);
    first.0.kill().expect("SIGKILL center");
    assert!(!first.0.wait().expect("reap SIGKILL center").success());
    std::fs::remove_file(&index).expect("delete disposable index before restart");

    let _environment = CenterEnvironment::install(&root);
    let stream = ctx_traits_io::center::ensure_connected().expect("restart through arbitration");
    drop(stream);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        root.join("center.sock").exists(),
        "restarted center lost the still-held driver lock"
    );
    drop(lock);
    await_socket_removal(&root.join("center.sock"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn independent_socket_versions_share_the_disposable_sqlite_index() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("shared-index");
    std::fs::create_dir_all(&root).expect("create scratch root");
    let index = root.join("index.sqlite3");
    let first_socket = root.join("center-v1.sock");
    let second_socket = root.join("center-v2.sock");
    let mut first = spawn_sentinel(&root, &first_socket, &index, "5000");
    let mut second = spawn_sentinel(&root, &second_socket, &index, "5000");
    drop(await_socket(&first_socket));
    drop(await_socket(&second_socket));
    assert!(
        index.exists(),
        "both centers must use the same derived index"
    );
    first.0.kill().expect("stop first center");
    second.0.kill().expect("stop second center");
    first.0.wait().expect("reap first center");
    second.0.wait().expect("reap second center");
    let _ = std::fs::remove_dir_all(root);
}
