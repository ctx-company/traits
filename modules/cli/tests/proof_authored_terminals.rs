//! 0189 behavioral proofs: authored terminals (`flow.error` / `flow.success`)
//! end the run at the exit point — an error terminal fails the run with the
//! authored message and its typed record on the reserved `flow-error` port,
//! a success terminal completes it with the exit's port bindings, a trait
//! declaring a success terminal that falls through every exit ends as the
//! authored `no-exit-reached` failure, and the divergence-aware
//! produced-before-read join admits reads guaranteed only on surviving
//! paths. All fixtures are hand-written canonicals in scratch repos — no
//! committed content, no agents, command steps only.

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
        "`ctx traits internal state --active`",
        &["traits", "internal", "state", "--active", "--file", fixture],
        repo,
        home,
    );
    commit_all(repo, "activate");
}

fn init_fixture(repo: &Path, home: &Path, canonical: &str) {
    fs::create_dir_all(repo.join(".ctx/traits/demo/generated")).unwrap();
    git_init_on_branch(repo, "main");
    fs::write(repo.join(".gitignore"), ".ctx/traits/worktrees/\n").unwrap();
    fs::write(
        repo.join(".ctx/traits/demo/generated/index.toml"),
        canonical,
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/demo/trait.toml"),
        "[package]\nid = \"demo\"\nversion = \"0.1.0\"\nname = \"Demo\"\nstatus = \"draft\"\n",
    )
    .unwrap();
    commit_all(repo, "init");
    review_and_activate(repo, home);
}

fn run_demo(repo: &Path, home: &Path) -> (String, bool) {
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
        repo,
        home,
    );
    let (stdout, stderr) = utf8(&output);
    (format!("{stdout}\n{stderr}"), output.status.success())
}

/// An error-ladder fixture: a command step measures a verdict, a sibling
/// branch routes on it into an error terminal, and a marker step follows the
/// branch. `verdict_cmd` decides which path a run takes.
fn error_ladder_canonical(verdict_cmd: &str) -> String {
    format!(
        r#"id = "demo"
schema-version = "0.4"
version = "0.1.0"
name = "Demo"
description = "Authored error terminal fixture."

[procedure]
description = "Measure a verdict, error out when it says revise."

[[slot]]
id = "verdict"
schema = "schema:text"

[[slot]]
id = "after-out"
schema = "schema:text"

[sequence.reject-arm]
[[sequence.reject-arm.sequence]]
id = "reject-exit"
title = "Version bump rejected"
kind = "terminal"
outcome = "error"
message = "Version bump rejected by review"

[[sequence.reject-arm.sequence.payload]]
destination = "flow-error"
source = "slot:verdict"

[[procedure.sequence]]
id = "measure"
title = "Measure the verdict"
kind = "command"
cmd = "{verdict_cmd}"
output = ["slot:verdict"]

[[procedure.sequence]]
id = "rung-reject"
title = "Reject rung"
kind = "branch"
sequence = "sequence:reject-arm"
when = {{ slot = "slot:verdict", equals = "revise" }}

[[procedure.sequence]]
id = "marker-after-branch"
title = "Marker after the branch"
kind = "command"
cmd = "printf MARKER-RAN"
input = ["slot:verdict"]
output = ["slot:after-out"]
"#
    )
}

#[test]
fn error_terminal_fails_the_run_with_authored_reason_and_typed_record() {
    let scratch = ScratchRoot::new("terminal-error-guard-true");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_fixture(
        &repo,
        &scratch.home(),
        &error_ladder_canonical("printf revise"),
    );

    let (combined, success) = run_demo(&repo, &scratch.home());
    assert!(
        !success,
        "an authored error terminal must end the run nonzero: {combined}"
    );
    assert!(
        combined.contains("authored-error"),
        "stop reason must carry the authored-error kind: {combined}"
    );
    assert!(
        combined.contains("Version bump rejected by review"),
        "the authored message must surface in the report: {combined}"
    );
    assert!(
        combined.contains("flow-error"),
        "the typed record must land on the reserved flow-error port: {combined}"
    );
    // The embedded canonical carries the cmd text; only an ACCEPTED value
    // proves execution.
    assert!(
        !combined.contains("\"value\": \"MARKER-RAN\""),
        "no step after the taken exit may run: {combined}"
    );
}

