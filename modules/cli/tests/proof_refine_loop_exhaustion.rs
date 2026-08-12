//! Task 0066.3: `ctx traits refine --apply` now drives the guarded
//! produce/evaluate loop declared in `refine-trait` instead of a single
//! unguarded provider call. This proves the non-convergence contract end to
//! end against a real (never-converging) refiner: the declared iteration
//! bound genuinely exhausts, the source file is never rewritten, and the
//! process exits non-zero naming the failing rung and rounds spent.

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

/// The `refiner` role is bound to the fixture agent's `--role refiner` stub,
/// which always returns deliberately invalid candidate text — so every round
/// fails at the ladder's `synth-normalize` rung and the loop's declared
/// bound (3 rounds, `on-exhausted: "continue"`) genuinely exhausts.
fn write_fixture_ctx_toml(repo: &Path) {
    let agent = fixture_agent_bin()
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let ctx_toml = format!(
        r#"schema-version = "0.4"

[harness.stub-refiner]
kind = "custom"
bin = "{agent}"
transports = ["cli"]
version-probe = ["--probe"]

[harness.stub-refiner.cli]
argv = ["--role", "refiner"]
output = "raw-json"

[agent.role.refiner]
harness = "stub-refiner"
transport = "cli"
session-mode = "per-frame"
"#
    );
    write_file(&repo.join("ctx.toml"), &ctx_toml);
}

const SOURCE_TRAIT: &str = r#"id = "refine-loop-exhaustion-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Refine Loop Exhaustion Fixture"
description = "0066.3 proof fixture: refine loop exhaustion target."
"#;

#[test]
fn refine_apply_loop_exhaustion_writes_nothing_and_names_the_failing_rung() {
    let scratch = ScratchRoot::new("refine-loop-exhaustion");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    write_fixture_ctx_toml(&proj);

    let source_path = proj.join("refine-loop-exhaustion-fixture.toml");
    write_file(&source_path, SOURCE_TRAIT);
    let source_before = fs::read(&source_path).expect("read source before refine");

    // The loop's in-round `evaluate-round` command step recursively invokes
    // `ctx` by bare name: resolve it by prepending the binary's own
    // directory to `PATH` for this run only.
    let ctx_dir = ctx_bin()
        .parent()
        .expect("ctx binary path has a parent directory")
        .to_path_buf();
    let existing_path = std::env::var("PATH").unwrap_or_default();
    let path_with_ctx = format!("{}:{existing_path}", ctx_dir.display());

    for dependency in ["refine-trait", "trait-spec"] {
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
            "refine",
            source_path.to_str().expect("utf8 source path"),
            "Never converges — proves the non-convergence path.",
            "--apply",
            "--json",
        ],
        &proj,
        &home,
        &[("PATH", path_with_ctx.as_str())],
    );
    let (stdout, stderr) = utf8(&output);

    assert!(
        !output.status.success(),
        "refine --apply must fail once the loop's bound exhausts without converging\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("synth-normalize") || stderr.contains("synth-normalize"),
        "the report must name the failing rung (synth-normalize)\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let evidence: serde_json::Value =
        serde_json::from_str(&stdout).expect("--json output must be one RoundEvidence document");
    assert_eq!(evidence["converged"], serde_json::json!(false));
    assert_eq!(evidence["rounds-spent"], serde_json::json!(3));
    assert_eq!(evidence["rounds-bound"], serde_json::json!(3));
    assert_eq!(
        evidence["failing-rung"],
        serde_json::json!("synth-normalize")
    );

    let source_after = fs::read(&source_path).expect("read source after refine");
    assert_eq!(
        source_before, source_after,
        "a non-converging refine --apply run must never rewrite the source file"
    );

    // The exit error names a preserved-candidate path; that path must
    // genuinely hold the last round's candidate text, not just be a stated
    // fact in the error message.
    let candidate_path = proj
        .join(".ctx/traits/authored")
        .join("refine-loop-exhaustion-fixture-candidate")
        .join("candidate.json");
    let candidate_contents = fs::read_to_string(&candidate_path).unwrap_or_else(|error| {
        panic!(
            "preserved-candidate file must exist at {}: {error}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            candidate_path.display()
        )
    });
    assert!(
        !candidate_contents.trim().is_empty(),
        "preserved-candidate file at {} must hold the last round's candidate text, not be empty",
        candidate_path.display()
    );
}

/// `refine` without `--apply` still runs `refine-trait` for its one produce
/// call (task 0066.3 §4's `single-shot` guard), but must never loop — one
/// round spent regardless of convergence, proving the non-`--apply` form's
/// one-provider-call contract survives the meta-trait now always declaring
/// a loop.
#[test]
fn refine_preview_without_apply_spends_exactly_one_round() {
    let scratch = ScratchRoot::new("refine-single-shot");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    write_fixture_ctx_toml(&proj);

    let source_path = proj.join("refine-loop-exhaustion-fixture.toml");
    write_file(&source_path, SOURCE_TRAIT);

    let ctx_dir = ctx_bin()
        .parent()
        .expect("ctx binary path has a parent directory")
        .to_path_buf();
    let existing_path = std::env::var("PATH").unwrap_or_default();
    let path_with_ctx = format!("{}:{existing_path}", ctx_dir.display());

    for dependency in ["refine-trait", "trait-spec"] {
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
            "refine",
            source_path.to_str().expect("utf8 source path"),
            "Preview only — proves the single-shot path.",
            "--json",
        ],
        &proj,
        &home,
        &[("PATH", path_with_ctx.as_str())],
    );
    let (stdout, stderr) = utf8(&output);

    // Round evidence is an `--apply`-only output surface (task 0066.3): a
    // non-`--apply` run's `--json` stdout must stay byte-compatible with
    // before the loop existed — exactly one top-level JSON document, the
    // assist-candidate one, with no round-evidence envelope concatenated
    // ahead of it.
    let mut documents =
        serde_json::Deserializer::from_str(&stdout).into_iter::<serde_json::Value>();
    let first = documents
        .next()
        .unwrap_or_else(|| {
            panic!("expected one JSON document on stdout\nstdout:\n{stdout}\nstderr:\n{stderr}")
        })
        .unwrap_or_else(|error| {
            panic!("stdout must parse as JSON: {error}\nstdout:\n{stdout}\nstderr:\n{stderr}")
        });
    assert!(
        documents.next().is_none(),
        "non-`--apply` refine must print exactly one JSON document, not a round-evidence envelope followed by the candidate\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        first.get("rounds").is_none() && first.get("rounds-spent").is_none(),
        "stdout's one document must be the assist-candidate document, not round evidence\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    // The one-provider-call contract is proven by stderr's progress signal
    // instead: exactly one drafting line and never a revising line.
    let drafting_lines = stderr
        .lines()
        .filter(|line| line.contains("drafting candidate"))
        .count();
    let revising_lines = stderr
        .lines()
        .filter(|line| line.contains("revising candidate"))
        .count();
    assert_eq!(
        drafting_lines, 1,
        "single-shot must spend exactly one produce round\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        revising_lines, 0,
        "single-shot must never loop into a revise round regardless of convergence\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}
