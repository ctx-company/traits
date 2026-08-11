//! Task 0066.4: assist genuinely inherits the run's command and frame bounds
//! — a hung round is killed by the bound that actually fired, the report
//! names that bound and its configured value, the two legs (a command-step
//! kill vs. a frame-dispatch kill) are never reported as each other, the
//! killed round enters round evidence with the candidate preserved, and no
//! phantom session survives the kill.

use std::fs;
use std::path::Path;

use support::{ScratchRoot, ctx_bin, fixture_agent_bin, git_init, run_ctx, run_ctx_with_env, utf8};

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!("cannot create directory {}: {error}", parent.display())
        });
    }
    fs::write(path, contents)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
}

/// Recursively finds every `*.json` file under a directory literally named
/// `runs` anywhere below `root` — the session-ledger store's own file shape
/// (P426), so finding none there is the proof that no phantom session
/// survived the kill.
fn find_run_session_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("json")
                && path
                    .parent()
                    .and_then(|parent| parent.file_name())
                    .and_then(|name| name.to_str())
                    == Some("runs")
            {
                found.push(path);
            }
        }
    }
    found
}

fn trust_approve_generate_deps(proj: &Path, home: &Path) {
    for dependency in ["generate-trait", "trait-spec"] {
        let approve = run_ctx(&["traits", "trust", "approve", dependency], proj, home);
        assert!(
            approve.status.success(),
            "trust approve {dependency} failed: {}",
            utf8(&approve).1
        );
    }
}

fn path_with_ctx_bin() -> String {
    let ctx_dir = ctx_bin()
        .parent()
        .expect("ctx binary path has a parent directory")
        .to_path_buf();
    let existing_path = std::env::var("PATH").unwrap_or_default();
    format!("{}:{existing_path}", ctx_dir.display())
}

/// The command-bound leg: `evaluate-round`'s command step invokes `ctx
/// traits generate-round` as a subprocess, which drives the candidate
/// through the CDK build rung. A candidate whose module hangs at top level
/// (`while (true) {}`) makes that subprocess produce zero output for as long
/// as it spins — well within the CDK build's own 30s wall — so the
/// repository's `[run] command-idle-seconds` policy (reaching this step
/// exactly as an ordinary command step's would, proving inheritance) fires
/// first and kills it.
fn write_command_bound_ctx_toml(repo: &Path) {
    let agent = fixture_agent_bin()
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let ctx_toml = format!(
        r#"schema-version = "0.4"

[run]
command-idle-seconds = 2

[harness.stub-generator]
kind = "custom"
bin = "{agent}"
transports = ["cli"]
version-probe = ["--probe"]

[harness.stub-generator.cli]
argv = ["--role", "generator"]
output = "raw-json"

[agent.role.generator]
harness = "stub-generator"
transport = "cli"
session-mode = "per-frame"
"#
    );
    write_file(&repo.join("ctx.toml"), &ctx_toml);
}

