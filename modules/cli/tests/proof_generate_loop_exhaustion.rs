//! Task 0066.1: `ctx traits generate`'s default path now drives the guarded
//! produce/evaluate loop declared in `generate-trait` instead of the old
//! one-shot canonical pipeline. This proves the non-convergence contract end
//! to end against a real (never-converging) generator: the declared
//! iteration bound genuinely exhausts, no target package is written, the
//! global trust store and this repo's `trait.lock` are never touched, the
//! scratch candidate is preserved at its reported path, and the process
//! exits non-zero naming the failing rung.

use std::fs;
use std::path::Path;

use support::{
    ScratchRoot, ctx_bin, fixture_agent_bin, git_init, run_ctx, run_ctx_with_env,
    symlink_node_modules, utf8,
};

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!("cannot create directory {}: {error}", parent.display())
        });
    }
    fs::write(path, contents)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
}

/// Every role `generate-trait` drives (`generator`) bound to the fixture
/// agent's `--role generator` stub, which always returns the same
/// deliberately invalid TypeScript candidate — so every round fails at the
/// ladder's `build` rung and the loop's declared bound (3 rounds,
/// `on-exhausted: "continue"`) genuinely exhausts.
fn write_fixture_ctx_toml(repo: &Path) {
    let agent = fixture_agent_bin()
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let ctx_toml = format!(
        r#"schema-version = "0.2"

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

/// Recursively finds a file named `trust.toml` anywhere under `root`, the
/// global machine trust store's file name (`ctx_traits_io::trust::trust_store_path`)
/// — proving it untouched means proving it was never even created inside
/// this test's isolated `HOME`.
fn find_trust_store(root: &Path) -> Option<std::path::PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().and_then(|name| name.to_str()) == Some("trust.toml") {
                return Some(path);
            }
        }
    }
    None
}

#[test]
fn generate_loop_exhaustion_writes_no_package_and_preserves_scratch() {
    let scratch = ScratchRoot::new("generate-loop-exhaustion");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    symlink_node_modules(&proj);
    write_fixture_ctx_toml(&proj);

    // The loop's in-round `evaluate-round` command step recursively invokes
    // `ctx` by bare name (P0066.1 §5's recursive-invocation risk): resolve it
    // by prepending the binary's own directory to `PATH` for this run only.
    let ctx_dir = ctx_bin()
        .parent()
        .expect("ctx binary path has a parent directory")
        .to_path_buf();
    let existing_path = std::env::var("PATH").unwrap_or_default();
    let path_with_ctx = format!("{}:{existing_path}", ctx_dir.display());

    // `generate-trait` itself must be locally trusted before it can be
    // driven at all — a deterministic, offline, local-only operation (no
    // provider call), distinct from the assertion below that a
    // non-converging run never adds a trust record for the CANDIDATE trait.
    for dependency in ["generate-trait", "trait-spec"] {
        let approve = run_ctx(&["traits", "trust", "approve", dependency], &proj, &home);
        assert!(
            approve.status.success(),
            "trust approve {dependency} failed: {}",
            utf8(&approve).1
        );
    }
    let trust_store_before =
        find_trust_store(&home).map(|path| fs::read(&path).expect("read trust store"));

    let trait_id = "generate-loop-exhaustion-fixture";
    let output = run_ctx_with_env(
        &[
            "traits",
            "generate",
            "Generate Loop Exhaustion Fixture",
            "Never converges — proves the non-convergence path.",
            "--json",
        ],
        &proj,
        &home,
        &[("PATH", path_with_ctx.as_str())],
    );
    let (stdout, stderr) = utf8(&output);

    assert!(
        !output.status.success(),
        "generate must fail once the loop's bound exhausts without converging\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("build") || stderr.contains("build"),
        "the report must name the failing rung (build)\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let target_package = proj.join(format!(".ctx/traits/packages/{trait_id}"));
    assert!(
        !target_package.exists(),
        "a non-converging run must never write the target package at {}",
        target_package.display()
    );
    assert!(
        !proj
            .join(format!(".ctx/traits/packages/{trait_id}/trait.lock"))
            .exists(),
        "a non-converging run must never write trait.lock"
    );

    let scratch_package = proj.join(format!(".ctx/traits/packages/{trait_id}-candidate"));
    assert!(
        scratch_package.join("source/index.ts").is_file(),
        "the scratch candidate must be preserved at {} for a non-converging run",
        scratch_package.display()
    );

    let trust_store_after =
        find_trust_store(&home).map(|path| fs::read(&path).expect("read trust store"));
    assert_eq!(
        trust_store_before, trust_store_after,
        "a non-converging run must never touch the global machine trust store"
    );
}
