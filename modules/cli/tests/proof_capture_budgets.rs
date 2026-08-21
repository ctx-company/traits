//! P489 behavioral proofs: a subprocess capture that exceeds its declared or
//! default ceiling refuses loudly, naming the cap, rather than silently
//! landing a truncated value in state that a later digest or receipt would
//! treat as complete. Covers both loud paths the runtime feeds forward
//! through: a default-input port's command (site A, `apply_default_inputs`)
//! and a command-step (site B, `run.rs`'s command-frame advance).
//!
//! Both fixtures use `head -c <n> /dev/zero`: deterministic, fast, and needs
//! no shell pipe (the declared `cmd` shorthand is argv-shape only, never a
//! shell string), so the oversized capture is produced directly by one
//! process rather than composed from several.

use std::fs;
use std::path::Path;
use std::process::Command;

use support::{ScratchRoot, git_init_on_branch, require_success, run_ctx, utf8};

fn commit_all(repo: &Path, message: &str) {
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-q", "-m", message])
        .current_dir(repo)
        .status()
        .unwrap();
}

fn review_and_activate(repo: &Path, home: &Path) {
    let fixture = ".ctx/traits/demo/generated/index.toml";
    require_success(
        "`ctx traits internal review --approve`",
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
        "`ctx traits state --active`",
        &["traits", "state", "--active", "--file", fixture],
        repo,
        home,
    );
    // `activate` mutates `trait.toml` in the invocation checkout; commit it so
    // a later clean-checkout precondition never parks this fixture.
    commit_all(repo, "activate");
}

fn write_trait_toml(repo: &Path) {
    fs::write(
        repo.join(".ctx/traits/demo/trait.toml"),
        "[package]\nid = \"demo\"\nversion = \"0.1.0\"\nname = \"Demo\"\nstatus = \"draft\"\n",
    )
    .unwrap();
}

/// A command-step fixture (site B) whose one step's own `cmd` is the
/// oversized-output command, declaring `capture-bytes` explicitly so the
/// test proves the *declared* per-command override is honored, not just the
/// hardcoded default.
fn init_command_step_fixture(repo: &Path, home: &Path, cmd: &str, capture_bytes: u64) {
    fs::create_dir_all(repo.join(".ctx/traits/demo/generated")).unwrap();
    git_init_on_branch(repo, "main");
    fs::write(repo.join(".gitignore"), ".ctx/traits/worktrees/\n").unwrap();
    fs::write(
        repo.join(".ctx/traits/demo/generated/index.toml"),
        format!(
            r#"id = "demo"
schema-version = "0.4"
version = "0.1.0"
name = "Demo"
description = "A provider-free command-only trait."

[procedure]
description = "Run one deterministic command."

[[slot]]
id = "notified"
schema = "schema:text"

[[procedure.sequence]]
id = "command"
title = "Run command"
kind = "command"
cmd = "{cmd}"
capture-bytes = {capture_bytes}
output = ["slot:notified"]
"#
        ),
    )
    .unwrap();
    write_trait_toml(repo);
    commit_all(repo, "init");
    review_and_activate(repo, home);
}

/// A default-input fixture (site A): one non-optional input `[[port]]`
/// declaring `default.command` as the oversized-output command, plus the
/// same trivial command step every other fixture here uses so the trait
/// stays otherwise well-formed.
fn init_default_input_fixture(repo: &Path, home: &Path, cmd: &str, capture_bytes: u64) {
    fs::create_dir_all(repo.join(".ctx/traits/demo/generated")).unwrap();
    git_init_on_branch(repo, "main");
    fs::write(repo.join(".gitignore"), ".ctx/traits/worktrees/\n").unwrap();
    fs::write(
        repo.join(".ctx/traits/demo/generated/index.toml"),
        format!(
            r#"id = "demo"
schema-version = "0.4"
version = "0.1.0"
name = "Demo"
description = "A provider-free command-only trait."

[[port]]
id = "big-input"
direction = "input"
schema = "schema:text"
description = "default command produces oversized output"

[port.default.command]
cmd = "{cmd}"
capture-bytes = {capture_bytes}

[procedure]
description = "Run one deterministic command."

[[slot]]
id = "notified"
schema = "schema:text"

[[procedure.sequence]]
id = "command"
title = "Run command"
kind = "command"
cmd = "true"
output = ["slot:notified"]
"#
        ),
    )
    .unwrap();
    write_trait_toml(repo);
    commit_all(repo, "init");
    review_and_activate(repo, home);
}

/// `ctx traits run` (with drive enabled, no `--no-drive`) executes a
/// runtime-only `kind = "command"` step itself as the trusted local
/// controlled runtime, so this exercises the actual command-frame advance in
/// `run.rs` rather than merely returning the frame contract for a caller to
/// execute out of band (which `--no-drive` does, and which never reaches the
/// truncation check at all).
#[test]
fn command_step_truncated_capture_parks_instead_of_landing_cut_off_state() {
    let scratch = ScratchRoot::new("p489-command-step-truncation");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_command_step_fixture(&repo, &scratch.home(), "head -c 300000 /dev/zero", 1024);

    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &scratch.home(),
    );
    let (stdout, stderr) = utf8(&output);
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        combined.contains("stdout-truncated: true"),
        "expected the command outcome to report stdout truncation: {combined}"
    );
    assert!(
        combined.contains("\"capture-bytes\": 1024"),
        "expected the declared 1024-byte cap to be the one actually applied: {combined}"
    );
    assert!(
        !combined.contains("\"final-session-status\": \"completed\""),
        "a truncated command-step capture must park the frame rather than complete the run: {combined}"
    );
    assert!(
        combined.contains("\"event\": \"command-step-failed\""),
        "expected a command-step-failed event rather than a silently-accepted truncated slot value: {combined}"
    );
}

/// The identical oversized command (300,000 bytes) succeeds when the
/// declared `capture-bytes` is raised above its output size — proving the
/// per-command knob itself, not merely that the refusal fires under the
/// default.
#[test]
fn command_step_succeeds_once_declared_capture_bytes_covers_the_output() {
    let scratch = ScratchRoot::new("p489-command-step-capture-override");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_command_step_fixture(&repo, &scratch.home(), "head -c 300000 /dev/zero", 400_000);

    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &scratch.home(),
    );
    let (stdout, stderr) = utf8(&output);
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        output.status.success(),
        "a command whose declared capture-bytes covers its output must succeed: {combined}"
    );
    assert!(
        combined.contains("\"final-session-status\": \"completed\""),
        "expected the run to complete once the declared cap covers the output: {combined}"
    );
}

#[test]
fn default_input_truncated_capture_aborts_the_run_with_no_slot_value() {
    let scratch = ScratchRoot::new("p489-default-input-truncation");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_default_input_fixture(&repo, &scratch.home(), "head -c 300000 /dev/zero", 1024);

    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
            "--no-drive",
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &scratch.home(),
    );
    assert_ne!(
        output.status.code(),
        Some(0),
        "an oversized default-input capture must not exit 0"
    );
    let (stdout, stderr) = utf8(&output);
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        combined.contains("port:big-input"),
        "expected the refused port to be named in the run output: {combined}"
    );
    assert!(
        combined.contains("1024"),
        "expected the declared 1024-byte cap named in the run output: {combined}"
    );
    // No run-session ledger should have been written: the refusal aborts
    // before any session exists, so this is not a stray absent-input warning
    // that still completed a session.
    assert!(
        !repo.join(".ctx/run").exists()
            || fs::read_dir(repo.join(".ctx/run"))
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(true),
        "a truncated default-input capture must not leave a run-session ledger behind"
    );
}
