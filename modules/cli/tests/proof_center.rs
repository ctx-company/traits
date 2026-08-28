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
    names: [&'static str; 11],
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
            "CTX_CENTER_LIVENESS_ROOT",
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
            // Isolate this proof's driver-liveness index from the real
            // machine-global one (`run_control::runtime_root`) and from
            // whatever any other concurrently or previously run test left
            // behind there — the same reasoning as the other four tuple
            // paths above.
            std::env::set_var("CTX_CENTER_LIVENESS_ROOT", root.join("liveness"));
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

fn write_repo_scoped_completed_ledger(root: &std::path::Path, repo_key: &str) -> Utf8PathBuf {
    let root = Utf8PathBuf::from_path_buf(root.to_path_buf()).expect("UTF-8 scratch root");
    let ledger = root.join(repo_key).join("session.json");
    let session = serde_json::from_value(serde_json::json!({
        "schema-version": "0.1.0",
        "session-id": "cached-center-session",
        "run-id": "cached-center-run",
        "trait-id": "cached-center-trait",
        "current-run-index": 0,
        "status": "completed",
        "provenance": {
            "started-by": {"surface": "test", "caller": "proof-center"},
            "state-source": "test",
            "started-at-epoch": 1000,
            "task-key": "cached-center-task",
            "merge-frames": [{
                "stage": "landing",
                "status": "merged",
                "evidence": ["landed=0123456789abcdef0123456789abcdef01234567"],
            }],
        },
        "ledger": {
            "run-id": "cached-center-run",
            "trait-id": "cached-center-trait",
            "current-run-index": 0,
            "final-state": "completed",
        },
        "last-drive-outcome": {"outcome": "completed", "recorded-at-epoch": 1001},
        "state-digest": "sha256:cached-center-proof",
    }))
    .expect("completed fixture session");
    ctx_traits_io::run_session::write_run_session(&ledger, &session).expect("write ledger");
    ledger
}

#[allow(clippy::too_many_arguments)]
fn assert_center_backed_readers_serve(
    socket: &std::path::Path,
    index: &std::path::Path,
    runs_root: &std::path::Path,
    repo: &std::path::Path,
    home: &std::path::Path,
    board: &std::path::Path,
    task_key: &str,
    run_id: &str,
    ledger_path: &camino::Utf8Path,
) {
    let run = |args: &[&str]| {
        let mut command = controlled_command(
            std::path::Path::new(env!("CARGO_BIN_EXE_ctx")),
            args,
            repo,
            home,
        );
        command
            .env("CTX_CENTER_SOCKET", socket)
            .env("CTX_CENTER_SPAWN_LOCK", runs_root.join("center.lock"))
            .env("CTX_CENTER_RUNS_ROOT", runs_root)
            .env("CTX_CENTER_INDEX", index)
            .env("CTX_CENTER_EXECUTABLE", runs_root.join("does-not-start"));
        command.output().expect("run center-backed reader")
    };
    let stats = run(&["traits", "internal", "stats", "--json"]);
    assert!(
        stats.status.success(),
        "stats failed: {}",
        String::from_utf8_lossy(&stats.stderr)
    );
    let stats: serde_json::Value = serde_json::from_slice(&stats.stdout).expect("stats JSON");
    assert_eq!(stats["total-runs"], 1);

    let board = board.to_str().expect("UTF-8 board");
    let proposals = run(&["tasks", "proposals", "--board", board, "--json"]);
    assert!(
        proposals.status.success(),
        "proposals failed: {}",
        String::from_utf8_lossy(&proposals.stderr)
    );
    assert!(String::from_utf8_lossy(&proposals.stdout).contains(task_key));
    let reconcile = run(&["tasks", "reconcile", "--board", board, "--json"]);
    assert!(
        reconcile.status.success(),
        "reconcile failed: {}",
        String::from_utf8_lossy(&reconcile.stderr)
    );

    let subscription = ctx_traits_io::center::subscribe(None).expect("center subscription");
    assert!(matches!(
        subscription.recv_timeout(PROCESS_DEADLINE),
        Ok(ctx_traits_io::center::CenterEvent::SnapshotStart)
    ));
    let mut found = false;
    loop {
        match subscription
            .recv_timeout(PROCESS_DEADLINE)
            .expect("center snapshot event")
        {
            ctx_traits_io::center::CenterEvent::SnapshotRow(row) => {
                found |= row.summary.parse_error.is_none() && row.summary.run_id == run_id;
            }
            ctx_traits_io::center::CenterEvent::SnapshotEnd => break,
            _ => {}
        }
    }
    assert!(found, "subscription must serve the cached readable row");
    drop(subscription);

    let merge = run(&["traits", "merge", run_id, "--json"]);
    assert!(
        !merge.status.success(),
        "merge must reopen the unavailable ledger"
    );
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&merge.stdout),
        String::from_utf8_lossy(&merge.stderr)
    );
    assert!(
        output.contains(ledger_path.as_str()),
        "merge error must name ledger: {output}"
    );
    assert!(
        !output.contains("center records no run-session ledger"),
        "merge must resolve the cached center row before reopening: {output}"
    );
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

const DRIVE_PROOF_TRAIT_TWO_FRAMES: &str = r#"id = "center-drive-proof"
schema-version = "0.4"
version = "0.1.0"
name = "Center drive proof"
description = "Two accepted frames through the real driver."

[[agent]]
id = "worker"
description = "Fixture worker."
summary = "Fixture worker."

[[slot]]
id = "answer"
schema = "schema:boolean"
description = "Fixture output."

[[slot]]
id = "answer-two"
schema = "schema:boolean"
description = "Second fixture output."

[procedure]
description = "Two fixture frames."

[[procedure.sequence]]
id = "frame-one"
title = "Accept first frame"
agent = "agent:worker"
prompt = "Return true."
output = ["slot:answer"]

[[procedure.sequence]]
id = "frame-two"
title = "Accept second frame"
agent = "agent:worker"
prompt = "Return true again."
output = ["slot:answer-two"]
"#;

fn write_drive_fixture(repo: &std::path::Path, script: &std::path::Path) {
    write_drive_fixture_with_trait(repo, script, DRIVE_PROOF_TRAIT);
}

fn write_drive_fixture_with_trait(
    repo: &std::path::Path,
    script: &std::path::Path,
    trait_source: &str,
) {
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
        trait_source,
    )
    .expect("write fixture trait");
}

enum FixtureRelease<'a> {
    AfterFixedDelay,
    WhenFileAppears(&'a std::path::Path),
}

fn write_fixture_harness(path: &std::path::Path, release: FixtureRelease<'_>) {
    let release = match release {
        FixtureRelease::AfterFixedDelay => "sleep 1".to_string(),
        FixtureRelease::WhenFileAppears(file) => format!(
            "i=0; while [ ! -e \"{}\" ]; do i=$((i+1)); [ \"$i\" -gt 120 ] && exit 9; sleep 1; done",
            file.display()
        ),
    };
    std::fs::write(
        path,
        format!(
            r#"#!/bin/sh
if [ "$1" = "--fixture-probe" ]; then
  printf 'fixture-1.0\n'
  exit 0
fi
cat >/dev/null
# Leave the real driver's registration refresh observable before the accepted
# frame is released from this fixture harness.
{release}
count_file="$0.invocations"
count=0
[ -f "$count_file" ] && count=$(cat "$count_file")
count=$((count+1))
printf '%s\n' "$count" > "$count_file"
case "$count" in
  1) result='{{\"answer\":true}}' ;;
  2) result='{{\"answer-two\":true}}' ;;
  *) result='{{\"answer\":true}}' ;;
