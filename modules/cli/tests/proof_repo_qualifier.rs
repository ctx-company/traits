//! P451: proves the repo qualifier is actually reachable at runtime, not
//! merely parsed and merged into a discarded intermediate document. Crosses
//! the seam a hand-built-map unit test over `flatten_agent_defaults`/
//! `merge_machine_config` cannot: the carried GLOBAL
//! `~/.config/ctx/config.toml`'s `[repo."<key>"]` block must survive
//! `resolve_config_report`'s machine-authoritative install onto the returned
//! runtime document, or `resolve_runtime_assignments_impl` (which reads
//! `runtime_config.repo`) and `doctor --config` never see it.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use support::{
    ScratchRoot, assert_exit_code, controlled_command, ctx_bin, git_init, require_success, run_ctx,
    utf8,
};

const TRAIT_CANONICAL: &str = r#"id = "fixture-p451-repo-smart"
schema-version = "0.4"
version = "0.1.0"
name = "Fixture P451 repo qualifier"
description = "P451 repo-qualifier dispatch proof fixture."

[[agent]]
id = "worker"
description = "Stub worker for the P451 repo-qualifier proof."
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
id = "fixture-p451-repo-smart"
version = "0.1.0"
name = "Fixture P451 repo qualifier"
status = "draft"
"#;

fn write_executable(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn fast_worker_script() -> &'static str {
    r#"#!/bin/sh
if [ "$1" = "--fixture-probe" ]; then
  printf 'fixture-worker-1.0\n'
  exit 0
fi
printf '{"answer":"fast"}'
"#
}