#[test]
fn command_bound_kill_names_the_bound_and_preserves_the_candidate() {
    let scratch = ScratchRoot::new("assist-bound-kill-command");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    support::symlink_node_modules(&proj);
    write_command_bound_ctx_toml(&proj);
    trust_approve_generate_deps(&proj, &home);

    let trait_id = "assist-bound-kill-command-fixture";
    let output = run_ctx_with_env(
        &[
            "traits",
            "generate",
            "Assist Bound Kill Command Fixture",
            "Proves a hanging round is killed by the named command-idle-seconds bound.",
            "--json",
        ],
        &proj,
        &home,
        &[
            ("PATH", path_with_ctx_bin().as_str()),
            ("CTX_FIXTURE_GENERATOR_CANDIDATE", "while (true) {}\n"),
        ],
    );
    let (stdout, stderr) = utf8(&output);

    assert!(
        !output.status.success(),
        "a command-bound kill must exit non-zero\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("command-idle-seconds") || stderr.contains("command-idle-seconds"),
        "the report must name the fired bound by its config key\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("command-idle-seconds (2s)")
            || stderr.contains("command-idle-seconds (2s)"),
        "the report must name the bound's configured value\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("frame-seconds") && !stderr.contains("frame-seconds"),
        "a command-bound kill must never be reported as a frame-bound kill\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let evidence: serde_json::Value =
        serde_json::from_str(&stdout).expect("--json output must be one RoundEvidence document");
    assert_eq!(evidence["converged"], serde_json::json!(false));
    let rounds = evidence["rounds"]
        .as_array()
        .expect("evidence must carry a rounds array");
    assert!(
        !rounds.is_empty(),
        "the killed round must appear in round evidence: {evidence}"
    );
    let killed_round = rounds.last().expect("at least one round");
    assert_eq!(killed_round["converged"], serde_json::json!(false));
    let diagnostics = killed_round["diagnostics"]
        .as_array()
        .expect("the killed round must carry diagnostics");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == serde_json::json!("round-killed-by-bound")),
        "the killed round's diagnostic must name the round-killed-by-bound code: {killed_round}"
    );
    assert!(
        diagnostics.iter().any(|diagnostic| diagnostic["message"]
            .as_str()
            .is_some_and(|message| message.contains("command-idle-seconds"))),
        "the killed round's diagnostic must name the bound: {killed_round}"
    );

    let scratch_package = proj.join(format!(".ctx/traits/packages/{trait_id}-candidate"));
    assert!(
        scratch_package.join("source/index.ts").is_file(),
        "the scratch candidate must be preserved at {} after the kill",
        scratch_package.display()
    );

    let target_package = proj.join(format!(".ctx/traits/packages/{trait_id}"));
    assert!(
        !target_package.exists(),
        "a killed run must never write the target package at {}",
        target_package.display()
    );

    let stray_sessions = find_run_session_files(&home);
    assert!(
        stray_sessions.is_empty(),
        "no session ledger may survive a bound kill; found: {stray_sessions:?}"
    );
}

/// The frame-bound leg: the loop's own `produce-first` frame dispatch sleeps
/// past a small declared `[agent.role.generator.budget] frame-seconds`, so
/// the drive's frame ceiling — not any command bound — ends the run before a
/// single round is ever evaluated.
fn write_frame_bound_ctx_toml(repo: &Path) {
    let agent = fixture_agent_bin()
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let ctx_toml = format!(
        r#"schema-version = "0.4"

[run]
max-retries = 0

[harness.stub-generator]
kind = "custom"
bin = "{agent}"
transports = ["cli"]
version-probe = ["--probe"]

[harness.stub-generator.cli]
argv = ["--role", "generator"]
output = "raw-json"

[agent.role.generator]
harness = "stub-generator"
transport = "cli"
session-mode = "per-frame"

[agent.role.generator.budget]
frame-seconds = 2
"#
    );
    write_file(&repo.join("ctx.toml"), &ctx_toml);
}

#[test]
fn frame_bound_kill_names_the_bound_and_never_reads_as_a_command_kill() {
    let scratch = ScratchRoot::new("assist-bound-kill-frame");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    support::symlink_node_modules(&proj);
    write_frame_bound_ctx_toml(&proj);
    trust_approve_generate_deps(&proj, &home);

    let output = run_ctx_with_env(
        &[
            "traits",
            "generate",
            "Assist Bound Kill Frame Fixture",
            "Proves the frame ceiling — not a command bound — ends the run.",
            "--json",
        ],
        &proj,
        &home,
        &[
            ("PATH", path_with_ctx_bin().as_str()),
            ("CTX_FIXTURE_GENERATOR_SLEEP_MS", "5000"),
        ],
    );
    let (stdout, stderr) = utf8(&output);

    assert!(
        !output.status.success(),
        "a frame-bound kill must exit non-zero\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("frame-seconds (2s)") || stderr.contains("frame-seconds (2s)"),
        "the report must name the fired frame bound and its value\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("command-idle-seconds") && !stderr.contains("command-idle-seconds"),
        "a frame-bound kill must never be reported as a command-bound kill\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("command-seconds") && !stderr.contains("command-seconds"),
        "a frame-bound kill must never be reported as a command-bound kill\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let stray_sessions = find_run_session_files(&home);
    assert!(
        stray_sessions.is_empty(),
        "no session ledger may survive a bound kill; found: {stray_sessions:?}"
    );
}