esac
printf '%s\n' "{{\"type\":\"result\",\"session_id\":\"center-drive-proof\",\"result\":\"$result\"}}"
"#
        ),
    )
    .expect("write fixture harness");
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)
        .expect("stat fixture harness")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("make fixture harness executable");
}

fn prepare_drive_fixture(
    root: &std::path::Path,
    release: FixtureRelease<'_>,
) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let home = root.join("home");
    let repo = root.join("repository");
    std::fs::create_dir_all(&repo).expect("create fixture repository");
    let harness = root.join("fixture-harness.sh");
    write_fixture_harness(&harness, release);
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
    (repo, home, fixture.to_string())
}

fn prepare_two_frame_drive_fixture(
    root: &std::path::Path,
    release: FixtureRelease<'_>,
) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let home = root.join("home");
    let repo = root.join("repository");
    std::fs::create_dir_all(&repo).expect("create fixture repository");
    let harness = root.join("fixture-harness.sh");
    write_fixture_harness(&harness, release);
    write_drive_fixture_with_trait(&repo, &harness, DRIVE_PROOF_TRAIT_TWO_FRAMES);
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
    (repo, home, fixture.to_string())
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
    write_fixture_harness(&harness, FixtureRelease::AfterFixedDelay);
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
            .env("CTX_CENTER_LIVENESS_ROOT", root.join("liveness"))
            .spawn()
            .expect("spawn private sentinel"),
    )
}

fn spawn_sentinel_with_home(
    root: &std::path::Path,
    socket: &std::path::Path,
    index: &std::path::Path,
    idle_ms: &str,
    home: &std::path::Path,
) -> ChildGuard {
    ChildGuard(
        sentinel_with_home_command(root, socket, index, idle_ms, home)
            .spawn()
            .expect("spawn private sentinel with fixture environment"),
    )
}

fn sentinel_with_home_command(
    root: &std::path::Path,
    socket: &std::path::Path,
    index: &std::path::Path,
    idle_ms: &str,
    home: &std::path::Path,
) -> std::process::Command {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_ctx"));
    command
        .arg("__ctx-center")
        .env("CTX_CENTER_SOCKET", socket)
        .env("CTX_CENTER_SPAWN_LOCK", root.join("center.lock"))
        .env("CTX_CENTER_RUNS_ROOT", root)
        .env("CTX_CENTER_INDEX", index)
        .env("CTX_CENTER_IDLE_MS", idle_ms)
        .env("CTX_CENTER_SCAN_MS", "20")
        .env("CTX_CENTER_LIVENESS_ROOT", root.join("liveness"))
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("XDG_CACHE_HOME", home)
        .env("TMPDIR", home)
        .env("NO_COLOR", "1")
        .env("PATH", std::env::var("PATH").unwrap_or_default());
    command
}

fn spawn_sentinel_with_home_and_outcome_failure_hook(
    root: &std::path::Path,
    socket: &std::path::Path,
    index: &std::path::Path,
    idle_ms: &str,
    home: &std::path::Path,
) -> ChildGuard {
    ChildGuard(
        sentinel_with_home_command(root, socket, index, idle_ms, home)
            .env("CTX_INTERNAL_TESTHOOK_FAIL_DRIVE_OUTCOME_WRITE", "1")
            .spawn()
            .expect("spawn private sentinel with fixture environment and hook"),
    )
}

fn start_fixture(repo: &std::path::Path, fixture: &str, ledger: &std::path::Path) -> String {
    match ctx_traits_io::center::start_trait(
        &[
            "--file".to_string(),
            fixture.to_string(),
            "--out".to_string(),
            ledger.to_string_lossy().into_owned(),
            "--json".to_string(),
        ],
        camino::Utf8Path::from_path(repo).expect("UTF-8 repository"),
    )
    .expect("center start request")
    {
        ctx_traits_io::center::StartResult::Started { session_id } => session_id,
        ctx_traits_io::center::StartResult::Exited { code, stderr } => {
            panic!("fixture driver exited before registering ({code:?}): {stderr}")
        }
    }
}

