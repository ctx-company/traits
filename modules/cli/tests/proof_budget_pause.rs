//! 0130 provider-free public-path proof: a run with a declared `[budget]
//! max-tokens` ceiling pauses typed as `paused-budget-exhausted` at the
//! frame-dispatch boundary — after the frame that pushed observed tokens
//! over the ceiling has already completed, never mid-frame — carrying
//! `budget-pause` evidence and per-model `tokens-by-model` ledger evidence.
//! Raising the ceiling and re-driving with the same `--session` resumes and
//! completes the run, proving the pause is a typed outcome, not a lock.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use support::{ScratchRoot, assert_exit_code, git_init, require_success, run_ctx, utf8};

const TRAIT_CANONICAL: &str = r#"id = "fixture-0130"
schema-version = "0.4"
version = "0.1.0"
name = "Fixture 0130"
description = "0130 budget-pause proof fixture."

[[agent]]
id = "worker"
description = "Stub worker for the 0130 budget-pause proof."
summary = "Fixture worker role."

[[slot]]
id = "answer1"
schema = "schema:text"
description = "First worker step's output."

[[slot]]
id = "answer2"
schema = "schema:text"
description = "Second worker step's output, reached only once the ceiling is raised."

[procedure]
description = "Two worker steps in sequence."

[[procedure.sequence]]
id = "answer-step"
title = "Produce fixture answer"
agent = "agent:worker"
prompt = "Produce a fixture text answer."
output = ["slot:answer1"]

[[procedure.sequence]]
id = "finish-step"
title = "Finish"
agent = "agent:worker"
prompt = "Produce a second fixture text answer."
output = ["slot:answer2"]
"#;

const TRAIT_MANIFEST: &str = r#"[package]
id = "fixture-0130"
version = "0.1.0"
name = "Fixture 0130"
status = "draft"
"#;

