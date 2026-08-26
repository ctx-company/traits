//! Process-boundary smoke proof for the private center sentinel.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use camino::Utf8PathBuf;
use support::{controlled_command, git_init, require_success};

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
    names: [&'static str; 10],
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
            "CTX_CENTER_FRAME_MARKER",
            "CTX_CENTER_REAL_EXE",
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
            std::env::set_var("CTX_CENTER_FRAME_MARKER", root.join("frames"));
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
            "task-key": "center-task",
            "task-digest": "sha256:center-task-digest",
            "session-title": {"state": "resolved", "attempts": 1, "title": "Center proof title"},
            "worktree": {
                "id": "center-proof-worktree",
                "branch": "ctx/run/center-proof-run",
                "path": "/tmp/center-proof-worktree",
            },
            "merge-frames": [{
                "stage": "landing",
                "status": "merged",
                "evidence": ["center-proof-commit"],
            }],
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

fn write_completed_ledger(ledger: &Utf8PathBuf) {
    let session = serde_json::from_value(serde_json::json!({
        "schema-version": "0.1.0",
        "session-id": "center-proof-session",
        "run-id": "center-proof-run",
        "trait-id": "center-proof-trait",
        "current-run-index": 0,
        "status": "completed",
        "provenance": {
            "started-by": {"surface": "test", "caller": "proof-center"},
            "state-source": "test",
            "started-at-epoch": 1000,
        },
        "ledger": {
            "run-id": "center-proof-run",
            "trait-id": "center-proof-trait",
            "current-run-index": 0,
            "final-state": "completed",
        },
        "state-digest": "sha256:center-proof-completed",
    }))
    .expect("completed fixture session");
    ctx_traits_io::run_session::write_run_session(ledger, &session)
        .expect("write completed ledger");
}

const DRIVE_PROOF_TRAIT: &str = r#"id = "center-drive-proof"
schema-version = "0.4"
version = "0.1.0"
name = "Center drive proof"
description = "One accepted frame through the real driver."

[[agent]]
id = "worker"
description = "Fixture worker."
summary = "Fixture worker."

[[slot]]
id = "answer"
schema = "schema:boolean"
description = "Fixture output."

[procedure]
description = "One fixture frame."

[[procedure.sequence]]
id = "frame"
title = "Accept one frame"
agent = "agent:worker"
prompt = "Return true."
output = ["slot:answer"]
"#;

fn write_drive_fixture(repo: &std::path::Path, script: &std::path::Path) {
    std::fs::create_dir_all(repo.join(".ctx/traits/center-drive-proof/generated"))
        .expect("create fixture directories");
    git_init(repo);
    std::fs::write(repo.join(".gitignore"), "ctx.toml\n.ctx/runs/\n").expect("write gitignore");
    std::fs::write(
        repo.join(".ctx/traits/runtime.toml"),
        format!(
            r#"schema-version = "0.4"

[harness.fixture]
kind = "custom"
bin = "{}"
transports = ["cli"]
version-probe = ["--fixture-probe"]

[harness.fixture.cli]
argv = []
prompt-via = "stdin"
output = "claude-stream-json"

[agent.role.worker]
harness = "fixture"
transport = "cli"
"#,
            script.display()
        ),
    )
    .expect("write fixture runtime");
    std::fs::write(
        repo.join(".ctx/traits/center-drive-proof/trait.toml"),
        "[package]\nid = \"center-drive-proof\"\nversion = \"0.1.0\"\nname = \"Center drive proof\"\nstatus = \"draft\"\n",
    )
    .expect("write fixture manifest");
    std::fs::write(
        repo.join(".ctx/traits/center-drive-proof/generated/index.toml"),
        DRIVE_PROOF_TRAIT,
    )
    .expect("write fixture trait");
}

fn write_fixture_harness(path: &std::path::Path) {
    std::fs::write(
        path,
        r#"#!/bin/sh
if [ "$1" = "--fixture-probe" ]; then
  printf 'fixture-1.0\n'
  exit 0
fi
cat >/dev/null
# Leave the real driver's registration refresh observable before the accepted
# frame is released from this fixture harness.
sleep 1
printf '%s\n' '{"type":"result","session_id":"center-drive-proof","result":"{\"answer\":true}"}'
"#,
    )
    .expect("write fixture harness");
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)
        .expect("stat fixture harness")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("make fixture harness executable");
}

struct DriveEvidence {
    status: std::process::ExitStatus,
    report: serde_json::Value,
    ledger: Vec<u8>,
    activity: Vec<u8>,
}

fn normalize_durable_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            // Each independent fixture drive necessarily has a new identity,
            // source path, timestamp, and elapsed harness duration. Strip only
            // those run-local fields before comparing durable content.
            object.retain(|key, _| {
                !matches!(
                    key.as_str(),
                    "at-epoch-ms"
                        | "at_epoch_ms"
                        | "recorded-at-epoch"
                        | "started-at-epoch"
                        | "approved-at"
                        | "run-id"
                        | "session-id"
                        | "canonical-digest"
                        | "final-session-digest"
                        | "source-digest"
                        | "state-digest"
                        | "value-digest"
                        | "duration-ms"
                        | "elapsed-seconds"
                        | "path"
                        | "repository-root"
                        | "bin"
                )
            });
            for value in object.values_mut() {
                normalize_durable_json(value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                normalize_durable_json(value);
            }
        }
        serde_json::Value::String(text) => {
            if text.starts_with("/tmp/") {
                *text = "{path}".to_string();
            }
            if let Some(start) = text.find("argv=/tmp/") {
                text.replace_range(start.., "argv={path}");
            }
            if let Some(start) = text.find("duration-ms=") {
                let digits = text[start + "duration-ms=".len()..]
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .count();
                text.replace_range(
                    start + "duration-ms=".len()..start + "duration-ms=".len() + digits,
                    "{elapsed}",
                );
            }
        }
        _ => {}
    }
}

fn assert_same_durable_evidence(baseline: &DriveEvidence, candidate: &DriveEvidence) {
    assert_eq!(candidate.status.code(), baseline.status.code());
    let mut baseline_report = baseline.report.clone();
    let mut candidate_report = candidate.report.clone();
    normalize_durable_json(&mut baseline_report);
    normalize_durable_json(&mut candidate_report);
    assert_eq!(candidate_report, baseline_report, "drive report changed");
    let mut baseline_ledger: serde_json::Value =
        serde_json::from_slice(&baseline.ledger).expect("decode baseline ledger bytes");
    let mut candidate_ledger: serde_json::Value =
        serde_json::from_slice(&candidate.ledger).expect("decode candidate ledger bytes");
    normalize_durable_json(&mut baseline_ledger);
    normalize_durable_json(&mut candidate_ledger);
    assert_eq!(
        candidate_ledger, baseline_ledger,
        "ledger bytes changed beyond timestamps"
    );
    let mut baseline_activity: Vec<serde_json::Value> = baseline
        .activity
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).expect("decode baseline activity line"))
        .collect();
    let mut candidate_activity: Vec<serde_json::Value> = candidate
        .activity
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).expect("decode candidate activity line"))
        .collect();
    for value in &mut baseline_activity {
        normalize_durable_json(value);
    }
    for value in &mut candidate_activity {
        normalize_durable_json(value);
    }
    assert_eq!(
        candidate_activity, baseline_activity,
        "activity bytes changed beyond timestamps"
    );
}

