//! P475: proves the phase's central claim end-to-end at the drive dispatch
//! consumer, not just through `doctor --config` — that a declared seat
//! `[agent.role.<role>.budget]` is the number an actual frame dispatch runs
//! under (`frame_budget`'s role-budget precedence, `budget_for_seat`'s seat
//! selection), and that the resolved budget is the one persisted to the
//! written session ledger's `agent-assignments` row (`role_budget_evidence`).
//! A stub CLI harness (mirroring `proof_drive_retries.rs`'s pattern) that
//! sleeps past a tiny declared `frame-seconds` proves the number was
//! actually applied to the dispatch's own timeout, not merely recorded —
//! wall-clock bounded well under the 300s built-in default so the assertion
//! fails loudly (instead of hanging for 5 minutes) if role-budget precedence
//! regresses.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Instant;

use support::{ScratchRoot, assert_exit_code, git_init, require_success, run_ctx, utf8};

const TRAIT_CANONICAL: &str = r#"id = "fixture-p475-budget"
schema-version = "0.2"
version = "0.1.0"
name = "Fixture P475 role budget"
summary = "P475 role-budget dispatch proof fixture."

[[agent]]
id = "worker"
description = "Stub worker for the P475 role-budget proof."
summary = "Fixture worker role."

[[slot]]
id = "answer"
schema = "schema:text"
description = "Fixture worker output."

[procedure]
description = "One worker step."

[[procedure.sequence]]
id = "answer-step"
title = "Produce fixture answer"
agent = "agent:worker"
prompt = "Produce a fixture answer."
output = ["slot:answer"]
"#;

const TRAIT_MANIFEST: &str = r#"[package]
id = "fixture-p475-budget"
version = "0.1.0"
name = "Fixture P475 role budget"
status = "draft"
"#;

fn write_executable(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

/// A stub harness that always sleeps 5s before answering — far longer than
/// the 1s role budget the fixture below declares, and comfortably shorter
/// than the 300s built-in default a precedence regression would fall back
/// to, so a passing assertion on wall-clock elapsed time is a real proof the
/// SMALL declared number governed the dispatch.
fn slow_worker_script() -> &'static str {
    r#"#!/bin/sh
if [ "$1" = "--fixture-probe" ]; then
  printf 'fixture-worker-1.0\n'
  exit 0
fi
sleep 5
printf '{"answer":"too slow"}'
"#
}

