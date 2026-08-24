//! Process-boundary smoke proof for the private center sentinel.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    // AF_UNIX paths have a small kernel limit; use /tmp instead of the
    // platform's potentially deep temporary directory.
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "ctx-center-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("create scratch root");
    let socket = root.join("center.sock");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_ctx"))
        .arg("__ctx-center")
        .env("CTX_CENTER_SOCKET", &socket)
        .env("CTX_CENTER_SPAWN_LOCK", root.join("center.lock"))
        .env("CTX_CENTER_RUNS_ROOT", &root)
        .env("CTX_CENTER_INDEX", root.join("index.sqlite3"))
        .spawn()
        .expect("spawn private sentinel");
    let mut stream = (0..100)
        .find_map(|_| match UnixStream::connect(&socket) {
            Ok(stream) => Some(stream),
            Err(_) => {
                std::thread::sleep(Duration::from_millis(20));
                None
            }
        })
        .expect("center listener became ready");
    stream
        .write_all(b"{\"kind\":\"hello\",\"id\":\"proof\"}\n")
        .expect("write hello");
    let mut ready = String::new();
    BufReader::new(&stream)
        .read_line(&mut ready)
        .expect("read ready");
    assert_eq!(ready, "{\"id\":\"proof\",\"kind\":\"ready\"}\n");
    child.kill().expect("stop private sentinel");
    child.wait().expect("reap private sentinel");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn private_sentinel_exits_after_its_bounded_idle_period() {
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "ctx-idle-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("create scratch root");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_ctx"))
        .arg("__ctx-center")
        .env("CTX_CENTER_SOCKET", root.join("center.sock"))
        .env("CTX_CENTER_SPAWN_LOCK", root.join("center.lock"))
        .env("CTX_CENTER_RUNS_ROOT", &root)
        .env("CTX_CENTER_INDEX", root.join("index.sqlite3"))
        .env("CTX_CENTER_IDLE_MS", "100")
        .env("CTX_CENTER_SCAN_MS", "20")
        .spawn()
        .expect("spawn private sentinel");
    let status = (0..100)
        .find_map(|_| match child.try_wait().expect("poll child") {
            Some(status) => Some(status),
            None => {
                std::thread::sleep(Duration::from_millis(20));
                None
            }
        })
        .expect("idle center exited");
    assert!(status.success());
    let _ = std::fs::remove_dir_all(root);
}