fn run_drive_with_notification_environment(
    root: &std::path::Path,
    configure: impl FnOnce(&mut std::process::Command),
) -> DriveEvidence {
    let home = root.join("home");
    let repo = root.join("repository");
    std::fs::create_dir_all(&repo).expect("create fixture repository");
    let harness = root.join("fixture-harness.sh");
    write_fixture_harness(&harness);
    write_drive_fixture(&repo, &harness);
    let fixture = ".ctx/traits/center-drive-proof/generated/index.toml";
    require_success("fixture init", &["traits", "init"], &repo, &home);
    require_success(
        "fixture review",
        &[
            "traits",
            "internal",
            "review",
            "--file",
            fixture,
            "--approve",
        ],
        &repo,
        &home,
    );
    require_success(
        "fixture activate",
        &["traits", "state", "--active", "--file", fixture],
        &repo,
        &home,
    );
    let ledger = repo.join(".ctx/runs/center-drive-proof.json");
    let ledger_text = ledger.to_string_lossy().into_owned();
    let mut command = controlled_command(
        std::path::Path::new(env!("CARGO_BIN_EXE_ctx")),
        &[
            "traits",
            "run",
            "--file",
            fixture,
            "--out",
            &ledger_text,
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    );
    configure(&mut command);
    let output = command.output().expect("run fixture drive");
    let activity = ctx_traits_io::activity_sidecar::activity_path(
        &Utf8PathBuf::from_path_buf(ledger.clone()).expect("UTF-8 ledger"),
    );
    DriveEvidence {
        status: output.status,
        report: serde_json::from_slice(&output.stdout).expect("decode drive report"),
        ledger: std::fs::read(&ledger).expect("read ledger bytes"),
        activity: std::fs::read(activity.as_std_path()).expect("read activity bytes"),
    }
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

/// A minimal, protocol-aware peer used to put the real driver notification
/// seam under a durability checkpoint. It deliberately acknowledges only
/// after observing the authoritative evidence the notification represents.
fn spawn_durability_checkpoint_peer(
    socket: std::path::PathBuf,
    ledger: Utf8PathBuf,
    expect_no_ended: bool,
) -> mpsc::Receiver<Result<(), String>> {
    let listener = UnixListener::bind(&socket).expect("bind durability checkpoint socket");
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let deadline = Instant::now() + PROCESS_DEADLINE;
        let mut saw_activity = false;
        let mut saw_frame = false;
        loop {
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if Instant::now() < deadline => {
                        let _ = error;
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => {
                        let _ = sender.send(Err(format!("accept checkpoint peer: {error}")));
                        return;
                    }
                }
            };
            let reader_stream = match stream.try_clone() {
                Ok(stream) => stream,
                Err(error) => {
                    let _ = sender.send(Err(format!("clone checkpoint peer: {error}")));
                    return;
                }
            };
            let mut reader = BufReader::new(reader_stream);
            let mut saw_ended = false;
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        if saw_ended {
                            let _ = sender.send(Ok(()));
                            return;
                        }
                        break;
                    }
                    Ok(_) => {}
                    Err(error)
                        if saw_ended
                            && (matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) || error.raw_os_error() == Some(35)) =>
                    {
                        let _ = sender.send(Ok(()));
                        return;
                    }
                    Err(error) => {
                        let _ = sender.send(Err(format!("read checkpoint peer: {error}")));
                        return;
                    }
                }
                let request: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(request) => request,
                    Err(error) => {
                        let _ = sender.send(Err(format!("decode checkpoint request: {error}")));
                        return;
                    }
                };
                let kind = request["kind"].as_str().unwrap_or_default();
                let id = request["id"].as_str().unwrap_or_default();
                let reply = match kind {
                    "hello" => serde_json::json!({"kind":"ready","id":id}),
                    "activity-line" => {
                        let activity = ctx_traits_io::activity_sidecar::activity_path(&ledger);
                        if std::fs::metadata(activity.as_std_path())
                            .map(|metadata| metadata.len() > 0)
                            .unwrap_or(false)
                        {
                            saw_activity = true;
                            serde_json::json!({"kind":"response","id":id,"result":{"type":"ok"}})
                        } else {
                            let _ = sender.send(Err(
                                "activity notification preceded its flushed sidecar line"
                                    .to_string(),
                            ));
                            return;
                        }
                    }
                    "frame-done" => {
                        let session = match ctx_traits_io::run_session::read_run_session(&ledger) {
                            Ok(session) => session,
                            Err(error) => {
                                let _ = sender.send(Err(format!(
                                    "frame notification could not read ledger: {error}"
                                )));
                                return;
                            }
                        };
                        if session.accepted_slot_values.is_empty() {
                            let _ = sender.send(Err(
                                "frame notification preceded accepted-frame ledger write"
                                    .to_string(),
                            ));
                            return;
                        }
                        saw_frame = true;
                        serde_json::json!({"kind":"response","id":id,"result":{"type":"ok"}})
                    }
                    "ended" => {
                        if saw_ended {
                            let _ =
                                sender
                                    .send(Err("driver emitted more than one ended notification"
                                        .to_string()));
                            return;
                        }
                        if expect_no_ended {
                            let _ = sender.send(Err(
                                "ended notification was emitted after a failed outcome write"
                                    .to_string(),
                            ));
                            return;
                        }
                        let session = match ctx_traits_io::run_session::read_run_session(&ledger) {
                            Ok(session) => session,
                            Err(error) => {
                                let _ = sender.send(Err(format!(
                                    "ended notification could not read ledger: {error}"
                                )));
                                return;
                            }
                        };
                        if !saw_activity || !saw_frame || session.last_drive_outcome.is_none() {
                            let _ =
                                sender
                                    .send(Err("ended notification preceded durable drive outcome"
                                        .to_string()));
                            return;
                        }
                        stream
                            .set_read_timeout(Some(Duration::from_millis(200)))
                            .expect("bound terminal checkpoint read");
                        saw_ended = true;
                        serde_json::json!({"kind":"response","id":id,"result":{"type":"ok"}})
                    }
                    _ => serde_json::json!({"kind":"response","id":id,"result":{"type":"ok"}}),
                };
                if writeln!(stream, "{reply}").is_err() || stream.flush().is_err() {
                    let _ = sender.send(Err("write checkpoint acknowledgement".to_string()));
                    return;
                }
            }
            if expect_no_ended && saw_frame {
                let _ = sender.send(Ok(()));
                return;
            }
        }
    });
    receiver
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
    assert_eq!(ready, "{\"kind\":\"ready\",\"id\":\"proof\"}\n");
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
fn private_sentinel_with_only_a_corrupt_ledger_still_idle_exits() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("corrupt-idle");
    let ledger = root.join("repository/session-corrupt.json");
    std::fs::create_dir_all(ledger.parent().expect("corrupt ledger parent"))
        .expect("create corrupt ledger parent");
    std::fs::write(&ledger, "not valid json").expect("write corrupt ledger");

    let mut child = spawn_sentinel(
        &root,
        &root.join("center.sock"),
        &root.join("index.sqlite3"),
        "100",
    );
    let status = await_exit(&mut child.0);
    assert!(status.success(), "corrupt ledger must not keep center busy");
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
    let before = std::fs::read(ledger.as_std_path()).expect("read authoritative ledger");
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
    assert_eq!(
        std::fs::read(ledger.as_std_path()).expect("read ledger after center scan"),
        before,
        "startup reconciliation must not classify a held waiting ledger as an orphan"
    );
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
fn driver_start_returns_without_waiting_for_center_readiness() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("driver-start");
    std::fs::create_dir_all(&root).expect("create scratch root");
    let _environment = CenterEnvironment::install(&root);
    let started = Instant::now();
    ctx_traits_io::center::start_for_driver();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "driver startup waited for the listener"
    );
    drop(await_socket(&root.join("center.sock")));
    let launches = std::fs::read_to_string(root.join("launches")).expect("read launch marker");
    assert_eq!(launches.lines().count(), 1);
    await_socket_removal(&root.join("center.sock"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn driver_start_returns_before_a_delayed_center_binds() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("driver-start-delayed-bind");
    std::fs::create_dir_all(&root).expect("create scratch root");
    let _environment = CenterEnvironment::install(&root);
    let delayed = root.join("delayed-center.sh");
    std::fs::write(
        &delayed,
        "#!/bin/sh\nsleep 1\nexec \"$CTX_CENTER_REAL_EXE\" \"$@\"\n",
    )
    .expect("write delayed center wrapper");
    let mut permissions = std::fs::metadata(&delayed)
        .expect("read delayed center wrapper")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&delayed, permissions)
        .expect("make delayed center wrapper executable");
    unsafe {
        std::env::set_var("CTX_CENTER_REAL_EXE", env!("CARGO_BIN_EXE_ctx"));
        std::env::set_var("CTX_CENTER_EXECUTABLE", &delayed);
        std::env::set_var("CTX_CENTER_IDLE_MS", "1000");
    }

    let started = Instant::now();
    ctx_traits_io::center::start_for_driver();
    assert!(
        started.elapsed() < Duration::from_millis(300),
        "driver startup waited for the deliberately delayed listener"
    );
    drop(await_socket(&root.join("center.sock")));
    let launches = std::fs::read_to_string(root.join("launches")).expect("read launch marker");
    assert_eq!(
        launches.lines().count(),
        1,
        "delayed bind admitted a duplicate launch"
    );
    await_socket_removal(&root.join("center.sock"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn driver_and_query_starters_share_one_launch_lease() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("mixed-start");
    std::fs::create_dir_all(&root).expect("create scratch root");
    let _environment = CenterEnvironment::install(&root);
    let driver = std::thread::spawn(ctx_traits_io::center::start_for_driver);
    let query = std::thread::spawn(ctx_traits_io::center::ensure_connected);
    driver.join().expect("driver startup thread");
    drop(
        query
            .join()
            .expect("query startup thread")
            .expect("query connection"),
    );
    let launches = std::fs::read_to_string(root.join("launches")).expect("read launch marker");
    assert_eq!(launches.lines().count(), 1, "starters raced to launch");
    await_socket_removal(&root.join("center.sock"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn driver_start_preserves_a_listener_with_a_delayed_valid_handshake() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("driver-delayed-handshake");
    std::fs::create_dir_all(&root).expect("create scratch root");
    let socket = root.join("center.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind delayed listener");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept hello");
        let mut hello = String::new();
        BufReader::new(stream.try_clone().expect("clone delayed stream"))
            .read_line(&mut hello)
            .expect("read hello");
        let id = serde_json::from_str::<serde_json::Value>(&hello)
            .expect("decode hello")
            .get("id")
            .and_then(serde_json::Value::as_str)
            .expect("hello id")
            .to_string();
        std::thread::sleep(Duration::from_millis(100));
        // Driver startup deliberately returns after its short handshake
        // deadline, so closing before this valid reply is expected.
        let _ = stream.write_all(format!("{{\"kind\":\"ready\",\"id\":\"{id}\"}}\n").as_bytes());
    });
    let _environment = CenterEnvironment::install(&root);
    ctx_traits_io::center::start_for_driver();
    server.join().expect("delayed handshake server");
    assert!(
        !root.join("launches").exists(),
        "an inconclusive but valid listener must not be replaced"
    );
    std::fs::remove_file(&socket).expect("remove delayed listener socket");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn driver_marker_publication_failure_aborts_the_unmarked_child() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("driver-marker-write-failure");
    std::fs::create_dir_all(&root).expect("create scratch root");
    let _environment = CenterEnvironment::install(&root);
    let delayed = root.join("delayed-center.sh");
    std::fs::write(
        &delayed,
        "#!/bin/sh\nsleep 1\nexec \"$CTX_CENTER_REAL_EXE\" \"$@\"\n",
    )
    .expect("write delayed center wrapper");
    let mut permissions = std::fs::metadata(&delayed)
        .expect("read delayed wrapper")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&delayed, permissions).expect("make delayed wrapper executable");
    std::fs::create_dir(root.join("center.spawn")).expect("block marker publication");
    unsafe {
        std::env::set_var("CTX_CENTER_REAL_EXE", env!("CARGO_BIN_EXE_ctx"));
        std::env::set_var("CTX_CENTER_EXECUTABLE", &delayed);
    }
    ctx_traits_io::center::start_for_driver();
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        !root.join("center.sock").exists(),
        "an unmarked child must be aborted before it can bind"
    );
    std::fs::remove_dir(root.join("center.spawn")).expect("unblock marker path");
    drop(ctx_traits_io::center::ensure_connected().expect("launch replacement center"));
    let launches = std::fs::read_to_string(root.join("launches")).expect("read launch marker");
    assert_eq!(
        launches.lines().count(),
        1,
        "only the replacement may launch"
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
fn ensure_recovers_an_accepting_non_center_listener() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("ensure-malformed-listener");
    std::fs::create_dir_all(&root).expect("create scratch root");
    let socket = root.join("center.sock");
    let listener =
        std::os::unix::net::UnixListener::bind(&socket).expect("bind malformed listener");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept hello");
        let mut hello = String::new();
        BufReader::new(stream.try_clone().expect("clone malformed stream"))
            .read_line(&mut hello)
            .expect("read hello");
        stream
            .write_all(b"{\"kind\":\"not-ready\",\"id\":\"wrong\"}\n")
            .expect("write malformed ready");
    });
    let _environment = CenterEnvironment::install(&root);
    let stream = ctx_traits_io::center::ensure_connected().expect("replace malformed listener");
    drop(stream);
    server.join().expect("malformed server");
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
    let mut activity = ctx_traits_io::activity_sidecar::ActivitySidecarWriter::open(&ledger);
    activity.append_session_title("durable before crash".to_string());
    let lock_path = ctx_traits_io::run_control::driver_lock_path(&ledger);
    let lock =
        ctx_traits_io::file_lock::open_lock_file_no_follow(&lock_path).expect("open driver lock");
    ctx_traits_io::file_lock::lock_exclusive_blocking(&lock).expect("hold driver lock");

    let socket = root.join("center.sock");
    let index = root.join("index.sqlite3");
    let mut first = spawn_sentinel(&root, &socket, &index, "100");
    let stream = await_socket(&root.join("center.sock"));
    drop(stream);
    let ledger_bytes = std::fs::read(ledger.as_std_path()).expect("capture ledger bytes");
    let activity_path = ctx_traits_io::activity_sidecar::activity_path(&ledger);
    let activity_bytes =
        std::fs::read(activity_path.as_std_path()).expect("capture activity bytes");
    first.0.kill().expect("SIGKILL center");
    assert!(!first.0.wait().expect("reap SIGKILL center").success());
    assert_eq!(
        std::fs::read(ledger.as_std_path()).expect("read ledger after crash"),
        ledger_bytes
    );
    assert_eq!(
        std::fs::read(activity_path.as_std_path()).expect("read activity after crash"),
        activity_bytes
    );
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
    write_running_ledger(&root);
    let mut first = spawn_sentinel(&root, &first_socket, &index, "5000");
    drop(await_socket(&first_socket));
    assert!(
        index.exists(),
        "the first center must write the shared derived index"
    );
    first.0.kill().expect("stop first center");
    first.0.wait().expect("reap first center");

    // Simulate the preceding v1 projection. It had flattened worktree facts
    // and none of the current nested or task/title/terminal additions. The next
    // process reprojects it from cached sessions without changing the legacy
    // marker that an installed v1 center still requires.
    let db = rusqlite::Connection::open(&index).expect("open first center index");
    let summary: String = db
        .query_row("SELECT summary FROM center_rows LIMIT 1", [], |row| {
            row.get(0)
        })
        .expect("read persisted summary");
    let mut preceding: serde_json::Value =
        serde_json::from_str(&summary).expect("decode persisted summary");
    let object = preceding
        .as_object_mut()
        .expect("persisted summary is an object");
    object.remove("task_key");
    object.remove("task_digest");
    object.remove("title");
    object.remove("worktree");
    object.remove("last_terminal_merge_frame");
    db.execute(
        "UPDATE center_rows SET summary = ?1",
        [serde_json::to_string(&preceding).expect("encode preceding summary")],
    )
    .expect("write preceding summary shape");
    db.execute("DELETE FROM center_projection_meta", [])
        .expect("remove marker absent from the preceding center");
    drop(db);

    let mut second = spawn_sentinel(&root, &second_socket, &index, "5000");
    drop(await_socket(&second_socket));
    std::thread::sleep(Duration::from_millis(100));
    let db = rusqlite::Connection::open(&index).expect("open shared index metadata");
    let version: i64 = db
        .query_row("SELECT version FROM center_projection_meta", [], |row| {
            row.get(0)
        })
        .expect("read shared index metadata");
    assert_eq!(
        version, 2,
        "the current center must mark the widened projection"
    );
    let legacy_version: i64 = db
        .query_row("SELECT version FROM center_meta", [], |row| row.get(0))
        .expect("read v1-compatible index marker");
    assert_eq!(
        legacy_version, 1,
        "the shared marker remains readable by an installed v1 center"
    );
    let summary: String = db
        .query_row("SELECT summary FROM center_rows LIMIT 1", [], |row| {
            row.get(0)
        })
        .expect("read reprojected summary");
    let summary: serde_json::Value =
        serde_json::from_str(&summary).expect("decode reprojected summary");
    assert_eq!(summary["task_key"], "center-task");
    assert_eq!(summary["task_digest"], "sha256:center-task-digest");
    assert_eq!(summary["title"], "Center proof title");
    assert_eq!(summary["worktree"]["branch"], "ctx/run/center-proof-run");
    assert_eq!(summary["worktree_id"], "center-proof-worktree");
    assert_eq!(summary["worktree_branch"], "ctx/run/center-proof-run");
    assert_eq!(summary["worktree_path"], "/tmp/center-proof-worktree");
    assert_eq!(summary["last_terminal_merge_frame"]["status"], "merged");
    drop(db);
    second.0.kill().expect("stop second center");
    second.0.wait().expect("reap second center");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn warm_center_queries_discover_an_unnotified_ledger_immediately() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("warm-unnotified-query");
    std::fs::create_dir_all(&root).expect("create scratch root");
    let socket = root.join("center.sock");
    let index = root.join("index.sqlite3");
    let _environment = CenterEnvironment::install(&root);
    let mut child = spawn_sentinel(&root, &socket, &index, "5000");
    drop(await_socket(&socket));

    assert!(
        ctx_traits_io::center::list(None)
            .expect("empty warm snapshot")
            .is_empty()
    );
    let ledger = write_running_ledger(&root);

    let rows = ctx_traits_io::center::list(None).expect("fresh list snapshot");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].ledger_path, ledger);
    assert_eq!(
        ctx_traits_io::center::stats(None, None, None)
            .expect("fresh stats snapshot")
            .total_runs,
        1
    );

    child.0.kill().expect("stop center");
    child.0.wait().expect("reap center");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn one_driver_frame_notification_reaches_two_subscribers() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("two-subscribers");
    std::fs::create_dir_all(&root).expect("create scratch root");
    let ledger = write_running_ledger(&root);
    let socket = root.join("center.sock");
    let index = root.join("index.sqlite3");
    let _environment = CenterEnvironment::install(&root);
    let mut child = spawn_sentinel(&root, &socket, &index, "5000");
    drop(await_socket(&socket));

    let first = ctx_traits_io::center::subscribe(None).expect("first subscription");
    let second = ctx_traits_io::center::subscribe(None).expect("second subscription");
    for subscription in [&first, &second] {
        assert!(matches!(
            subscription.recv_timeout(PROCESS_DEADLINE),
            Ok(ctx_traits_io::center::CenterEvent::SnapshotStart)
        ));
        assert!(matches!(
            subscription.recv_timeout(PROCESS_DEADLINE),
            Ok(ctx_traits_io::center::CenterEvent::SnapshotRow(_))
        ));
        assert!(matches!(
            subscription.recv_timeout(PROCESS_DEADLINE),
            Ok(ctx_traits_io::center::CenterEvent::SnapshotEnd)
        ));
    }

    let driver_lock = ctx_traits_io::run_control::try_acquire(
        &ctx_traits_io::run_liveness::LiveRunFacts {
            session_id: "center-proof-session".to_string(),
            run_id: "center-proof-run".to_string(),
            repo_key: "repository".to_string(),
            repo_path: root.display().to_string(),
            ledger_path: ledger.clone(),
            worktree_path: None,
            branch: None,
            log_path: None,
        },
        std::sync::Arc::new(|| {}),
    )
    .expect("acquire driver lock")
    .expect("test owns driver lock");
    let notifier =
        ctx_traits_io::center::DriverNotifier::new(ctx_traits_io::center::DriverRegistration {
            ledger_path: ledger.to_string(),
            holder: driver_lock.holder().clone(),
        });
    // The frame notification follows the authoritative atomic ledger rewrite.
    write_completed_ledger(&ledger);
    notifier.frame_done();
    for subscription in [&first, &second] {
        let deadline = Instant::now() + PROCESS_DEADLINE;
        let completed = loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "completed frame delta was not delivered"
            );
            match subscription.recv_timeout(remaining) {
                Ok(ctx_traits_io::center::CenterEvent::Delta(
                    ctx_traits_io::center::CenterDelta::RowChanged { row },
                )) if row.summary.status
                    == ctx_traits_core::procedure::session::Status::Completed =>
                {
                    break row;
                }
                Ok(_) => {}
                Err(error) => panic!("frame delta: {error}"),
            }
        };
        assert_eq!(
            completed.summary.status,
            ctx_traits_core::procedure::session::Status::Completed
        );
    }
    let frame_marker = root.join("frames");
    let deadline = Instant::now() + PROCESS_DEADLINE;
    while !frame_marker.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let frames = std::fs::read_to_string(&frame_marker).expect("read frame-notification marker");
    assert_eq!(
        frames.lines().count(),
        1,
        "one driver frame event was accepted by the center"
    );
    drop(notifier);
    drop(driver_lock);
    drop(first);
    drop(second);
    child.0.kill().expect("stop private sentinel");
    child.0.wait().expect("reap private sentinel");
    let _ = std::fs::remove_dir_all(root);
}

