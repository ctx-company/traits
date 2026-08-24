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
    let root = std::env::temp_dir().join(format!(
        "ctx-center-proof-{}-{}",
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