fn write_executable(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn ctx_toml(harness_id: &str, script: &Path, max_tokens: u64) -> String {
    let script = script.to_string_lossy().replace('\\', "\\\\");
    format!(
        r#"schema-version = "0.4"

[budget]
max-tokens = {max_tokens}

[harness.{harness_id}]
kind = "custom"
bin = "{script}"
transports = ["cli"]
version-probe = ["--fixture-probe"]

[harness.{harness_id}.cli]
argv = []
prompt-via = "stdin"
output = "raw-json"

[agent.role.worker]
harness = "{harness_id}"
transport = "cli"
session-mode = "per-frame"
"#
    )
}

fn init_fixture_repo(repo: &Path, home: &Path, harness_id: &str, script: &Path, max_tokens: u64) {
    fs::create_dir_all(repo.join(".ctx/traits/fixture-0130/generated")).unwrap();
    git_init(repo);
    fs::write(repo.join(".gitignore"), "ctx.toml\n.ctx/runs/\n").unwrap();
    fs::write(
        repo.join(".ctx/traits/runtime.toml"),
        ctx_toml(harness_id, script, max_tokens),
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/fixture-0130/trait.toml"),
        TRAIT_MANIFEST,
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/fixture-0130/generated/index.toml"),
        TRAIT_CANONICAL,
    )
    .unwrap();
    require_success(
        "0130-proof `ctx traits init`",
        &["traits", "init"],
        repo,
        home,
    );
    let fixture = ".ctx/traits/fixture-0130/generated/index.toml";
    require_success(
        "0130-proof `ctx traits internal review --approve`",
        &[
            "traits",
            "internal",
            "review",
            "--file",
            fixture,
            "--approve",
        ],
        repo,
        home,
    );
    require_success(
        "0130-proof `ctx traits state --active`",
        &["traits", "state", "--active", "--file", fixture],
        repo,
        home,
    );
}

fn value_json(output: &std::process::Output) -> serde_json::Value {
    let (stdout, stderr) = utf8(output);
    let start = stdout
        .lines()
        .position(|line| line.trim_start().starts_with('{'));
    let json_text = match start {
        Some(index) => stdout.lines().skip(index).collect::<Vec<_>>().join("\n"),
        None => stdout.clone(),
    };
    serde_json::from_str(&json_text).unwrap_or_else(|error| {
        panic!("stdout was not a JSON envelope: {error}\nstdout:\n{stdout}\nstderr:\n{stderr}")
    })
}

#[test]
fn declared_token_ceiling_pauses_typed_at_the_boundary_then_resumes_on_raise() {
    let scratch = ScratchRoot::new("p0130-budget-pause");
    let repo = scratch.home().join("repo");
    let home = scratch.home();
    let ledger = repo.join(".ctx/runs/fixture.json");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();

    let script = home.join("worker.sh");
    let log_dir = repo.join(".ctx/debug/prompts");
    write_executable(
        &script,
        &format!(
            r#"#!/bin/sh
if [ "$1" = "--fixture-probe" ]; then
  printf 'fixture-worker-1.0\n'
  exit 0
fi
mkdir -p "{log_dir}"
COUNT=$(ls "{log_dir}" 2>/dev/null | wc -l | tr -d ' ')
cat > "{log_dir}/prompt-$COUNT.txt"
if [ "$COUNT" = "0" ]; then
  printf '{{"answer1":"done","usage":{{"output_tokens":100}}}}'
  exit 0
fi
printf '{{"answer2":"done2"}}'
"#,
            log_dir = log_dir.display(),
        ),
    );

    // A ceiling well below the 100 tokens the first frame reports, so the
    // SECOND frame's dispatch (never the first, which has already
    // completed) is the one that pauses.
    init_fixture_repo(&repo, &home, "budget-worker", &script, 50);

    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/fixture-0130/generated/index.toml",
            "--out",
            &ledger.to_string_lossy(),
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    );
    assert_exit_code(&output, 0);
    let envelope = value_json(&output);
    let report = envelope["value"]["drive"].clone();
    assert_eq!(
        report["status"], "paused-budget-exhausted",
        "report: {report}"
    );

    // The first frame completed (its answer was accepted) before the pause
    // fired on the SECOND frame's dispatch — never mid-frame.
    let harness_runs = report["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["event"] == "harness-run")
        .count();
    assert_eq!(
        harness_runs, 1,
        "only the first frame's harness attempt must have dispatched: {report}"
    );

    let pause = &report["budget-pause"];
    assert_eq!(pause["ceiling-kind"], "tokens", "pause: {pause}");
    assert_eq!(pause["ceiling"], 50.0, "pause: {pause}");
    assert!(
        pause["observed"].as_f64().unwrap() >= 100.0,
        "pause: {pause}"
    );
    assert_eq!(pause["role"], serde_json::Value::Null, "pause: {pause}");

    let tokens_by_model = &report["tokens-by-model"];
    assert_eq!(tokens_by_model["unknown"], 100, "report: {report}");

    // Raising the cap (a config edit, not a new resume mechanism) and
    // re-driving with the same `--session` resumes past the pause.
    fs::write(
        repo.join(".ctx/traits/runtime.toml"),
        ctx_toml("budget-worker", &script, 1000),
    )
    .unwrap();
    let resumed = run_ctx(
        &[
            "traits",
            "internal",
            "drive",
            "--session",
            &ledger.to_string_lossy(),
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    );
    assert_exit_code(&resumed, 0);
    let resumed_report = value_json(&resumed)["value"].clone();
    assert_eq!(
        resumed_report["status"], "completed",
        "resumed report: {resumed_report}"
    );
}

#[test]
fn disk_floor_parks_before_dispatch_then_resumes_same_worktree() {
    let scratch = ScratchRoot::new("p0280-disk-floor");
    let repo = scratch.home().join("repo");
    let home = scratch.home();
    let ledger = repo.join(".ctx/runs/fixture.json");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    let script = home.join("worker.sh");
    let log_dir = repo.join(".ctx/debug/prompts");
    write_executable(
        &script,
        &format!(
            r#"#!/bin/sh
if [ "$1" = "--fixture-probe" ]; then
  printf 'fixture-worker-1.0\n'
  exit 0
fi
mkdir -p "{log_dir}"
COUNT=$(ls "{log_dir}" 2>/dev/null | wc -l | tr -d ' ')
touch "{log_dir}/prompt-$COUNT.txt"
if [ "$COUNT" = "0" ]; then
  printf '{{"answer1":"done"}}'
else
  printf '{{"answer2":"done2"}}'
fi
"#,
            log_dir = log_dir.display(),
        ),
    );
    init_fixture_repo(&repo, &home, "disk-worker", &script, 1000);
    let config = repo.join(".ctx/traits/runtime.toml");
    fs::write(
        &config,
        format!(
            "{}\n[worktree]\nenabled = true\n\n[worktree.retention]\ndisk-floor-mb = 1073741824\n",
            ctx_toml("disk-worker", &script, 1000)
        ),
    )
    .unwrap();
    let add = Command::new("git")
        .args(["add", "."])
        .current_dir(&repo)
        .status()
        .expect("stage worktree fixture");
    assert!(add.success(), "stage worktree fixture");
    let commit = Command::new("git")
        .args([
            "-c",
            "user.name=ctx test",
            "-c",
            "user.email=ctx@example.test",
            "commit",
            "-m",
            "fixture",
        ])
        .current_dir(&repo)
        .status()
        .expect("commit worktree fixture");
    assert!(commit.success(), "commit worktree fixture");

    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/fixture-0130/generated/index.toml",
            "--out",
            &ledger.to_string_lossy(),
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    );
    assert_exit_code(&output, 0);
    let report = value_json(&output)["value"]["drive"].clone();
    assert_eq!(report["status"], "disk-full", "report: {report}");
    assert_eq!(
        report["disk-full-park"]["floor-mb"], 1_073_741_824u64,
        "report: {report}"
    );
    assert_eq!(
        report["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["event"] == "harness-run")
            .count(),
        0,
        "the parked frame must not dispatch: {report}"
    );
    let parked = ctx_traits_io::run_session::read_run_session(
        camino::Utf8Path::from_path(&ledger).expect("UTF-8 ledger"),
    )
    .expect("read parked ledger");
    let worktree = parked
        .provenance
        .worktree
        .clone()
        .expect("park retains a worktree");
    assert!(
        Path::new(worktree.path.as_deref().expect("worktree path")).exists(),
        "parked worktree remains available"
    );
    assert!(
        parked.next_frame.is_some(),
        "parked frame remains unsubmitted"
    );
    assert_eq!(
        parked
            .last_drive_outcome
            .as_ref()
            .map(|outcome| outcome.outcome.as_str()),
        Some("disk-full")
    );
    assert!(
        parked
            .last_drive_outcome
            .as_ref()
            .and_then(|outcome| outcome.disk_full.as_ref())
            .is_some(),
        "typed disk evidence is durable"
    );

    fs::write(
        &config,
        format!(
            "{}\n[worktree]\nenabled = true\n\n[worktree.retention]\ndisk-floor-mb = 1\n",
            ctx_toml("disk-worker", &script, 1000)
        ),
    )
    .unwrap();
    let resumed = run_ctx(
        &[
            "traits",
            "internal",
            "drive",
            "--session",
            &ledger.to_string_lossy(),
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    );
    assert_exit_code(&resumed, 0);
    assert_eq!(value_json(&resumed)["value"]["status"], "completed");
    let completed = ctx_traits_io::run_session::read_run_session(
        camino::Utf8Path::from_path(&ledger).expect("UTF-8 ledger"),
    )
    .expect("read completed ledger");
    assert_eq!(
        completed.provenance.worktree.as_ref(),
        Some(&worktree),
        "resume must retain the parked worktree identity, branch, and path"
    );
}

#[test]
fn disk_floor_does_not_rewrite_completed_session() {
    let scratch = ScratchRoot::new("p0280-disk-floor-completed");
    let repo = scratch.home().join("repo");
    let home = scratch.home();
    let ledger = repo.join(".ctx/runs/fixture.json");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    let script = home.join("worker.sh");
    write_executable(
        &script,
        r##"#!/bin/sh
if [ "$1" = "--fixture-probe" ]; then
  printf 'fixture-worker-1.0\n'
  exit 0
fi
printf '{"answer1":"done","answer2":"done2"}'
"##,
    );
    init_fixture_repo(&repo, &home, "disk-completed-worker", &script, 1000);

    let completed = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/fixture-0130/generated/index.toml",
            "--out",
            &ledger.to_string_lossy(),
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    );
    assert_exit_code(&completed, 0);
    assert_eq!(
        value_json(&completed)["value"]["drive"]["status"],
        "completed"
    );

    fs::write(
        repo.join(".ctx/traits/runtime.toml"),
        format!(
            "{}\n[worktree.retention]\ndisk-floor-mb = 1073741824\n",
            ctx_toml("disk-completed-worker", &script, 1000)
        ),
    )
    .unwrap();
    let redriven = run_ctx(
        &[
            "traits",
            "internal",
            "drive",
            "--session",
            &ledger.to_string_lossy(),
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    );
    assert_exit_code(&redriven, 0);
    assert_eq!(value_json(&redriven)["value"]["status"], "completed");
    let session = ctx_traits_io::run_session::read_run_session(
        camino::Utf8Path::from_path(&ledger).expect("UTF-8 ledger"),
    )
    .expect("read completed ledger");
    assert_eq!(
        session
            .last_drive_outcome
            .as_ref()
            .map(|outcome| outcome.outcome.as_str()),
        Some("completed")
    );
}

/// The ceiling is RUN-cumulative, not per-invocation: a fresh `drive`
/// invocation's own token accumulator starts at zero, so without carrying
/// the prior drive's observed tokens forward, a resume with the ceiling left
/// UNCHANGED would wrongly dispatch another frame (each invocation re-earns
/// its own private allowance). Re-driving with the same ceiling must instead
/// pause again immediately, dispatching nothing.
#[test]
fn resuming_without_raising_the_ceiling_pauses_again_without_dispatching() {
    let scratch = ScratchRoot::new("p0130-budget-pause-no-raise");
    let repo = scratch.home().join("repo");
    let home = scratch.home();
    let ledger = repo.join(".ctx/runs/fixture.json");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();

    let script = home.join("worker.sh");
    let log_dir = repo.join(".ctx/debug/prompts");
    write_executable(
        &script,
        &format!(
            r#"#!/bin/sh
if [ "$1" = "--fixture-probe" ]; then
  printf 'fixture-worker-1.0\n'
  exit 0
fi
mkdir -p "{log_dir}"
COUNT=$(ls "{log_dir}" 2>/dev/null | wc -l | tr -d ' ')
cat > "{log_dir}/prompt-$COUNT.txt"
if [ "$COUNT" = "0" ]; then
  printf '{{"answer1":"done","usage":{{"output_tokens":100}}}}'
  exit 0
fi
printf '{{"answer2":"done2"}}'
"#,
            log_dir = log_dir.display(),
        ),
    );

    init_fixture_repo(&repo, &home, "budget-worker", &script, 50);

    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/fixture-0130/generated/index.toml",
            "--out",
            &ledger.to_string_lossy(),
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    );
    assert_exit_code(&output, 0);
    let report = value_json(&output)["value"]["drive"].clone();
    assert_eq!(
        report["status"], "paused-budget-exhausted",
        "report: {report}"
    );

    // Re-drive with the SAME `ctx.toml` (ceiling left at 50, well below the
    // 100 tokens already observed) — a fresh invocation's own accumulator
    // must be seeded from the prior drive's persisted evidence, so this
    // pauses again typed at the boundary instead of dispatching the second
    // frame.
    let resumed = run_ctx(
        &[
            "traits",
            "internal",
            "drive",
            "--session",
            &ledger.to_string_lossy(),
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    );
    assert_exit_code(&resumed, 0);
    let resumed_report = value_json(&resumed)["value"].clone();
    assert_eq!(
        resumed_report["status"], "paused-budget-exhausted",
        "resumed report: {resumed_report}"
    );
    let harness_runs = resumed_report["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["event"] == "harness-run")
        .count();
    assert_eq!(
        harness_runs, 0,
        "the resumed invocation must dispatch nothing before re-pausing: {resumed_report}"
    );
    assert_eq!(
        resumed_report["tokens-by-model"]["unknown"], 100,
        "carried-forward evidence must not double count the first drive's tokens: {resumed_report}"
    );
}
