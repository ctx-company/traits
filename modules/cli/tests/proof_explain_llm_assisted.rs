//! Task 0124: `ctx traits explain --scaffold --llm-assisted` (no `--candidate`)
//! now drives the `explain-trait` runner through the harness — proving the
//! Done-when end to end against a real (fixture) narrator: the deterministic
//! evidence reaches the runner, the runner's echoed scaffold survives the
//! grounding gate unaltered, and the printed candidate carries a non-empty
//! advisory explanation. Deterministic `explain --scaffold` (no
//! `--llm-assisted`) is exercised too, proving it is unaffected by the
//! contract retarget.

use std::fs;
use std::path::Path;

use support::{ScratchRoot, fixture_agent_bin, git_init, run_ctx, run_ctx_with_env, utf8};

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!("cannot create directory {}: {error}", parent.display())
        });
    }
    fs::write(path, contents)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
}

/// The `generator` role is bound to the fixture agent's `--role explainer`
/// stub, which echoes the deterministic scaffold embedded in the compiled
/// prompt verbatim, adding a fixed non-empty `explanation` — proving the
/// grounding gate accepts an unaltered echo rather than exercising a live
/// provider.
fn write_fixture_ctx_toml(repo: &Path) {
    let agent = fixture_agent_bin()
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let ctx_toml = format!(
        r#"schema-version = "0.2"

[harness.stub-explainer]
kind = "custom"
bin = "{agent}"
transports = ["cli"]
version-probe = ["--probe"]

[harness.stub-explainer.cli]
argv = ["--role", "explainer"]
output = "raw-json"

[agent.role.generator]
harness = "stub-explainer"
transport = "cli"
session-mode = "per-frame"
"#
    );
    write_file(&repo.join("ctx.toml"), &ctx_toml);
}

const SOURCE_TRAIT: &str = r#"id = "explain-llm-assisted-fixture"
schema-version = "0.2"
version = "0.1.0"
name = "Explain LLM Assisted Fixture"
summary = "0124 proof fixture: explain --llm-assisted target."
"#;

fn write_source_map(repo: &Path, source_path: &Path) -> std::path::PathBuf {
    let map_path = repo.join("explain-fixture.map.json");
    let source_path_str = source_path.to_str().expect("utf8 source path");
    let map = format!(
        r#"{{"trait:explain-llm-assisted-fixture": {{"file": {source_path_str:?}, "start": 1, "end": 1}}}}"#
    );
    write_file(&map_path, &map);
    map_path
}

#[test]
fn explain_llm_assisted_narrates_the_deterministic_scaffold() {
    let scratch = ScratchRoot::new("explain-llm-assisted");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    write_fixture_ctx_toml(&proj);

    let source_path = proj.join("explain-llm-assisted-fixture.toml");
    write_file(&source_path, SOURCE_TRAIT);
    let map_path = write_source_map(&proj, &source_path);

    for dependency in ["explain-trait", "trait-spec"] {
        let approve = run_ctx(&["traits", "trust", "approve", dependency], &proj, &home);
        assert!(
            approve.status.success(),
            "trust approve {dependency} failed: {}",
            utf8(&approve).1
        );
    }

    let output = run_ctx_with_env(
        &[
            "traits",
            "explain",
            "--scaffold",
            "--file",
            source_path.to_str().expect("utf8 source path"),
            "--source-map",
            map_path.to_str().expect("utf8 map path"),
            "--llm-assisted",
            "--json",
        ],
        &proj,
        &home,
        &[],
    );
    let (stdout, stderr) = utf8(&output);
    assert!(
        output.status.success(),
        "explain --scaffold --llm-assisted must succeed once the narrator echoes the scaffold unaltered\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let candidate: serde_json::Value =
        serde_json::from_str(&stdout).expect("--json output must be one candidate document");
    assert_eq!(candidate["status"], serde_json::json!("validated"));
    assert_eq!(
        candidate["gate-summary"]["parse"]["ok"],
        serde_json::json!(true)
    );
    assert_eq!(
        candidate["gate-summary"]["normalize"]["ok"],
        serde_json::json!(true)
    );
    assert_eq!(
        candidate["gate-summary"]["audit"]["ok"],
        serde_json::json!(true)
    );
    assert_eq!(
        candidate["gate-summary"]["check"]["ok"],
        serde_json::json!(true)
    );

    let scaffold = &candidate["context-evidence"]["explain-scaffold"];
    assert_eq!(
        scaffold["trait-id"],
        serde_json::json!("explain-llm-assisted-fixture")
    );
    let explanation = scaffold["explanation"]
        .as_str()
        .expect("narrated scaffold must carry a non-empty explanation string");
    assert!(
        !explanation.trim().is_empty(),
        "narrated explanation must not be empty"
    );
}

/// Deterministic `explain --scaffold` (no `--llm-assisted`) must remain
/// unaffected by the contract retarget: no provider dispatch, no
/// `explanation` field in the printed scaffold.
#[test]
fn explain_scaffold_without_llm_assisted_stays_deterministic() {
    let scratch = ScratchRoot::new("explain-deterministic");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    write_fixture_ctx_toml(&proj);

    let source_path = proj.join("explain-llm-assisted-fixture.toml");
    write_file(&source_path, SOURCE_TRAIT);
    let map_path = write_source_map(&proj, &source_path);

    let output = run_ctx_with_env(
        &[
            "traits",
            "explain",
            "--scaffold",
            "--file",
            source_path.to_str().expect("utf8 source path"),
            "--source-map",
            map_path.to_str().expect("utf8 map path"),
            "--json",
        ],
        &proj,
        &home,
        &[],
    );
    let (stdout, stderr) = utf8(&output);
    assert!(
        output.status.success(),
        "deterministic explain --scaffold must still pass its own gates\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let candidate: serde_json::Value =
        serde_json::from_str(&stdout).expect("--json output must be one candidate document");
    assert_eq!(
        candidate["provider"]["provider-available"],
        serde_json::json!(false)
    );
    let scaffold = &candidate["context-evidence"]["explain-scaffold"];
    assert!(
        scaffold.get("explanation").is_none(),
        "deterministic scaffold must carry no explanation field\nstdout:\n{stdout}"
    );
}