#[test]
fn error_terminal_guard_false_proceeds_untouched() {
    let scratch = ScratchRoot::new("terminal-error-guard-false");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_fixture(
        &repo,
        &scratch.home(),
        &error_ladder_canonical("printf approved"),
    );

    let (combined, success) = run_demo(&repo, &scratch.home());
    assert!(
        success,
        "a false guard must leave the run untouched by the terminal: {combined}"
    );
    assert!(
        combined.contains("\"final-session-status\": \"completed\""),
        "the no-terminal path must complete normally: {combined}"
    );
    assert!(
        combined.contains("\"value\": \"MARKER-RAN\""),
        "the step after the branch must run on the surviving path: {combined}"
    );
}

/// A success-exit fixture: the exit binds a declared output port; falling
/// through (guard false) must FAIL as `no-exit-reached`, because declaring a
/// success terminal opts the trait into exit-point completion.
fn success_exit_canonical(verdict_cmd: &str) -> String {
    format!(
        r#"id = "demo"
schema-version = "0.4"
version = "0.1.0"
name = "Demo"
description = "Authored success terminal fixture."

[procedure]
description = "Measure a verdict, succeed only at the explicit exit."

[[slot]]
id = "verdict"
schema = "schema:text"

[[port]]
id = "release-report"
direction = "output"
schema = "schema:text"
description = "Bound at the success exit."

[sequence.release-arm]
[[sequence.release-arm.sequence]]
id = "release-exit"
title = "Released"
kind = "terminal"
outcome = "success"
message = "Release complete"

[[sequence.release-arm.sequence.payload]]
destination = "release-report"
source = "slot:verdict"

[[procedure.sequence]]
id = "measure"
title = "Measure the verdict"
kind = "command"
cmd = "{verdict_cmd}"
output = ["slot:verdict"]

[[procedure.sequence]]
id = "rung-release"
title = "Release rung"
kind = "branch"
sequence = "sequence:release-arm"
when = {{ slot = "slot:verdict", equals = "ok" }}
"#
    )
}

#[test]
fn success_terminal_completes_with_exit_bindings() {
    let scratch = ScratchRoot::new("terminal-success-guard-true");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_fixture(&repo, &scratch.home(), &success_exit_canonical("printf ok"));

    let (combined, success) = run_demo(&repo, &scratch.home());
    assert!(
        success,
        "a taken success exit must complete the run: {combined}"
    );
    assert!(
        combined.contains("authored-success"),
        "stop reason must carry the authored-success kind: {combined}"
    );
    assert!(
        combined.contains("Release complete"),
        "the authored message must surface: {combined}"
    );
    assert!(
        combined.contains("release-report"),
        "the exit must bind the declared output port: {combined}"
    );
}

#[test]
fn fall_through_past_every_success_exit_is_an_authored_failure() {
    let scratch = ScratchRoot::new("terminal-no-exit-reached");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_fixture(
        &repo,
        &scratch.home(),
        &success_exit_canonical("printf meh"),
    );

    let (combined, success) = run_demo(&repo, &scratch.home());
    assert!(
        !success,
        "declaring a success terminal makes fall-through a failure: {combined}"
    );
    assert!(
        combined.contains("no-exit-reached"),
        "the fall-through failure must carry the no-exit-reached kind: {combined}"
    );
}

