//! Process-boundary smoke proof for the private center sentinel.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
    let _serial = SENTINEL_TEST_LOCK.lock().expect("lock sentinel proofs");
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
    let _serial = SENTINEL_TEST_LOCK.lock().expect("lock sentinel proofs");
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