fn await_outcome(ledger: &std::path::Path, outcome: &str) {
    let deadline = Instant::now() + PROCESS_DEADLINE;
    loop {
        if let Ok(session) = ctx_traits_io::run_session::read_run_session(
            &Utf8PathBuf::from_path_buf(ledger.to_path_buf()).expect("UTF-8 ledger"),
        ) && session
            .last_drive_outcome
            .as_ref()
            .map(|record| record.outcome.as_str())
            == Some(outcome)
        {
            return;
        }
        assert!(Instant::now() < deadline, "ledger did not record {outcome}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn await_session_id(ledger: &std::path::Path) -> String {
    let deadline = Instant::now() + PROCESS_DEADLINE;
    loop {
        if let Ok(session) = ctx_traits_io::run_session::read_run_session(
            &Utf8PathBuf::from_path_buf(ledger.to_path_buf()).expect("UTF-8 ledger"),
        ) {
            return session.session_id.as_str().to_string();
        }
        assert!(Instant::now() < deadline, "driver did not write its ledger");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn await_control(session_id: &str, action: ctx_traits_io::center::ControlAction) {
    let deadline = Instant::now() + PROCESS_DEADLINE;
    loop {
        match ctx_traits_io::center::control(session_id, None, action) {
            Ok(ctx_traits_io::center::ControlResult::Acknowledged) => return,
            Ok(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(result) => panic!("center did not acknowledge control request: {result:?}"),
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("control request: {error}"),
        }
    }
}

fn await_start_log(root: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    let logs = root.join("start-logs");
    let deadline = Instant::now() + PROCESS_DEADLINE;
    loop {
        if let Ok(entries) = std::fs::read_dir(&logs)
            && let Some(path) = entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .find(|path| path.to_string_lossy().ends_with(suffix))
        {
            return path;
        }
        assert!(Instant::now() < deadline, "center did not create {suffix}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn requester_process_start_helper() {
    let Ok(repo) = std::env::var("CTX_TEST_START_REPOSITORY") else {
        return;
    };
    let fixture = std::env::var("CTX_TEST_START_FIXTURE").expect("fixture argument");
    let ledger = std::env::var("CTX_TEST_START_LEDGER").expect("ledger argument");
    let _ = ctx_traits_io::center::start_trait(
        &[
            "--file".to_string(),
            fixture,
            "--out".to_string(),
            ledger,
            "--json".to_string(),
        ],
        camino::Utf8Path::new(&repo),
    );
}

fn commit_fixture_repo(repo: &std::path::Path) {
    let status = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=CTX Test",
            "-c",
            "user.email=ctx-test@example.invalid",
            "add",
            ".",
        ])
        .current_dir(repo)
        .status()
        .expect("stage fixture repository");
    assert!(status.success(), "stage fixture repository");
    let status = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=CTX Test",
            "-c",
            "user.email=ctx-test@example.invalid",
            "commit",
            "-m",
            "fixture",
        ])
        .current_dir(repo)
        .status()
        .expect("commit fixture repository");
    assert!(status.success(), "commit fixture repository");
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

/// One direct sentinel handshake, bypassing `hello`/`ready` framing.
fn handshake_ready(socket: &std::path::Path) {
    let stream = await_socket(socket);
    stream
        .try_clone()
        .expect("clone handshake stream")
        .write_all(b"{\"kind\":\"hello\",\"id\":\"proof\"}\n")
        .expect("write hello");
    let mut ready = String::new();
    BufReader::new(&stream)
        .read_line(&mut ready)
        .expect("read ready");
    assert_eq!(ready, "{\"kind\":\"ready\",\"id\":\"proof\"}\n");
}

#[test]
fn second_direct_sentinel_on_the_same_tuple_exits_without_replacing_the_owner() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("owner");
    std::fs::create_dir_all(&root).expect("create scratch root");
    let socket = root.join("center.sock");
    let spawn_lock = root.join("center.lock");
    let index = root.join("index.sqlite3");
    let mut first = ChildGuard(
        std::process::Command::new(env!("CARGO_BIN_EXE_ctx"))
            .arg("__ctx-center")
            .env("CTX_CENTER_SOCKET", &socket)
            .env("CTX_CENTER_SPAWN_LOCK", &spawn_lock)
            .env("CTX_CENTER_RUNS_ROOT", &root)
            .env("CTX_CENTER_INDEX", &index)
            .env("CTX_CENTER_IDLE_MS", "5000")
            .spawn()
            .expect("spawn first direct sentinel"),
    );
    handshake_ready(&socket);
    let owner_metadata = std::fs::symlink_metadata(&socket).expect("stat owner socket");
    use std::os::unix::fs::MetadataExt;
    let owner_dev_ino = (owner_metadata.dev(), owner_metadata.ino());

    // A second direct `ctx __ctx-center` on the exact same tuple must not
    // replace the owner: it contends the owner lock, proves a live answer,
    // and exits without binding.
    let mut second = std::process::Command::new(env!("CARGO_BIN_EXE_ctx"))
        .arg("__ctx-center")
        .env("CTX_CENTER_SOCKET", &socket)
        .env("CTX_CENTER_SPAWN_LOCK", &spawn_lock)
        .env("CTX_CENTER_RUNS_ROOT", &root)
        .env("CTX_CENTER_INDEX", &index)
        .env("CTX_CENTER_IDLE_MS", "5000")
        .spawn()
        .expect("spawn second direct sentinel");
    let status = await_exit(&mut second);
    assert!(
        status.success(),
        "a second direct center on an owned tuple must exit without error, got {status}"
    );

    let after_metadata = std::fs::symlink_metadata(&socket).expect("stat socket after contender");
    assert_eq!(
        (after_metadata.dev(), after_metadata.ino()),
        owner_dev_ino,
        "the owner's socket inode must survive a contended second direct start"
    );
    // The original owner must still answer.
    handshake_ready(&socket);

    // A launcher path (`ensure_connected`) against the already-serving owner
    // must reuse it rather than spawning a competitor: no new process is
    // observable other than the reused socket, whose dev/inode must be
    // unchanged after the launcher call.
    let names = [
        "CTX_CENTER_SOCKET",
        "CTX_CENTER_SPAWN_LOCK",
        "CTX_CENTER_RUNS_ROOT",
        "CTX_CENTER_INDEX",
        "CTX_CENTER_EXECUTABLE",
    ];
    let previous: Vec<Option<std::ffi::OsString>> = names.iter().map(std::env::var_os).collect();
    // SAFETY: mutation is serialized by SENTINEL_TEST_LOCK for the whole proof.
    unsafe {
        std::env::set_var("CTX_CENTER_SOCKET", &socket);
        std::env::set_var("CTX_CENTER_SPAWN_LOCK", &spawn_lock);
        std::env::set_var("CTX_CENTER_RUNS_ROOT", &root);
        std::env::set_var("CTX_CENTER_INDEX", &index);
        std::env::set_var("CTX_CENTER_EXECUTABLE", env!("CARGO_BIN_EXE_ctx"));
    }
    let launcher_result = ctx_traits_io::center::ensure_connected();
    unsafe {
        for (name, previous) in names.iter().zip(previous.iter()) {
            match previous {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
    launcher_result.expect("launcher reuses the already-serving owner");
    let after_launcher_metadata =
        std::fs::symlink_metadata(&socket).expect("stat socket after launcher call");
    assert_eq!(
        (after_launcher_metadata.dev(), after_launcher_metadata.ino()),
        owner_dev_ino,
        "a launcher reusing a live owner must never rebind its socket"
    );

    // An isolated private tuple (a distinct socket path) still starts
    // independently in the same scratch root.
    let other_socket = root.join("other.sock");
    let mut isolated = ChildGuard(
        std::process::Command::new(env!("CARGO_BIN_EXE_ctx"))
            .arg("__ctx-center")
            .env("CTX_CENTER_SOCKET", &other_socket)
            .env("CTX_CENTER_SPAWN_LOCK", root.join("other.lock"))
            .env("CTX_CENTER_RUNS_ROOT", &root)
            .env("CTX_CENTER_INDEX", root.join("other-index.sqlite3"))
            .env("CTX_CENTER_IDLE_MS", "5000")
            .spawn()
            .expect("spawn isolated sentinel"),
    );
    handshake_ready(&other_socket);

    first.0.kill().expect("stop first sentinel");
    first.0.wait().expect("reap first sentinel");
    isolated.0.kill().expect("stop isolated sentinel");
    isolated.0.wait().expect("reap isolated sentinel");
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
            .env("CTX_CENTER_LIVENESS_ROOT", root.join("liveness"))
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
            .env("CTX_CENTER_LIVENESS_ROOT", root.join("liveness"))
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
    // The replacement center reconstructs liveness for the still-held ledger
    // from the kernel flock alone: no driver re-registered and no driver
    // frame was ever sent to this fresh process.
    let rows = ctx_traits_io::center::list(None).expect("query restarted center");
    let row = rows
        .iter()
        .find(|row| row.summary.session_id == "center-proof-session")
        .expect("restarted center indexes the still-held ledger");
    assert!(
        row.live,
        "restarted center must report the genuinely held ledger as live"
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

    // Simulate a preceding v1 index: no projection marker at all. The
    // metadata-only rebuild drops the stale-shaped `center_rows` payload and
    // the payload cache table, and the next process reprojects directly from
    // the authoritative ledger (not from an edited cached summary — that
    // cache no longer exists) without changing the legacy marker an
    // installed v1 center still requires.
    let db = rusqlite::Connection::open(&index).expect("open first center index");
    db.execute("DELETE FROM center_projection_meta", [])
        .expect("remove marker absent from the preceding center");
    drop(db);

    let mut second = spawn_sentinel(&root, &second_socket, &index, "5000");
    drop(await_socket(&second_socket));
    // `list` resolves its target from the process environment; point it at
    // the second sentinel's socket explicitly rather than relying on
    // whatever CTX_CENTER_SOCKET a previous test in this serialized suite
    // happened to leave behind. Scoped so these four overrides cannot leak
    // into a later test in this serialized suite even on an early return.
    let names = [
        "CTX_CENTER_SOCKET",
        "CTX_CENTER_SPAWN_LOCK",
        "CTX_CENTER_RUNS_ROOT",
        "CTX_CENTER_INDEX",
    ];
    let previous: Vec<Option<std::ffi::OsString>> = names.iter().map(std::env::var_os).collect();
    // SAFETY: mutation is serialized by SENTINEL_TEST_LOCK for the whole
    // proof, so no concurrently executing test can observe it.
    unsafe {
        std::env::set_var("CTX_CENTER_SOCKET", &second_socket);
        std::env::set_var("CTX_CENTER_SPAWN_LOCK", root.join("center.lock"));
        std::env::set_var("CTX_CENTER_RUNS_ROOT", &root);
        std::env::set_var("CTX_CENTER_INDEX", &index);
    }
    let restore_env = || {
        // SAFETY: same serialization as above.
        unsafe {
            for (name, previous) in names.iter().zip(previous.iter()) {
                match previous {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    };
    let list_result = ctx_traits_io::center::list(None);
    restore_env();
    for (name, previous) in names.iter().zip(previous.iter()) {
        assert_eq!(
            std::env::var_os(name).as_ref(),
            previous.as_ref(),
            "{name} must be restored after the scoped override"
        );
    }
    list_result.expect("second center answers after rebuild");
    std::thread::sleep(Duration::from_millis(200));
    let db = rusqlite::Connection::open(&index).expect("open shared index metadata");
    let version: i64 = db
        .query_row("SELECT version FROM center_projection_meta", [], |row| {
            row.get(0)
        })
        .expect("read shared index metadata");
    assert_eq!(
        version, 3,
        "the current center must mark the metadata-only projection"
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
        std::sync::Arc::new(|_| {}),
    )
    .expect("acquire driver lock")
    .expect("test owns driver lock");
    let notifier =
        ctx_traits_io::center::DriverNotifier::new(ctx_traits_io::center::DriverRegistration {
            ledger_path: ledger.to_string(),
            holder: driver_lock.holder().clone(),
            spawn_token: None,
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
/// reopening a ledger after discovery: they stay served from the projected
/// summary. `Get` is the one query that always reads its selected ledger on
/// demand, so a read failure there is a protocol error naming the ledger,
/// never a silently served `Missing`.
#[test]
fn metadata_only_queries_serve_an_unreadable_ledger_while_get_fails_naming_it() {
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

    let error = ctx_traits_io::center::get("center-proof-session", None)
        .expect_err("get must fail naming the unreadable ledger, not report Missing");
    assert!(error.to_string().contains(ledger.as_str()));

    std::fs::set_permissions(
        ledger.as_std_path(),
        std::fs::Permissions::from_mode(original_mode),
    )
    .expect("restore ledger permissions");
    assert!(matches!(
        ctx_traits_io::center::get("center-proof-session", None).expect("restored session lookup"),
        ctx_traits_io::center::GetResult::Session(_)
    ));
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
    write_fixture_harness(&harness, FixtureRelease::AfterFixedDelay);
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
fn center_started_run_outlives_its_requester_and_appears_once_to_two_subscribers() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("center-start-two-subscribers");
    let release = root.join("release-frame");
    let (repo, home, fixture) =
        prepare_drive_fixture(&root, FixtureRelease::WhenFileAppears(&release));
    let _environment = CenterEnvironment::install(&root);
    let socket = root.join("center.sock");
    let mut sentinel =
        spawn_sentinel_with_home(&root, &socket, &root.join("index.sqlite3"), "5000", &home);
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
    let result = ctx_traits_io::center::start_trait(
        &[
            "--file".to_string(),
            fixture,
            "--out".to_string(),
            ledger.to_string_lossy().into_owned(),
            "--json".to_string(),
        ],
        camino::Utf8Path::from_path(&repo).expect("UTF-8 repository"),
    )
    .expect("center start request");
    assert!(matches!(
        result,
        ctx_traits_io::center::StartResult::Started { .. }
    ));
    std::fs::write(&release, "release").expect("release fixture frame");
    for subscription in [&first, &second] {
        let deadline = Instant::now() + PROCESS_DEADLINE;
        let mut appeared = 0;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "subscriber did not observe the detached driver's completion"
            );
            match subscription.recv_timeout(remaining) {
                Ok(ctx_traits_io::center::CenterEvent::Delta(
                    ctx_traits_io::center::CenterDelta::Appeared { row },
                )) if row.ledger_path == ledger.to_string_lossy() => appeared += 1,
                Ok(ctx_traits_io::center::CenterEvent::Delta(
                    ctx_traits_io::center::CenterDelta::RowChanged { row },
                )) if row.ledger_path == ledger.to_string_lossy()
                    && row.summary.last_drive_outcome.as_deref() == Some("completed") =>
                {
                    break;
                }
                Ok(_) => {}
                Err(error) => panic!("read start delta: {error}"),
            }
        }
        assert_eq!(appeared, 1, "each subscriber receives one Appeared delta");
    }
    drop(first);
    drop(second);
    sentinel.0.kill().expect("stop private sentinel");
    sentinel.0.wait().expect("reap private sentinel");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn center_start_of_a_child_that_exits_before_registering_reports_its_stderr() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("center-start-exit-stderr");
    std::fs::create_dir_all(root.join("home")).expect("create fixture home");
    let _environment = CenterEnvironment::install(&root);
    let socket = root.join("center.sock");
    let mut sentinel = spawn_sentinel_with_home(
        &root,
        &socket,
        &root.join("index.sqlite3"),
        "100",
        &root.join("home"),
    );
    drop(await_socket(&socket));
    let result = ctx_traits_io::center::start_trait(
        &["--definitely-not-a-traits-run-flag".to_string()],
        camino::Utf8Path::from_path(&root).expect("UTF-8 repository"),
    )
    .expect("center start request");
    match result {
        ctx_traits_io::center::StartResult::Exited { stderr, .. } => {
            assert!(
                stderr.contains("--definitely-not-a-traits-run-flag"),
                "the child stderr must be genuine, not an unrelated startup failure: {stderr}"
            )
        }
        ctx_traits_io::center::StartResult::Started { session_id } => {
            panic!("invalid child unexpectedly registered as {session_id}")
        }
    }
    assert!(
        await_exit(&mut sentinel.0).success(),
        "center idle-exits after cleanup"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn dropping_the_requester_mid_start_leaves_the_detached_driver_running() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("center-start-requester-drop");
    let release = root.join("release-frame");
    let (repo, home, fixture) =
        prepare_drive_fixture(&root, FixtureRelease::WhenFileAppears(&release));
    let _environment = CenterEnvironment::install(&root);
    let socket = root.join("center.sock");
    let mut sentinel =
        spawn_sentinel_with_home(&root, &socket, &root.join("index.sqlite3"), "5000", &home);
    drop(await_socket(&socket));
    let ledger = repo.join(".ctx/runs/center-drive-proof.json");
    // A real process owns the client socket. Killing it produces EOF on the
    // center's cloned reader, unlike dropping a thread join handle.
    let mut requester =
        std::process::Command::new(std::env::current_exe().expect("current proof test executable"))
            .args(["--exact", "requester_process_start_helper", "--nocapture"])
            .env("CTX_TEST_START_REPOSITORY", &repo)
            .env("CTX_TEST_START_FIXTURE", &fixture)
            .env("CTX_TEST_START_LEDGER", ledger.to_string_lossy().as_ref())
            .spawn()
            .expect("spawn requester process");
    let _stdout = await_start_log(&root, ".stdout.log");
    requester
        .kill()
        .expect("terminate requester before registration");
    requester.wait().expect("reap requester");
    let deadline = Instant::now() + PROCESS_DEADLINE;
    while !ledger.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(ledger.exists(), "detached driver did not create its ledger");
    std::fs::write(&release, "release").expect("release detached driver");
    await_outcome(&ledger, "completed");
    sentinel.0.kill().expect("stop private sentinel");
    sentinel.0.wait().expect("reap private sentinel");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn center_interrupt_stops_a_run_rediscovered_after_a_center_restart() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("center-interrupt-after-restart");
    let release = root.join("release-frame");
    let (repo, home, fixture) =
        prepare_drive_fixture(&root, FixtureRelease::WhenFileAppears(&release));
    let _environment = CenterEnvironment::install(&root);
    let socket = root.join("center.sock");
    let mut sentinel =
        spawn_sentinel_with_home(&root, &socket, &root.join("index.sqlite3"), "5000", &home);
    drop(await_socket(&socket));
    let ledger = repo.join(".ctx/runs/center-drive-proof.json");
    let _started_session_id = start_fixture(&repo, &fixture, &ledger);
    let session_id = await_session_id(&ledger);
    sentinel.0.kill().expect("kill original sentinel");
    sentinel.0.wait().expect("reap original sentinel");
    std::fs::remove_file(&socket).expect("remove stale socket after SIGKILL");
    let mut restarted =
        spawn_sentinel_with_home(&root, &socket, &root.join("index.sqlite3"), "5000", &home);
    drop(await_socket(&socket));
    await_control(&session_id, ctx_traits_io::center::ControlAction::Interrupt);
    std::fs::write(&release, "release").expect("release interrupted harness");
    await_outcome(&ledger, "interrupted");
    restarted.0.kill().expect("stop restarted sentinel");
    restarted.0.wait().expect("reap restarted sentinel");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn center_pause_in_flight_persists_before_its_delta_and_resume_continues_at_the_next_frame() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("center-pause-resume");
    let release = root.join("release-frame");
    let (repo, home, fixture) =
        prepare_two_frame_drive_fixture(&root, FixtureRelease::WhenFileAppears(&release));
    let _environment = CenterEnvironment::install(&root);
    let socket = root.join("center.sock");
    let mut sentinel =
        spawn_sentinel_with_home(&root, &socket, &root.join("index.sqlite3"), "5000", &home);
    drop(await_socket(&socket));
    let ledger = repo.join(".ctx/runs/center-drive-proof.json");
    let _started_session_id = start_fixture(&repo, &fixture, &ledger);
    let session_id = await_session_id(&ledger);
    let subscription = ctx_traits_io::center::subscribe(None).expect("subscribe for pause delta");
    assert!(matches!(
        subscription.recv_timeout(PROCESS_DEADLINE),
        Ok(ctx_traits_io::center::CenterEvent::SnapshotStart)
    ));
    loop {
        match subscription.recv_timeout(PROCESS_DEADLINE) {
            Ok(ctx_traits_io::center::CenterEvent::SnapshotEnd) => break,
            Ok(_) => {}
            Err(error) => panic!("read pause snapshot: {error}"),
        }
    }
    let count_file = root.join("fixture-harness.sh.invocations");
    await_control(&session_id, ctx_traits_io::center::ControlAction::Pause);
    let acknowledgement_deadline = Instant::now() + Duration::from_millis(100);
    while Instant::now() < acknowledgement_deadline {
        match subscription
            .recv_timeout(acknowledgement_deadline.saturating_duration_since(Instant::now()))
        {
            Ok(ctx_traits_io::center::CenterEvent::Delta(
                ctx_traits_io::center::CenterDelta::RowChanged { row },
            )) if row.ledger_path == ledger.to_string_lossy()
                && row.summary.last_drive_outcome.as_deref() == Some("paused") =>
            {
                panic!("a control acknowledgement must not publish a state delta");
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(error) => panic!("read acknowledgement window: {error}"),
        }
    }
    std::fs::write(&release, "release first frame").expect("release first frame");
    await_outcome(&ledger, "paused");
    let deadline = Instant::now() + PROCESS_DEADLINE;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "center did not publish the durable pause"
        );
        match subscription.recv_timeout(remaining) {
            Ok(ctx_traits_io::center::CenterEvent::Delta(
                ctx_traits_io::center::CenterDelta::RowChanged { row },
            )) if row.ledger_path == ledger.to_string_lossy()
                && row.summary.last_drive_outcome.as_deref() == Some("paused") =>
            {
                break;
            }
            Ok(_) => {}
            Err(error) => panic!("read pause delta: {error}"),
        }
    }
    assert_eq!(std::fs::read_to_string(&count_file).unwrap().trim(), "1");
    let resumed =
        ctx_traits_io::center::start_session(&session_id, None).expect("resume through center");
    match resumed {
        ctx_traits_io::center::StartResult::Started { .. } => {}
        ctx_traits_io::center::StartResult::Exited { code, stderr } => {
            panic!("resume exited before registering ({code:?}): {stderr}")
        }
    }
    std::fs::write(&release, "second frame").expect("release second frame");
    await_outcome(&ledger, "completed");
    assert_eq!(std::fs::read_to_string(&count_file).unwrap().trim(), "2");
    drop(subscription);
    sentinel.0.kill().expect("stop private sentinel");
    sentinel.0.wait().expect("reap private sentinel");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn paused_worktree_run_retains_its_worktree_and_resume_reuses_it() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("center-paused-worktree");
    let release = root.join("release-frame");
    let (repo, home, fixture) =
        prepare_drive_fixture(&root, FixtureRelease::WhenFileAppears(&release));
    // Worktree creation requires a real HEAD and a resolvable default branch.
    commit_fixture_repo(&repo);
    let _environment = CenterEnvironment::install(&root);
    let socket = root.join("center.sock");
    let mut sentinel =
        spawn_sentinel_with_home(&root, &socket, &root.join("index.sqlite3"), "5000", &home);
    drop(await_socket(&socket));
    let ledger = repo.join(".ctx/runs/center-drive-proof.json");
    let _started_session_id = match ctx_traits_io::center::start_trait(
        &[
            "--file".to_string(),
            fixture,
            "--out".to_string(),
            ledger.to_string_lossy().into_owned(),
            "--json".to_string(),
            "--worktree".to_string(),
        ],
        camino::Utf8Path::from_path(&repo).expect("UTF-8 repository"),
    )
    .expect("start worktree drive")
    {
        ctx_traits_io::center::StartResult::Started { session_id } => session_id,
        result => panic!("worktree drive did not register: {result:?}"),
    };
    let session_id = await_session_id(&ledger);
    await_control(&session_id, ctx_traits_io::center::ControlAction::Pause);
    std::fs::write(&release, "pause worktree").expect("release worktree harness");
    await_outcome(&ledger, "paused");
    let paused_session = ctx_traits_io::run_session::read_run_session(
        &Utf8PathBuf::from_path_buf(ledger.clone()).expect("UTF-8 ledger"),
    )
    .expect("read paused worktree ledger");
    let retained_worktree = paused_session
        .provenance
        .worktree
        .clone()
        .expect("paused worktree provenance");
    let worktrees = repo.join(".ctx/traits/worktrees");
    let retained = std::fs::read_dir(&worktrees)
        .expect("read retained worktrees")
        .count();
    assert!(retained > 0, "paused drive must retain its worktree");
    match ctx_traits_io::center::start_session(&session_id, None) {
        Ok(ctx_traits_io::center::StartResult::Started { .. }) => {}
        result => panic!("resumed worktree driver did not register: {result:?}"),
    }
    await_outcome(&ledger, "completed");
    let completed_session = ctx_traits_io::run_session::read_run_session(
        &Utf8PathBuf::from_path_buf(ledger.clone()).expect("UTF-8 ledger"),
    )
    .expect("read resumed worktree ledger");
    assert_eq!(
        completed_session.provenance.worktree.as_ref(),
        Some(&retained_worktree),
        "resume must reuse the retained worktree identity, branch, and path"
    );
    sentinel.0.kill().expect("stop private sentinel");
    sentinel.0.wait().expect("reap private sentinel");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn failed_outcome_write_on_a_pause_reports_harness_failed_and_emits_no_ended_notification() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("center-failed-pause-outcome");
    let release = root.join("release-frame");
    let (repo, home, fixture) =
        prepare_drive_fixture(&root, FixtureRelease::WhenFileAppears(&release));
    let _environment = CenterEnvironment::install(&root);
    let socket = root.join("center.sock");
    let mut sentinel = spawn_sentinel_with_home_and_outcome_failure_hook(
        &root,
        &socket,
        &root.join("index.sqlite3"),
        "5000",
        &home,
    );
    drop(await_socket(&socket));
    let subscription = ctx_traits_io::center::subscribe(None).expect("subscribe");
    assert!(matches!(
        subscription.recv_timeout(PROCESS_DEADLINE),
        Ok(ctx_traits_io::center::CenterEvent::SnapshotStart)
    ));
    assert!(matches!(
        subscription.recv_timeout(PROCESS_DEADLINE),
        Ok(ctx_traits_io::center::CenterEvent::SnapshotEnd)
    ));
    let ledger = repo.join(".ctx/runs/center-drive-proof.json");
    let _started_session_id = start_fixture(&repo, &fixture, &ledger);
    let session_id = await_session_id(&ledger);
    await_control(&session_id, ctx_traits_io::center::ControlAction::Pause);
    std::fs::write(&release, "release pause").expect("release harness");
    let report = await_start_log(&root, ".stdout.log");
    let deadline = Instant::now() + PROCESS_DEADLINE;
    loop {
        let output = std::fs::read_to_string(&report).unwrap_or_default();
        if output.contains("harness-failed") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "detached driver report did not record harness-failed: {output}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let quiet_deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < quiet_deadline {
        match subscription.recv_timeout(Duration::from_millis(100)) {
            Ok(ctx_traits_io::center::CenterEvent::Delta(
                ctx_traits_io::center::CenterDelta::Ended { row },
            )) if row.ledger_path == ledger.to_string_lossy() => {
                panic!("failed pause outcome write must not emit Ended");
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(error) => panic!("read center delta: {error}"),
        }
    }
    let session = ctx_traits_io::run_session::read_run_session(
        &Utf8PathBuf::from_path_buf(ledger).expect("UTF-8 ledger"),
    )
    .expect("read ledger");
    assert!(session.last_drive_outcome.is_none());
    drop(subscription);
    sentinel.0.kill().expect("stop private sentinel");
    sentinel.0.wait().expect("reap private sentinel");
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
    write_fixture_harness(&harness, FixtureRelease::AfterFixedDelay);
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
    write_fixture_harness(&harness, FixtureRelease::AfterFixedDelay);
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
    write_fixture_harness(&harness, FixtureRelease::AfterFixedDelay);
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
    // The center's own warming loop concurrently probes this same ledger's
    // driver-lock file with a non-blocking flock (`run_control::probe`) to
    // classify it as unheld; that probe and this acquire race on the same
    // kernel lock, so a single non-blocking attempt can transiently lose to
    // it. Retry within a short deadline rather than treating that race as
    // ownership failure.
    let acquire_deadline = Instant::now() + PROCESS_DEADLINE;
    let driver_lock = loop {
        match ctx_traits_io::run_control::try_acquire(&facts, std::sync::Arc::new(|_| {}))
            .expect("acquire driver lock")
        {
            Some(lock) => break lock,
            None if Instant::now() < acquire_deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            None => panic!("test owns driver lock"),
        }
    };
    // Establish the notifier connection before the crash. The later frame must
    // use this same worker, reconnecting and registering again on its own.
    let notifier =
        ctx_traits_io::center::DriverNotifier::new(ctx_traits_io::center::DriverRegistration {
            ledger_path: ledger.to_string(),
            holder: driver_lock.holder().clone(),
            spawn_token: None,
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

#[test]
fn center_kill_and_restart_recovers_a_subscription_without_a_client_request() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("subscription-restart");
    std::fs::create_dir_all(&root).expect("create scratch root");
    let socket = root.join("center.sock");
    let index = root.join("index.sqlite3");
    let _environment = CenterEnvironment::install(&root);
    let mut first = spawn_sentinel(&root, &socket, &index, "5000");
    drop(await_socket(&socket));

    let subscription = ctx_traits_io::center::subscribe(None).expect("subscription");
    assert!(matches!(
        subscription.recv_timeout(PROCESS_DEADLINE),
        Ok(ctx_traits_io::center::CenterEvent::SnapshotStart)
    ));
    assert!(matches!(
        subscription.recv_timeout(PROCESS_DEADLINE),
        Ok(ctx_traits_io::center::CenterEvent::SnapshotEnd)
    ));

    let ledger = write_running_ledger(&root);
    let deadline = Instant::now() + PROCESS_DEADLINE;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "center did not publish appeared delta"
        );
        match subscription.recv_timeout(remaining) {
            Ok(ctx_traits_io::center::CenterEvent::Delta(
                ctx_traits_io::center::CenterDelta::Appeared { row },
            )) if row.ledger_path == ledger => break,
            Ok(_) => {}
            Err(error) => panic!("appeared delta: {error}"),
        }
    }

    first.0.kill().expect("kill first center");
    first.0.wait().expect("reap first center");
    // SIGKILL bypasses the sentinel's SocketGuard cleanup.
    std::fs::remove_file(&socket).expect("remove dead center socket");
    assert!(
        subscription.recv_timeout(PROCESS_DEADLINE).is_err(),
        "killed center must disconnect the subscription"
    );
    drop(subscription);

    let restarted = ctx_traits_io::center::subscribe(None).expect("restart subscription");
    assert!(matches!(
        restarted.recv_timeout(PROCESS_DEADLINE),
        Ok(ctx_traits_io::center::CenterEvent::SnapshotStart)
    ));
    let mut recovered = false;
    while !recovered {
        match restarted
            .recv_timeout(PROCESS_DEADLINE)
            .expect("restarted snapshot event")
        {
            ctx_traits_io::center::CenterEvent::SnapshotRow(row) => {
                recovered = row.ledger_path == ledger;
            }
            ctx_traits_io::center::CenterEvent::SnapshotEnd => break,
            _ => {}
        }
    }
    assert!(recovered, "restarted center must serve the persisted row");
    drop(restarted);
    await_socket_removal(&socket);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn center_backed_readers_serve_an_unreadable_ledger_warm_and_after_restart() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("unreadable-ledger-readers");
    let repo = root.join("repository");
    let home = root.join("home");
    let board = root.join("board");
    std::fs::create_dir_all(&repo).expect("create repository");
    std::fs::create_dir_all(&board).expect("create board");
    git_init(&repo);
    std::fs::write(
        board.join("cached-center-task.toml"),
        "schema-version = \"0.2\"\nkey = \"cached-center-task\"\ntitle = \"Cached center task\"\nstatus = \"ready\"\n",
    )
    .expect("write board task");
    let repo = Utf8PathBuf::from_path_buf(repo).expect("UTF-8 repository");
    let repo_root = ctx_traits_io::state::canonical_repo_root(&repo).expect("canonical repo");
    let repo_key = ctx_traits_io::state::repo_key(&repo_root);
    let ledger = write_repo_scoped_completed_ledger(&root, &repo_key);
    let socket = root.join("center.sock");
    let index = root.join("index.sqlite3");
    let _environment = CenterEnvironment::install(&root);
    let mut first = spawn_sentinel(&root, &socket, &index, "60000");
    drop(await_socket(&socket));

    let rows = ctx_traits_io::center::list(Some(&repo_key)).expect("prime center index");
    assert_eq!(rows.len(), 1, "warm center must index the completed ledger");
    let original_permissions = std::fs::metadata(&ledger)
        .expect("stat ledger")
        .permissions();
    let mut unreadable = original_permissions.clone();
    unreadable.set_mode(0o000);
    std::fs::set_permissions(&ledger, unreadable).expect("make ledger unreadable");

    assert_center_backed_readers_serve(
        &socket,
        &index,
        &root,
        repo.as_std_path(),
        &home,
        &board,
        "cached-center-task",
        "cached-center-run",
        &ledger,
    );
    first.0.kill().expect("kill warm center");
    first.0.wait().expect("reap warm center");
    std::fs::remove_file(&socket).expect("remove dead center socket");
    await_socket_removal(&socket);

    let mut second = spawn_sentinel(&root, &socket, &index, "60000");
    drop(await_socket(&socket));
    assert_center_backed_readers_serve(
        &socket,
        &index,
        &root,
        repo.as_std_path(),
        &home,
        &board,
        "cached-center-task",
        "cached-center-run",
        &ledger,
    );
    std::fs::set_permissions(&ledger, original_permissions).expect("restore ledger permissions");
    second.0.kill().expect("stop restarted center");
    second.0.wait().expect("reap restarted center");
    let _ = std::fs::remove_dir_all(root);
}

const CORPUS_LEDGER_COUNT: usize = 2500;
/// Padding per ledger so the corpus totals well over 600MB, landing squarely
/// inside goal 1's binding size proof rather than skirting its edge.
const CORPUS_LEDGER_FILLER_BYTES: usize = 260_000;

/// One large session body, built once and varied only by session/run id per
/// file, so the measured cost is the center's parse and index build — never
/// the fixture's own write cost or a repeated large-string allocation.
fn write_corpus_ledger(root: &std::path::Path, index: usize, filler: &str) -> Utf8PathBuf {
    let root = Utf8PathBuf::from_path_buf(root.to_path_buf()).expect("UTF-8 scratch root");
    let ledger = root.join(format!("corpus/session-{index}.json"));
    // The filler lives in a top-level key `Session` does not declare — never
    // `deny_unknown_fields`, so `read_session` parses it and silently drops
    // the extra key, and `RunSummary::from_session` never sees it at all.
    // That is the point: this fixture must bulk up ledger CONTENT without
    // being reachable by any field the metadata-only projection copies, or
    // the corpus proof would trivially fail the very size bound it exists
    // to prove.
    let mut value = serde_json::json!({
        "schema-version": "0.1.0",
        "session-id": format!("corpus-session-{index}"),
        "run-id": format!("corpus-run-{index}"),
        "trait-id": "corpus-trait",
        "current-run-index": 0,
        "status": "awaiting-agent-output",
        "provenance": {
            "started-by": {"surface": "test", "caller": "proof-center-corpus"},
            "state-source": "test",
            "started-at-epoch": 1000,
        },
        "ledger": {
            "run-id": format!("corpus-run-{index}"),
            "trait-id": "corpus-trait",
            "current-run-index": 0,
            "final-state": "running",
        },
        "state-digest": format!("sha256:corpus-{index}"),
    });
    value["proof-corpus-filler"] = serde_json::Value::String(filler.to_string());
    // A round trip through the typed `Session` would drop the unknown key
    // (that is exactly what makes it safe), so it must not happen here:
    // write the raw JSON bytes directly, the same shape `read_run_session`
    // (plain `read_text` + `serde_json::from_str`) expects on the read side.
    std::fs::create_dir_all(ledger.parent().expect("corpus ledger parent"))
        .expect("create corpus ledger parent");
    std::fs::write(
        ledger.as_std_path(),
        serde_json::to_string(&value).expect("serialize corpus fixture"),
    )
    .expect("write corpus ledger");
    // Prove the fixture still parses as a valid `Session` before the corpus
    // proof relies on it — a mistake here should fail loudly at generation
    // time, not surface as a mysterious center-side parse error later.
    ctx_traits_io::run_session::read_run_session(&ledger).expect("corpus fixture must parse");
    ledger
}

fn physical_index_bytes(index: &std::path::Path) -> u64 {
    ["", "-wal", "-journal", "-shm"]
        .iter()
        .filter_map(|suffix| {
            let mut path = index.as_os_str().to_owned();
            path.push(suffix);
            std::fs::metadata(&path).ok()
        })
        .map(|metadata| metadata.len())
        .sum()
}

#[test]
fn cold_start_binds_and_answers_a_partial_query_before_the_corpus_finishes_indexing() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("corpus-bind-before-build");
    std::fs::create_dir_all(&root).expect("create scratch root");
    let filler = "x".repeat(CORPUS_LEDGER_FILLER_BYTES);
    let mut corpus_bytes: u64 = 0;
    let write_started = Instant::now();
    for index in 0..CORPUS_LEDGER_COUNT {
        let ledger = write_corpus_ledger(&root, index, &filler);
        corpus_bytes += std::fs::metadata(&ledger)
            .expect("stat corpus ledger")
            .len();
    }
    eprintln!(
        "corpus proof: fixture write took {:?}, {corpus_bytes} bytes",
        write_started.elapsed()
    );
    assert!(
        corpus_bytes >= 600_000_000,
        "fixture corpus must total at least 600MB, got {corpus_bytes} bytes"
    );

    let socket = root.join("center.sock");
    let index = root.join("index.sqlite3");
    // Generous idle so idle exit is never the thing under test; `has_live`
    // already treats an in-progress warming scan as live regardless.
    let mut child = spawn_sentinel(&root, &socket, &index, "120000");
    let _environment = CenterEnvironment::install(&root);
    // The bound listener must answer a handshake immediately — no wall-clock
    // ceiling tied to how long the corpus takes to index.
    let handshake_deadline = Instant::now();
    drop(await_socket(&socket));
    assert!(
        handshake_deadline.elapsed() < PROCESS_DEADLINE,
        "the listener must be servicing handshakes long before the corpus finishes"
    );
    eprintln!(
        "corpus proof: handshake completed in {:?}",
        handshake_deadline.elapsed()
    );

    // `Stats` is a bounded aggregate (unlike a non-subscription `List`, which
    // would serialize every row into one line and risk exceeding
    // `MAX_LINE_BYTES`), so it is the natural "how many rows are indexed so
    // far" observation. Later completion polling below may retry under
    // sibling-test contention, but the *first* query is the one goal 2 binds:
    // "a connection refusal or client timeout is not [acceptable]", so it
    // must be a single request that fails hard on a timeout rather than
    // silently retrying past it.
    fn poll_stats_with_retry(deadline: Instant) -> ctx_traits_core::procedure::stats::StatsReport {
        loop {
            match ctx_traits_io::center::stats(None, None, None) {
                Ok(report) => return report,
                Err(error) if Instant::now() < deadline => {
                    eprintln!("corpus proof: retrying a stats query after {error}");
                }
                Err(error) => panic!("center did not answer a query in time: {error}"),
            }
        }
    }

    let query_started = Instant::now();
    let first_report = ctx_traits_io::center::stats(None, None, None)
        .expect("the first cold-corpus query must be answered, not refused or timed out");
    eprintln!(
        "corpus proof: first query answered in {:?} with {}/{CORPUS_LEDGER_COUNT} rows indexed",
        query_started.elapsed(),
        first_report.total_runs
    );
    assert!(
        first_report.total_runs < CORPUS_LEDGER_COUNT as u64,
        "an immediate query must observe genuinely partial progress, got {} of {CORPUS_LEDGER_COUNT}",
        first_report.total_runs
    );

    let completion_started = Instant::now();
    let completion_deadline = completion_started + Duration::from_secs(180);
    loop {
        let report = poll_stats_with_retry(completion_deadline);
        let last_seen = report.total_runs;
        if last_seen == CORPUS_LEDGER_COUNT as u64 {
            break;
        }
        assert!(
            Instant::now() < completion_deadline,
            "corpus indexing did not complete in time, last observed {last_seen} of {CORPUS_LEDGER_COUNT}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    eprintln!(
        "corpus proof: full corpus indexed {} after the first partial answer",
        format_args!("{:?}", completion_started.elapsed())
    );

    let physical_bytes = physical_index_bytes(&index);
    eprintln!("corpus proof: physical index size is {physical_bytes} bytes");
    assert!(
        physical_bytes <= 10_000_000,
        "the built index must stay metadata-only and small: {physical_bytes} bytes on disk"
    );

    child.0.kill().expect("stop corpus center");
    child.0.wait().expect("reap corpus center");
    let _ = std::fs::remove_dir_all(&root);
}

/// Same v1 schema shape as `center::tests::a_version_one_index_is_reprojected_by_the_current_center`,
/// scaled up to a genuinely large `center_sessions` payload rather than one
/// row — a small-payload `VACUUM` proves nothing about a real ~500MB index
/// on disk, which is what goal 1's migration half exists to catch.
fn build_v1_payload_bearing_index(index: &std::path::Path, ledgers: &[Utf8PathBuf]) -> u64 {
    let db = rusqlite::Connection::open(index).expect("create v1 index");
    db.execute_batch(
        "CREATE TABLE center_meta (version INTEGER NOT NULL); \
         CREATE TABLE center_rows (ledger TEXT PRIMARY KEY, repo_key TEXT NOT NULL, repo_path TEXT NOT NULL, mtime_secs INTEGER NOT NULL, mtime_nanos INTEGER NOT NULL, size INTEGER NOT NULL, summary TEXT NOT NULL); \
         CREATE TABLE center_sessions (ledger TEXT PRIMARY KEY, mtime_secs INTEGER NOT NULL, mtime_nanos INTEGER NOT NULL, size INTEGER NOT NULL, session TEXT NOT NULL);",
    )
    .expect("create v1 tables");
    // `CENTER_SCHEMA_VERSION` is a private center.rs constant; 1 is its only
    // value that has ever shipped, so it is safe to hardcode from this
    // external process-proof binary.
    db.execute("INSERT INTO center_meta(version) VALUES (1)", [])
        .expect("write v1 marker");
    // Payload per row large enough that 2,500 rows clear 600MB — its content
    // is irrelevant, since the migration drops this table outright rather
    // than reading it; only its on-disk size before reclamation matters.
    let payload = "x".repeat(CORPUS_LEDGER_FILLER_BYTES);
    db.execute_batch("BEGIN").expect("begin v1 fixture insert");
    for ledger in ledgers {
        let metadata = std::fs::metadata(ledger.as_std_path()).expect("stat corpus ledger");
        let secs = metadata
            .modified()
            .expect("ledger mtime")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("mtime after epoch")
            .as_secs() as i64;
        let size = i64::try_from(metadata.len()).expect("ledger size fits sqlite");
        let summary = serde_json::json!({
            "session_id": format!("corpus-session-{ledger}", ledger = ledger.file_stem().expect("ledger stem")),
            "run_id": "corpus-run",
            "trait_id": "corpus-trait",
            "status": "awaiting-agent-output",
            "has_merge_frames": false,
        })
        .to_string();
        db.execute(
            "INSERT INTO center_rows(ledger, repo_key, repo_path, mtime_secs, mtime_nanos, size, summary) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)",
            rusqlite::params![ledger.as_str(), "corpus", "/corpus", secs, 0, summary],
        )
        .expect("write v1 cached row");
        db.execute(
            "INSERT INTO center_sessions(ledger, mtime_secs, mtime_nanos, size, session) VALUES (?1, ?2, 0, ?3, ?4)",
            rusqlite::params![ledger.as_str(), secs, size, payload],
        )
        .expect("write v1 cached payload");
    }
    db.execute_batch("COMMIT")
        .expect("commit v1 fixture insert");
    drop(db);
    std::fs::metadata(index)
        .expect("stat v1 fixture index")
        .len()
}

#[test]
fn migrated_v1_index_over_the_corpus_is_reclaimed_to_the_same_physical_bound() {
    let _serial = SENTINEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let root = scratch("corpus-migration");
    std::fs::create_dir_all(&root).expect("create scratch root");
    // Ledgers themselves stay small here — goal 1's migration half binds the
    // pre-existing v1 INDEX's on-disk payload, not the ledger corpus size
    // (already proved by the fresh-index sibling test above); the empty
    // filler keeps fixture setup fast without weakening that bound.
    let ledgers: Vec<Utf8PathBuf> = (0..CORPUS_LEDGER_COUNT)
        .map(|index| write_corpus_ledger(&root, index, ""))
        .collect();

    let socket = root.join("center.sock");
    let index = root.join("index.sqlite3");
    let v1_bytes = build_v1_payload_bearing_index(&index, &ledgers);
    eprintln!("migration proof: v1 fixture index is {v1_bytes} bytes before takeover");
    assert!(
        v1_bytes >= 600_000_000,
        "the v1 fixture index must itself total at least 600MB, got {v1_bytes} bytes"
    );

    // Generous idle so idle exit is never the thing under test.
    let mut child = spawn_sentinel(&root, &socket, &index, "120000");
    let _environment = CenterEnvironment::install(&root);
    drop(await_socket(&socket));

    let completion_deadline = Instant::now() + Duration::from_secs(180);
    loop {
        match ctx_traits_io::center::stats(None, None, None) {
            Ok(report) if report.total_runs == CORPUS_LEDGER_COUNT as u64 => break,
            Ok(report) => eprintln!(
                "migration proof: {}/{CORPUS_LEDGER_COUNT} rows reprojected so far",
                report.total_runs
            ),
            Err(error) => eprintln!("migration proof: retrying a stats query after {error}"),
        }
        assert!(
            Instant::now() < completion_deadline,
            "the migrated corpus did not finish reprojecting in time"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    let physical_bytes = physical_index_bytes(&index);
    eprintln!(
        "migration proof: physical index size after takeover is {physical_bytes} bytes (was {v1_bytes})"
    );
    assert!(
        physical_bytes <= 10_000_000,
        "the migrated index must be reclaimed to the same metadata-only bound as a fresh build: {physical_bytes} bytes on disk"
    );

    child.0.kill().expect("stop migration center");
    child.0.wait().expect("reap migration center");
    let _ = std::fs::remove_dir_all(&root);
}