/// Divergence-aware join: the then arm DIVERGES (error terminal), the
/// otherwise arm produces a slot, and a step after the branch reads it. The
/// old intersection join dropped the otherwise arm's product (the read was
/// refused at decode); the divergence-aware join admits it, and at runtime
/// the surviving path genuinely produced the value.
fn divergence_canonical() -> String {
    r#"id = "demo"
schema-version = "0.4"
version = "0.1.0"
name = "Demo"
description = "Divergence-aware join fixture."

[procedure]
description = "Then-arm diverges; the otherwise arm's product is readable after."

[[slot]]
id = "verdict"
schema = "schema:text"

[[slot]]
id = "survivor-out"
schema = "schema:text"

[[slot]]
id = "final-out"
schema = "schema:text"

[sequence.diverge-arm]
[[sequence.diverge-arm.sequence]]
id = "diverge-exit"
title = "Diverging exit"
kind = "terminal"
outcome = "error"
message = "diverged"

[sequence.survive-arm]
[[sequence.survive-arm.sequence]]
id = "produce-survivor"
title = "Produce the survivor value"
kind = "command"
cmd = "printf survivor"
output = ["slot:survivor-out"]

[[procedure.sequence]]
id = "measure"
title = "Measure the verdict"
kind = "command"
cmd = "printf keep-going"
output = ["slot:verdict"]

[[procedure.sequence]]
id = "rung-diverge"
title = "Diverge rung"
kind = "branch"
sequence = "sequence:diverge-arm"
otherwise = "sequence:survive-arm"
when = { slot = "slot:verdict", equals = "revise" }

[[procedure.sequence]]
id = "read-survivor"
title = "Read the surviving arm's product"
kind = "command"
cmd = "printf done"
input = ["slot:survivor-out"]
output = ["slot:final-out"]
"#
    .to_string()
}

#[test]
fn step_after_a_diverging_arm_may_read_the_surviving_arms_product() {
    let scratch = ScratchRoot::new("terminal-divergence-join");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_fixture(&repo, &scratch.home(), &divergence_canonical());

    let (combined, success) = run_demo(&repo, &scratch.home());
    assert!(
        success,
        "the divergence-aware join must admit the surviving arm's product and the run must complete: {combined}"
    );
    assert!(
        combined.contains("read-survivor"),
        "the post-branch reader must run on the surviving path: {combined}"
    );
}

fn decode_refusal(canonical: &str, scratch_name: &str) -> String {
    let scratch = ScratchRoot::new(scratch_name);
    let repo = scratch.home().join("repo");
    fs::create_dir_all(repo.join(".ctx/traits/demo/generated")).unwrap();
    git_init_on_branch(&repo, "main");
    fs::write(
        repo.join(".ctx/traits/demo/generated/index.toml"),
        canonical,
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/demo/trait.toml"),
        "[package]\nid = \"demo\"\nversion = \"0.1.0\"\nname = \"Demo\"\nstatus = \"draft\"\n",
    )
    .unwrap();
    commit_all(&repo, "init");
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
    assert!(
        !output.status.success(),
        "the malformed terminal fixture must refuse\nstdout: {stdout}\nstderr: {stderr}"
    );
    format!("{stdout}\n{stderr}")
}

#[test]
fn terminal_below_schema_version_0_4_is_refused() {
    let canonical = error_ladder_canonical("printf revise")
        .replace("schema-version = \"0.4\"", "schema-version = \"0.3\"");
    let combined = decode_refusal(&canonical, "terminal-schema-floor");
    assert!(
        combined.contains("0.4"),
        "the refusal must name the schema-version floor: {combined}"
    );
}

#[test]
fn error_payload_must_target_the_reserved_port() {
    let canonical = error_ladder_canonical("printf revise").replace(
        "destination = \"flow-error\"",
        "destination = \"somewhere-else\"",
    );
    let combined = decode_refusal(&canonical, "terminal-wrong-error-port");
    assert!(
        combined.contains("reserved"),
        "the refusal must name the reserved error port rule: {combined}"
    );
}

#[test]
fn a_value_backed_port_cannot_also_be_bound_at_an_exit() {
    let canonical = success_exit_canonical("printf ok").replace(
        "description = \"Bound at the success exit.\"",
        "description = \"Bound twice.\"\nvalue = \"slot:verdict\"",
    );
    let combined = decode_refusal(&canonical, "terminal-double-binding");
    assert!(
        combined.contains("cannot also be bound at an exit"),
        "the refusal must name the double-binding rule: {combined}"
    );
}