fn init_fixture_repo(repo: &Path, home: &Path, harness_script: &Path) {
    fs::create_dir_all(repo.join(".ctx/traits/authored/fixture-p451-repo-smart/generated"))
        .unwrap();
    git_init(repo);
    fs::write(repo.join(".gitignore"), "ctx.toml\n.ctx/runs/\n").unwrap();
    let script = harness_script.to_string_lossy().replace('\\', "\\\\");
    let ctx_toml = format!(
        r#"schema-version = "0.4"

[harness.repo-qualifier-worker]
kind = "custom"
bin = "{script}"
transports = ["cli"]
version-probe = ["--fixture-probe"]

[harness.repo-qualifier-worker.cli]
argv = []
prompt-via = "stdin"
output = "raw-json"
model-flag = "--model"

[agent.role.worker]
harness = "repo-qualifier-worker"
transport = "cli"
session-mode = "per-frame"
model = "base-model"
"#
    );
    fs::write(repo.join(".ctx/traits/runtime.toml"), ctx_toml).unwrap();
    fs::write(
        repo.join(".ctx/traits/authored/fixture-p451-repo-smart/trait.toml"),
        TRAIT_MANIFEST,
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/authored/fixture-p451-repo-smart/generated/index.toml"),
        TRAIT_CANONICAL,
    )
    .unwrap();
    require_success(
        "p451-repo-qualifier-proof `ctx traits init`",
        &["traits", "init"],
        repo,
        home,
    );
    let fixture = ".ctx/traits/authored/fixture-p451-repo-smart/generated/index.toml";
    require_success(
        "p451-repo-qualifier-proof `ctx traits internal review --approve`",
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
        "p451-repo-qualifier-proof `ctx traits internal state --active`",
        &["traits", "internal", "state", "--active", "--file", fixture],
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

/// Discover the P451 active repo-qualifier key `doctor --config` reports for
/// `repo` — the same key an operator would copy into `[repo."<key>"]` — by
/// running doctor before any `[repo.*]` block exists, so the discovery
/// itself never depends on the behavior under test.
fn active_repo_key(repo: &Path, home: &Path) -> String {
    let stdout = require_success(
        "p451-repo-qualifier-proof discover active repo key",
        &["traits", "doctor", "--config"],
        repo,
        home,
    );
    let line = stdout
        .lines()
        .find(|line| line.trim_start().starts_with("repo.active-key:"))
        .unwrap_or_else(|| panic!("doctor --config printed no repo.active-key row: {stdout}"));
    let after_colon = line.split_once(':').unwrap().1;
    after_colon
        .split('[')
        .next()
        .unwrap_or(after_colon)
        .trim()
        .to_string()
}

fn worker_row(ledger: &Path) -> serde_json::Value {
    let ledger_bytes = fs::read_to_string(ledger).unwrap();
    let ledger_json: serde_json::Value = serde_json::from_str(&ledger_bytes).unwrap();
    let assignments = ledger_json["provenance"]["agent-assignments"]
        .as_array()
        .unwrap_or_else(|| panic!("ledger had no agent-assignments array: {ledger_json}"))
        .clone();
    assignments
        .into_iter()
        .find(|row| row["role"] == "worker")
        .unwrap_or_else(|| panic!("no agent-assignments row for role \"worker\": {ledger_json}"))
}

/// Blocker `repo-qualifier-never-applied`, done-when (a)+(b): a
/// `[repo."<active-key>".agent.role.worker]` block in the carried global
/// config actually governs a real `ctx traits run`'s resolved model AND its
/// recorded evidence — and `doctor --config` renders the declared row —
/// proving the block survives `resolve_config_report`'s machine-install step
/// rather than being silently dropped.
#[test]
fn repo_qualifier_governs_dispatch_and_renders_in_doctor() {
    let scratch = ScratchRoot::new("p451-repo-qualifier-matching");
    let repo = scratch.home().join("repo");
    let home = scratch.home();
    let ledger = repo.join(".ctx/runs/fixture.json");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    let script = home.join("repo-qualifier-worker.sh");
    write_executable(&script, fast_worker_script());
    git_init(&repo);
    let active_key = active_repo_key(&repo, &home);

    let global_config = home.join("ctx").join("traits").join("runtime.toml");
    fs::create_dir_all(global_config.parent().unwrap()).unwrap();
    fs::write(
        &global_config,
        format!(
            "schema-version = \"0.2\"\n\n[repo.\"{active_key}\".agent.role.worker]\nmodel = \"personal-model\"\n"
        ),
    )
    .unwrap();

    init_fixture_repo(&repo, &home, &script);

    let personal_output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/authored/fixture-p451-repo-smart/generated/index.toml",
            "--out",
            &ledger.to_string_lossy(),
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    );
    assert_exit_code(&personal_output, 0);
    let personal_row = worker_row(&ledger);
    assert_eq!(
        personal_row["model"], "personal-model",
        "matching personal override must beat the repository default: {personal_row}"
    );

    let environment_config = home.join("environment.toml");
    fs::write(
        &environment_config,
        "[agent.role.worker]\nmodel = \"environment-model\"\n",
    )
    .unwrap();
    let mut command = controlled_command(
        &ctx_bin(),
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/authored/fixture-p451-repo-smart/generated/index.toml",
            "--out",
            &ledger.to_string_lossy(),
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    );
    command.env("CTX_CONFIG", &environment_config);
    let output = command.output().unwrap();
    assert_exit_code(&output, 0);
    let envelope = value_json(&output);
    let report = &envelope["value"]["drive"];
    assert_eq!(report["status"], "completed", "report: {report}");

    let row = worker_row(&ledger);
    assert_eq!(
        row["model"], "environment-model",
        "repository default < matching personal override < CTX_CONFIG: {row}"
    );
    let evidence = row["evidence"]
        .as_str()
        .unwrap_or_else(|| panic!("no evidence string on worker row: {row}"));
    assert!(
        evidence.contains(&format!("config-qualifier=repo:{active_key}")),
        "evidence must name the repo qualifier that won: {evidence}"
    );

    let doctor_stdout = require_success(
        "p451-repo-qualifier-proof doctor --config after declaring the block",
        &["traits", "doctor", "--config"],
        &repo,
        &home,
    );
    // What this proof is about is that the qualifier TAKES EFFECT and is
    // visible in the configuration report — not which of two layouts renders
    // it. The report names the winning block as one `repo.<key>.…` row, and
    // 0010's leaf-granular provenance work renders the same fact as per-leaf
    // rows carrying their own source instead. Pinning the block row made this
    // proof fail the landing gate for every run in that arc while the
    // qualifier itself worked exactly as asserted three lines above. Accept
    // either shape and keep the assertion on the effect.
    let block_row = format!("repo.{active_key}.agent.role.worker:");
    let leaf_row = "agent.role.worker.model: personal-model";
    assert!(
        doctor_stdout.contains(&block_row) || doctor_stdout.contains(leaf_row),
        "doctor --config must show the repo-qualified worker assignment, as \
         either `{block_row}` or `{leaf_row}`: {doctor_stdout}"
    );
}

/// Counterpart negative case: the identical `[repo."<key>"]` block declared
/// under a key that does NOT match this repo's own active key must be
/// completely inert — proving the qualifier is scoped per-repo, not merely
/// "any repo block present".
#[test]
fn repo_qualifier_is_inert_for_a_non_matching_key() {
    let scratch = ScratchRoot::new("p451-repo-qualifier-non-matching");
    let repo = scratch.home().join("repo");
    let home = scratch.home();
    let ledger = repo.join(".ctx/runs/fixture.json");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    let script = home.join("repo-qualifier-worker.sh");
    write_executable(&script, fast_worker_script());

    let global_config = home.join("ctx").join("traits").join("runtime.toml");
    fs::create_dir_all(global_config.parent().unwrap()).unwrap();
    fs::write(
        &global_config,
        "schema-version = \"0.2\"\n\n[repo.\"totally-unrelated-key-00000000\".agent.role.worker]\nmodel = \"repo-scoped-model\"\n",
    )
    .unwrap();

    init_fixture_repo(&repo, &home, &script);

    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/authored/fixture-p451-repo-smart/generated/index.toml",
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

    let row = worker_row(&ledger);
    assert_eq!(
        row["model"], "base-model",
        "a non-matching repo key must never apply: {row}"
    );
    let evidence = row["evidence"].as_str().unwrap_or_default();
    assert!(
        !evidence.contains("config-qualifier=repo:"),
        "a non-matching repo key must leave evidence unqualified: {evidence}"
    );
}

/// Blocker `repo-qualifier-never-applied`'s `resolve_run_variant` half: a
/// variant declared ONLY under the repo-scoped table
/// (`[repo."<key>".agent.variant.<v>.role.*]`, no base-level
/// `[agent.variant.<v>]` sibling) must still be derivable by longest-suffix
/// match against the trait id, or the `(repo,role,variant)` rung can never
/// be reached for that config shape. The fixture trait id ends in `-smart`.
#[test]
fn repo_scoped_variant_declaration_is_derivable_with_no_base_level_sibling() {
    let scratch = ScratchRoot::new("p451-repo-qualifier-variant");
    let repo = scratch.home().join("repo");
    let home = scratch.home();
    let ledger = repo.join(".ctx/runs/fixture.json");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    let script = home.join("repo-qualifier-worker.sh");
    write_executable(&script, fast_worker_script());
    git_init(&repo);
    let active_key = active_repo_key(&repo, &home);

    let global_config = home.join("ctx").join("traits").join("runtime.toml");
    fs::create_dir_all(global_config.parent().unwrap()).unwrap();
    fs::write(
        &global_config,
        format!(
            "schema-version = \"0.2\"\n\n[repo.\"{active_key}\".agent.variant.smart.role.worker]\nmodel = \"repo-smart-model\"\n"
        ),
    )
    .unwrap();

    init_fixture_repo(&repo, &home, &script);

    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/authored/fixture-p451-repo-smart/generated/index.toml",
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

    let row = worker_row(&ledger);
    assert_eq!(
        row["model"], "repo-smart-model",
        "a variant declared only under the repo-scoped table must still resolve, since the \
         fixture trait id ends in \"-smart\" and no base [agent.variant.smart] table exists: {row}"
    );
    let evidence = row["evidence"].as_str().unwrap_or_default();
    assert!(
        evidence.contains(&format!("config-qualifier=repo:{active_key}+variant:smart")),
        "evidence must name the composed repo+variant qualifier: {evidence}"
    );
}