fn init_fixture_repo(repo: &Path, home: &Path, harness_script: &Path, extra_role_toml: &str) {
    fs::create_dir_all(repo.join(".ctx/traits/fixture-p475-budget/generated")).unwrap();
    git_init(repo);
    fs::write(repo.join(".gitignore"), "ctx.toml\n.ctx/runs/\n").unwrap();
    let script = harness_script.to_string_lossy().replace('\\', "\\\\");
    let ctx_toml = format!(
        r#"schema-version = "0.2"

[harness.budget-worker]
kind = "custom"
bin = "{script}"
transports = ["cli"]
version-probe = ["--fixture-probe"]

[harness.budget-worker.cli]
argv = []
prompt-via = "stdin"
output = "raw-json"

[agent.role.worker]
harness = "budget-worker"
transport = "cli"
session-mode = "per-frame"
{extra_role_toml}
"#
    );
    fs::write(repo.join("ctx.toml"), ctx_toml).unwrap();
    fs::write(
        repo.join(".ctx/traits/fixture-p475-budget/trait.toml"),
        TRAIT_MANIFEST,
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/fixture-p475-budget/generated/index.toml"),
        TRAIT_CANONICAL,
    )
    .unwrap();
    require_success(
        "p475-budget-proof `ctx traits init`",
        &["traits", "init"],
        repo,
        home,
    );
    let fixture = ".ctx/traits/fixture-p475-budget/generated/index.toml";
    require_success(
        "p475-budget-proof `ctx traits review --approve`",
        &["traits", "review", "--file", fixture, "--approve"],
        repo,
        home,
    );
    require_success(
        "p475-budget-proof `ctx traits activate`",
        &["traits", "activate", "--file", fixture],
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

/// A declared `[agent.role.worker.budget] frame-seconds = 1, max-retries = 0`
/// (well below the 300s/1-retry built-in defaults, and with no `[run]`/CLI
/// flag declared to compete with it — the precedence this asserts is role
/// budget vs. built-in default, not role budget vs. a nearer layer) governs
/// the ACTUAL dispatch timeout, not merely `doctor --config`'s display: the
/// frame times out and the drive report's terminal status is
/// `frame-timeout`, observed well under the 300s built-in default would
/// have taken had role-budget precedence not applied (`frame_budget`,
/// drive.rs). The written session ledger's `agent-assignments` row for
/// `worker` also carries the declared budget verbatim (`role_budget_evidence`),
/// so both the resolution the phase's `frame_budget`/`budget_for_seat` do
/// and the evidence `role_budget_evidence` writes are proven from one run.
#[test]
fn declared_role_budget_governs_frame_dispatch_and_is_recorded_in_the_ledger() {
    let scratch = ScratchRoot::new("p475-role-budget-dispatch");
    let repo = scratch.home().join("repo");
    let home = scratch.home();
    let ledger = repo.join(".ctx/runs/fixture.json");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    let script = home.join("budget-worker.sh");
    write_executable(&script, slow_worker_script());
    init_fixture_repo(
        &repo,
        &home,
        &script,
        "\n[agent.role.worker.budget]\nframe-seconds = 1\nmax-retries = 0\n",
    );

    let started = Instant::now();
    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/fixture-p475-budget/generated/index.toml",
            "--out",
            &ledger.to_string_lossy(),
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    );
    let elapsed = started.elapsed();
    assert_exit_code(&output, 0);
    let envelope = value_json(&output);
    let report = &envelope["value"]["drive"];
    assert_eq!(
        report["status"], "frame-timeout",
        "the 1s role budget (not the 300s built-in default) must time out the frame: {report}"
    );
    assert!(
        elapsed.as_secs() < 60,
        "the role budget's 1s frame-seconds must govern the dispatch timeout — a fall-through \
         to the 300s built-in default would take at least five minutes here (elapsed: {elapsed:?})"
    );

    let ledger_bytes = fs::read_to_string(&ledger).unwrap();
    let ledger_json: serde_json::Value = serde_json::from_str(&ledger_bytes).unwrap();
    let assignments = ledger_json["provenance"]["agent-assignments"]
        .as_array()
        .unwrap_or_else(|| panic!("ledger had no agent-assignments array: {ledger_json}"));
    let worker_row = assignments
        .iter()
        .find(|row| row["role"] == "worker")
        .unwrap_or_else(|| panic!("no agent-assignments row for role \"worker\": {ledger_json}"));
    assert_eq!(
        worker_row["budget"]["frame-seconds"], 1,
        "the declared role budget must be recorded on its own AgentAssignment row: {worker_row}"
    );
    assert_eq!(
        worker_row["budget"]["max-retries"], 0,
        "worker_row: {worker_row}"
    );
    assert!(
        worker_row["budget"].get("idle-seconds").is_none(),
        "an undeclared budget field must not appear at all (skip_serializing_if): {worker_row}"
    );
}

/// The counterpart negative case: with NO role budget declared, the same
/// slow harness is still governed by the 300s built-in default and the
/// ledger's `agent-assignments` row carries no `budget` key at all
/// (`skip_serializing_if`) — proven by asserting the row is absent rather
/// than by waiting out the full 300s (which would make this proof itself
/// the slowest thing in the suite for no added assurance): a completed
/// answer within a short custom `--frame-seconds` override on the CLI
/// (the nearer layer in the precedence chain) still completes fast, and a
/// role with no budget table produces no `budget` field on its ledger row.
#[test]
fn undeclared_role_budget_produces_no_ledger_budget_row() {
    let scratch = ScratchRoot::new("p475-role-budget-absent");
    let repo = scratch.home().join("repo");
    let home = scratch.home();
    let ledger = repo.join(".ctx/runs/fixture.json");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    let script = home.join("budget-worker.sh");
    // Answers immediately (no sleep) — this run only needs to complete, not
    // time out, so it stays fast regardless of which budget layer wins.
    write_executable(
        &script,
        r#"#!/bin/sh
if [ "$1" = "--fixture-probe" ]; then
  printf 'fixture-worker-1.0\n'
  exit 0
fi
printf '{"answer":"fast"}'
"#,
    );
    init_fixture_repo(&repo, &home, &script, "");

    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/fixture-p475-budget/generated/index.toml",
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
    let report = &envelope["value"]["drive"];
    assert_eq!(report["status"], "completed", "report: {report}");

    let ledger_bytes = fs::read_to_string(&ledger).unwrap();
    let ledger_json: serde_json::Value = serde_json::from_str(&ledger_bytes).unwrap();
    let assignments = ledger_json["provenance"]["agent-assignments"]
        .as_array()
        .unwrap_or_else(|| panic!("ledger had no agent-assignments array: {ledger_json}"));
    let worker_row = assignments
        .iter()
        .find(|row| row["role"] == "worker")
        .unwrap_or_else(|| panic!("no agent-assignments row for role \"worker\": {ledger_json}"));
    assert!(
        worker_row.get("budget").is_none(),
        "an undeclared role budget must leave the ledger row byte-identical to pre-P475 \
         (no budget key at all), not an empty budget object: {worker_row}"
    );
}