/// Client requests must be served from the center's accepted model rather than
/// reopening a ledger after discovery. Removing read permission after the
/// initial snapshot makes an accidental client-side read fail deterministically
/// while preserving the center's `(mtime, size)` fingerprint.
#[test]
fn cached_center_queries_do_not_reopen_a_discovered_ledger() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("cached-queries-no-ledger-read");
    std::fs::create_dir_all(&root).expect("create scratch root");
    let ledger = write_running_ledger(&root);
    let socket = root.join("center.sock");
    let index = root.join("index.sqlite3");
    let _environment = CenterEnvironment::install(&root);
    let mut child = spawn_sentinel(&root, &socket, &index, "5000");
    drop(await_socket(&socket));

    let rows = ctx_traits_io::center::list(None).expect("initial center snapshot");
    assert_eq!(rows.len(), 1);
    let stats = ctx_traits_io::center::stats(None, None, None).expect("initial stats");
    assert_eq!(stats.total_runs, 1);

    let original_mode = std::fs::metadata(ledger.as_std_path())
        .expect("ledger metadata")
        .permissions()
        .mode();
    std::fs::set_permissions(ledger.as_std_path(), std::fs::Permissions::from_mode(0o000))
        .expect("deny ledger reads after center snapshot");

    let cached_rows = ctx_traits_io::center::list(None).expect("cached list");
    assert_eq!(cached_rows, rows);
    let cached_stats = ctx_traits_io::center::stats(None, None, None).expect("cached stats");
    assert_eq!(cached_stats.total_runs, stats.total_runs);
    let by_run =
        ctx_traits_io::center::find_by_run_id("center-proof-run", None).expect("cached run lookup");
    assert_eq!(by_run.len(), 1);
    assert!(matches!(
        ctx_traits_io::center::get("center-proof-session", None).expect("cached session lookup"),
        ctx_traits_io::center::GetResult::Session(_)
    ));

    std::fs::set_permissions(
        ledger.as_std_path(),
        std::fs::Permissions::from_mode(original_mode),
    )
    .expect("restore ledger permissions");
    child.0.kill().expect("stop private sentinel");
    child.0.wait().expect("reap private sentinel");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn real_accepted_drive_frame_notifies_two_subscribers_once() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("real-drive-two-subscribers");
    let home = root.join("home");
    let repo = root.join("repository");
    std::fs::create_dir_all(&repo).expect("create fixture repository");
    let harness = root.join("fixture-harness.sh");
    write_fixture_harness(&harness);
    write_drive_fixture(&repo, &harness);
    let fixture = ".ctx/traits/center-drive-proof/generated/index.toml";
    require_success("fixture init", &["traits", "init"], &repo, &home);
    require_success(
        "fixture review",
        &[
            "traits",
            "internal",
            "review",
            "--file",
            fixture,
            "--approve",
        ],
        &repo,
        &home,
    );
    require_success(
        "fixture activate",
        &["traits", "state", "--active", "--file", fixture],
        &repo,
        &home,
    );

    let _environment = CenterEnvironment::install(&root);
    let socket = root.join("center.sock");
    let mut child = spawn_sentinel(&root, &socket, &root.join("index.sqlite3"), "5000");
    drop(await_socket(&socket));
    let first = ctx_traits_io::center::subscribe(None).expect("first subscription");
    let second = ctx_traits_io::center::subscribe(None).expect("second subscription");
    for subscription in [&first, &second] {
        assert!(matches!(
            subscription.recv_timeout(PROCESS_DEADLINE),
            Ok(ctx_traits_io::center::CenterEvent::SnapshotStart)
        ));
        assert!(matches!(
            subscription.recv_timeout(PROCESS_DEADLINE),
            Ok(ctx_traits_io::center::CenterEvent::SnapshotEnd)
        ));
    }

    let ledger = repo.join(".ctx/runs/center-drive-proof.json");
    let ledger_text = ledger.to_string_lossy();
    let socket_text = socket.to_string_lossy();
    let lock_text = root.join("center.lock").to_string_lossy().into_owned();
    let root_text = root.to_string_lossy();
    let index_text = root.join("index.sqlite3").to_string_lossy().into_owned();
    let executable = env!("CARGO_BIN_EXE_ctx");
    let drive = controlled_command(
        std::path::Path::new(executable),
        &[
            "traits",
            "run",
            "--file",
            fixture,
            "--out",
            &ledger_text,
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    )
    .env("CTX_CENTER_SOCKET", socket_text.as_ref())
    .env("CTX_CENTER_SPAWN_LOCK", &lock_text)
    .env("CTX_CENTER_RUNS_ROOT", root_text.as_ref())
    .env("CTX_CENTER_INDEX", &index_text)
    .env("CTX_CENTER_EXECUTABLE", executable)
    .env("CTX_CENTER_IDLE_MS", "5000")
    .env("CTX_CENTER_SCAN_MS", "5000")
    .env("CTX_CENTER_LAUNCH_MARKER", root.join("launches"))
    .env("CTX_CENTER_FRAME_MARKER", root.join("frames"))
    .spawn()
    .expect("start fixture drive");
    for subscription in [&first, &second] {
        let deadline = Instant::now() + PROCESS_DEADLINE;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "real driver registration was not delivered"
            );
            match subscription.recv_timeout(remaining) {
                Ok(ctx_traits_io::center::CenterEvent::Delta(
                    ctx_traits_io::center::CenterDelta::Appeared { row }
                    | ctx_traits_io::center::CenterDelta::RowChanged { row },
                )) if row.ledger_path == ledger_text => break,
                Ok(_) => {}
                Err(error) => panic!("real driver registration delta: {error}"),
            }
        }
    }
    let output = drive.wait_with_output().expect("wait for fixture drive");
    assert!(
        output.status.success(),
        "fixture drive failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    for subscription in [&first, &second] {
        let deadline = Instant::now() + PROCESS_DEADLINE;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "real drive delta was not delivered");
            match subscription.recv_timeout(remaining) {
                Ok(ctx_traits_io::center::CenterEvent::Delta(
                    ctx_traits_io::center::CenterDelta::RowChanged { row }
                    | ctx_traits_io::center::CenterDelta::Ended { row },
                )) if row.ledger_path == ledger_text => break,
                Ok(_) => {}
                Err(error) => panic!("real drive delta: {error}"),
            }
        }
    }
    let frame_marker = root.join("frames");
    let deadline = Instant::now() + PROCESS_DEADLINE;
    while !frame_marker.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let frames = std::fs::read_to_string(frame_marker).expect("read frame marker");
    assert_eq!(
        frames.lines().count(),
        1,
        "one accepted frame reached center"
    );
    drop(first);
    drop(second);
    child.0.kill().expect("stop private sentinel");
    child.0.wait().expect("reap private sentinel");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn real_driver_notifications_follow_their_durable_write_checkpoints() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("drive-durability-checkpoints");
    let home = root.join("home");
    let repo = root.join("repository");
    std::fs::create_dir_all(&repo).expect("create fixture repository");
    let harness = root.join("fixture-harness.sh");
    write_fixture_harness(&harness);
    write_drive_fixture(&repo, &harness);
    let fixture = ".ctx/traits/center-drive-proof/generated/index.toml";
    require_success("fixture init", &["traits", "init"], &repo, &home);
    require_success(
        "fixture review",
        &[
            "traits",
            "internal",
            "review",
            "--file",
            fixture,
            "--approve",
        ],
        &repo,
        &home,
    );
    require_success(
        "fixture activate",
        &["traits", "state", "--active", "--file", fixture],
        &repo,
        &home,
    );

    let socket = root.join("center.sock");
    let ledger = Utf8PathBuf::from_path_buf(repo.join(".ctx/runs/center-drive-proof.json"))
        .expect("UTF-8 ledger");
    let checkpoint = spawn_durability_checkpoint_peer(socket.clone(), ledger.clone(), false);
    let output = controlled_command(
        std::path::Path::new(env!("CARGO_BIN_EXE_ctx")),
        &[
            "traits",
            "run",
            "--file",
            fixture,
            "--out",
            ledger.as_str(),
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    )
    .env("CTX_CENTER_SOCKET", &socket)
    .env("CTX_CENTER_SPAWN_LOCK", root.join("center.lock"))
    .env("CTX_CENTER_RUNS_ROOT", &root)
    .env("CTX_CENTER_INDEX", root.join("index.sqlite3"))
    .env("CTX_CENTER_EXECUTABLE", root.join("does-not-start"))
    .output()
    .expect("run fixture drive through checkpoint peer");
    assert!(
        output.status.success(),
        "fixture drive failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    match checkpoint.recv_timeout(PROCESS_DEADLINE) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("durability checkpoint failed: {error}"),
        Err(error) => {
            panic!("durability checkpoint did not observe terminal notification: {error}")
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn failed_outcome_write_emits_no_ended_notification() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("failed-outcome-no-ended");
    let home = root.join("home");
    let repo = root.join("repository");
    std::fs::create_dir_all(&repo).expect("create fixture repository");
    let harness = root.join("fixture-harness.sh");
    write_fixture_harness(&harness);
    write_drive_fixture(&repo, &harness);
    let fixture = ".ctx/traits/center-drive-proof/generated/index.toml";
    require_success("fixture init", &["traits", "init"], &repo, &home);
    require_success(
        "fixture review",
        &[
            "traits",
            "internal",
            "review",
            "--file",
            fixture,
            "--approve",
        ],
        &repo,
        &home,
    );
    require_success(
        "fixture activate",
        &["traits", "state", "--active", "--file", fixture],
        &repo,
        &home,
    );

    let socket = root.join("center.sock");
    let ledger = Utf8PathBuf::from_path_buf(repo.join(".ctx/runs/center-drive-proof.json"))
        .expect("UTF-8 ledger");
    let checkpoint = spawn_durability_checkpoint_peer(socket.clone(), ledger.clone(), true);
    let output = controlled_command(
        std::path::Path::new(env!("CARGO_BIN_EXE_ctx")),
        &[
            "traits",
            "run",
            "--file",
            fixture,
            "--out",
            ledger.as_str(),
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    )
    .env("CTX_CENTER_SOCKET", &socket)
    .env("CTX_CENTER_SPAWN_LOCK", root.join("center.lock"))
    .env("CTX_CENTER_RUNS_ROOT", &root)
    .env("CTX_CENTER_INDEX", root.join("index.sqlite3"))
    .env("CTX_CENTER_EXECUTABLE", root.join("does-not-start"))
    .env("CTX_INTERNAL_TESTHOOK_FAIL_DRIVE_OUTCOME_WRITE", "1")
    .output()
    .expect("run fixture drive with failed outcome write");
    assert!(
        output.status.success(),
        "outcome persistence remains best effort: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    match checkpoint.recv_timeout(PROCESS_DEADLINE) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("failed outcome checkpoint failed: {error}"),
        Err(error) => panic!("failed outcome checkpoint did not complete: {error}"),
    }
    let session = ctx_traits_io::run_session::read_run_session(&ledger).expect("read ledger");
    assert!(
        session.last_drive_outcome.is_none(),
        "failed outcome write must not leave an outcome notification candidate"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn unavailable_center_does_not_change_a_completed_drive_or_its_durable_evidence() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("drive-notification-failure");
    let home = root.join("home");
    let repo = root.join("repository");
    std::fs::create_dir_all(&repo).expect("create fixture repository");
    let harness = root.join("fixture-harness.sh");
    write_fixture_harness(&harness);
    write_drive_fixture(&repo, &harness);
    let fixture = ".ctx/traits/center-drive-proof/generated/index.toml";
    require_success("fixture init", &["traits", "init"], &repo, &home);
    require_success(
        "fixture review",
        &[
            "traits",
            "internal",
            "review",
            "--file",
            fixture,
            "--approve",
        ],
        &repo,
        &home,
    );
    require_success(
        "fixture activate",
        &["traits", "state", "--active", "--file", fixture],
        &repo,
        &home,
    );

    // This is a configured endpoint that cannot start or accept a center. The
    // driver still creates its notifier, whose worker may only lose freshness.
    let ledger = repo.join(".ctx/runs/center-drive-proof.json");
    let ledger_text = ledger.to_string_lossy().into_owned();
    let output = controlled_command(
        std::path::Path::new(env!("CARGO_BIN_EXE_ctx")),
        &[
            "traits",
            "run",
            "--file",
            fixture,
            "--out",
            &ledger_text,
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    )
    .env("CTX_CENTER_SOCKET", root.join("missing-center.sock"))
    .env("CTX_CENTER_SPAWN_LOCK", root.join("center.lock"))
    .env("CTX_CENTER_RUNS_ROOT", &root)
    .env("CTX_CENTER_INDEX", root.join("index.sqlite3"))
    .env("CTX_CENTER_EXECUTABLE", root.join("does-not-exist"))
    .output()
    .expect("run fixture drive without center");
    assert!(
        output.status.success(),
        "notification failure changed drive exit status: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("decode completed drive report");
    let drive_report = &report["value"]["drive"];
    assert_eq!(
        drive_report["status"], "completed",
        "unexpected drive report: {report}"
    );
    assert!(
        drive_report["warnings"]
            .as_array()
            .expect("drive warnings array")
            .iter()
            .all(|warning| !warning.as_str().unwrap_or_default().contains("center")),
        "notification loss must not add center warnings: {report}"
    );
    let ledger_bytes = std::fs::read(&ledger).expect("read authoritative ledger");
    assert!(
        !ledger_bytes.is_empty(),
        "drive must still write its ledger"
    );
    let activity = ctx_traits_io::activity_sidecar::activity_path(
        &Utf8PathBuf::from_path_buf(ledger.clone()).expect("UTF-8 ledger"),
    );
    assert!(
        std::fs::metadata(activity.as_std_path())
            .expect("read durable activity sidecar")
            .len()
            > 0,
        "notification loss must not suppress durable activity"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn notification_failures_match_disabled_baseline() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("notification-failure-baseline");
    std::fs::create_dir_all(&root).expect("create scratch root");

    let baseline_root = root.join("baseline");
    std::fs::create_dir_all(&baseline_root).expect("create baseline root");
    let baseline_socket = baseline_root.join("center.sock");
    let baseline = run_drive_with_notification_environment(&baseline_root, |command| {
        command
            .env("CTX_CENTER_DISABLE_NOTIFICATIONS", "1")
            .env("CTX_CENTER_SOCKET", &baseline_socket)
            .env("CTX_CENTER_SPAWN_LOCK", baseline_root.join("center.lock"))
            .env("CTX_CENTER_RUNS_ROOT", &baseline_root)
            .env("CTX_CENTER_INDEX", baseline_root.join("index.sqlite3"))
            .env(
                "CTX_CENTER_EXECUTABLE",
                baseline_root.join("does-not-start"),
            );
    });
    assert!(baseline.status.success(), "disabled baseline drive failed");

    let absent_root = root.join("absent");
    std::fs::create_dir_all(&absent_root).expect("create absent root");
    let absent_socket = absent_root.join("center.sock");
    let absent = run_drive_with_notification_environment(&absent_root, |command| {
        command
            .env("CTX_CENTER_SOCKET", &absent_socket)
            .env("CTX_CENTER_SPAWN_LOCK", absent_root.join("center.lock"))
            .env("CTX_CENTER_RUNS_ROOT", &absent_root)
            .env("CTX_CENTER_INDEX", absent_root.join("index.sqlite3"))
            .env("CTX_CENTER_EXECUTABLE", absent_root.join("does-not-start"));
    });
    assert_same_durable_evidence(&baseline, &absent);

    let refused_root = root.join("refused");
    std::fs::create_dir_all(&refused_root).expect("create refused root");
    let refused_socket = refused_root.join("center.sock");
    let refused_listener = UnixListener::bind(&refused_socket).expect("bind refused peer");
    let (refused_seen, refused_result) = mpsc::channel();
    let refused_peer = std::thread::spawn(move || {
        for connection in 0..2 {
            let (mut stream, _) = refused_listener.accept().expect("accept refused peer");
            let mut hello = String::new();
            BufReader::new(stream.try_clone().expect("clone refused stream"))
                .read_line(&mut hello)
                .expect("read peer hello");
            let id = serde_json::from_str::<serde_json::Value>(&hello).expect("decode peer hello")
                ["id"]
                .as_str()
                .expect("hello id")
                .to_string();
            stream
                .write_all(format!("{{\"kind\":\"ready\",\"id\":\"{id}\"}}\n").as_bytes())
                .expect("write peer ready");
            if connection == 1 {
                let mut register = String::new();
                BufReader::new(stream.try_clone().expect("clone register stream"))
                    .read_line(&mut register)
                    .expect("read registration before refusal");
                let register_id = serde_json::from_str::<serde_json::Value>(&register)
                    .expect("decode registration")["id"]
                    .as_str()
                    .expect("registration id")
                    .to_string();
                stream
                    .write_all(
                        format!(
                            "{{\"kind\":\"response\",\"id\":\"{register_id}\",\"result\":{{\"type\":\"ok\"}}}}\n"
                        )
                        .as_bytes(),
                    )
                    .expect("acknowledge registration");
                stream.flush().expect("flush registration acknowledgement");
                let mut event = String::new();
                BufReader::new(stream.try_clone().expect("clone event stream"))
                    .read_line(&mut event)
                    .expect("read post-registration event before refusal");
                let _ = refused_seen.send(());
                // Drop without acknowledging a post-registration event. The
                // notifier's next write/acknowledgement observes the failure.
            }
        }
    });
    let refused = run_drive_with_notification_environment(&refused_root, |command| {
        command
            .env("CTX_CENTER_SOCKET", &refused_socket)
            .env("CTX_CENTER_SPAWN_LOCK", refused_root.join("center.lock"))
            .env("CTX_CENTER_RUNS_ROOT", &refused_root)
            .env("CTX_CENTER_INDEX", refused_root.join("index.sqlite3"))
            .env("CTX_CENTER_EXECUTABLE", refused_root.join("does-not-start"));
    });
    refused_result
        .recv_timeout(PROCESS_DEADLINE)
        .expect("refused peer observed registration");
    refused_peer.join().expect("join refused peer");
    assert_same_durable_evidence(&baseline, &refused);

    let saturated_root = root.join("saturated");
    std::fs::create_dir_all(&saturated_root).expect("create saturated root");
    let saturated_socket = saturated_root.join("center.sock");
    let saturated_listener = UnixListener::bind(&saturated_socket).expect("bind saturated peer");
    let (blocked_sender, blocked_receiver) = mpsc::channel();
    let saturated_peer = std::thread::spawn(move || {
        // Startup confirms this listener first. The notifier's next connection
        // then blocks in its handshake, leaving its one-slot producer queue
        // full while the real drive emits activity, frame, and terminal events.
        let (mut startup, _) = saturated_listener.accept().expect("accept startup probe");
        let mut hello = String::new();
        BufReader::new(startup.try_clone().expect("clone startup stream"))
            .read_line(&mut hello)
            .expect("read startup hello");
        let id =
            serde_json::from_str::<serde_json::Value>(&hello).expect("decode startup hello")["id"]
                .as_str()
                .expect("startup hello id")
                .to_string();
        startup
            .write_all(format!("{{\"kind\":\"ready\",\"id\":\"{id}\"}}\n").as_bytes())
            .expect("write startup ready");
        let (notifier, _) = saturated_listener.accept().expect("accept notifier");
        let mut notifier_hello = String::new();
        BufReader::new(notifier.try_clone().expect("clone notifier stream"))
            .read_line(&mut notifier_hello)
            .expect("read notifier hello");
        blocked_sender.send(()).expect("signal blocked notifier");
        std::thread::sleep(Duration::from_secs(3));
        let _ = notifier.shutdown(std::net::Shutdown::Both);
    });
    let saturated = run_drive_with_notification_environment(&saturated_root, |command| {
        command
            .env("CTX_CENTER_SOCKET", &saturated_socket)
            .env("CTX_CENTER_SPAWN_LOCK", saturated_root.join("center.lock"))
            .env("CTX_CENTER_RUNS_ROOT", &saturated_root)
            .env("CTX_CENTER_INDEX", saturated_root.join("index.sqlite3"))
            .env(
                "CTX_CENTER_EXECUTABLE",
                saturated_root.join("does-not-start"),
            )
            .env("CTX_CENTER_NOTIFIER_QUEUE", "1");
    });
    blocked_receiver
        .recv_timeout(PROCESS_DEADLINE)
        .expect("notifier blocked before producer saturation");
    saturated_peer.join().expect("join saturated peer");
    assert_same_durable_evidence(&baseline, &saturated);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn restarted_center_accepts_the_next_driver_registration_and_frame() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("restart-reregister");
    std::fs::create_dir_all(&root).expect("create scratch root");
    let ledger = write_running_ledger(&root);
    let socket = root.join("center.sock");
    let index = root.join("index.sqlite3");
    let _environment = CenterEnvironment::install(&root);
    let mut first = spawn_sentinel(&root, &socket, &index, "5000");
    drop(await_socket(&socket));

    let facts = ctx_traits_io::run_liveness::LiveRunFacts {
        session_id: "center-proof-session".to_string(),
        run_id: "center-proof-run".to_string(),
        repo_key: "repository".to_string(),
        repo_path: root.display().to_string(),
        ledger_path: ledger.clone(),
        worktree_path: None,
        branch: None,
        log_path: None,
    };
    let driver_lock = ctx_traits_io::run_control::try_acquire(&facts, std::sync::Arc::new(|| {}))
        .expect("acquire driver lock")
        .expect("test owns driver lock");
    // Establish the notifier connection before the crash. The later frame must
    // use this same worker, reconnecting and registering again on its own.
    let notifier =
        ctx_traits_io::center::DriverNotifier::new(ctx_traits_io::center::DriverRegistration {
            ledger_path: ledger.to_string(),
            holder: driver_lock.holder().clone(),
        });
    notifier.frame_done();
    let marker = root.join("frames");
    let deadline = Instant::now() + PROCESS_DEADLINE;
    while !marker.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        std::fs::read_to_string(&marker)
            .expect("read initial frame marker")
            .lines()
            .count(),
        1,
        "the original notifier established its first registration"
    );
    first.0.kill().expect("SIGKILL first center");
    assert!(!first.0.wait().expect("reap first center").success());
    // SIGKILL bypasses SocketGuard. The restarted sentinel receives the same
    // stale-listener condition that startup arbitration normally removes.
    std::fs::remove_file(&socket).expect("remove dead center socket");

    let mut second = spawn_sentinel(&root, &socket, &index, "5000");
    drop(await_socket(&socket));
    write_completed_ledger(&ledger);
    notifier.frame_done();
    let deadline = Instant::now() + PROCESS_DEADLINE;
    while std::fs::read_to_string(&marker)
        .map(|frames| frames.lines().count() < 2)
        .unwrap_or(true)
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        std::fs::read_to_string(marker)
            .expect("read restarted frame marker")
            .lines()
            .count(),
        2,
        "the original notifier re-registered before its next frame reached the restarted center"
    );
    drop(notifier);
    drop(driver_lock);
    second.0.kill().expect("stop restarted center");
    second.0.wait().expect("reap restarted center");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn periodic_scan_repairs_a_dropped_frame_notification_for_subscribers() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("scan-repair");
    std::fs::create_dir_all(&root).expect("create scratch root");
    let ledger = write_running_ledger(&root);
    let socket = root.join("center.sock");
    let index = root.join("index.sqlite3");
    let _environment = CenterEnvironment::install(&root);
    let mut child = spawn_sentinel(&root, &socket, &index, "5000");
    drop(await_socket(&socket));

    let subscription = ctx_traits_io::center::subscribe(None).expect("subscription");
    assert!(matches!(
        subscription.recv_timeout(PROCESS_DEADLINE),
        Ok(ctx_traits_io::center::CenterEvent::SnapshotStart)
    ));
    assert!(matches!(
        subscription.recv_timeout(PROCESS_DEADLINE),
        Ok(ctx_traits_io::center::CenterEvent::SnapshotRow(_))
    ));
    assert!(matches!(
        subscription.recv_timeout(PROCESS_DEADLINE),
        Ok(ctx_traits_io::center::CenterEvent::SnapshotEnd)
    ));

    // Simulate a notification lost with the center: the authoritative ledger
    // changes, but no DriverNotifier event is sent. The scan must use the same
    // refresh/diff path and deliver the repaired model delta.
    write_completed_ledger(&ledger);
    let deadline = Instant::now() + PROCESS_DEADLINE;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "scan did not repair the dropped frame"
        );
        match subscription.recv_timeout(remaining) {
            Ok(ctx_traits_io::center::CenterEvent::Delta(
                ctx_traits_io::center::CenterDelta::RowChanged { row },
            )) if row.summary.status == ctx_traits_core::procedure::session::Status::Completed => {
                break;
            }
            Ok(_) => {}
            Err(error) => panic!("scan-repair delta: {error}"),
        }
    }
    drop(subscription);
    child.0.kill().expect("stop private sentinel");
    child.0.wait().expect("reap private sentinel");
    let _ = std::fs::remove_dir_all(root);
}
