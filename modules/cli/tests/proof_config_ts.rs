//! Public-path proofs for 0177's `config.ts` -> `ConfigDocument` authoring
//! pathway (`.ctx/traits/config.ts` -> `ctx traits config build` ->
//! `.ctx/traits/generated/config.toml`) and its drift guard, plus the
//! retirement proofs for the P457 `.ctx/config.ts` -> `RuntimeConfig`
//! pathway and the standalone `vendor.toml` manifest it replaces.

use std::fs;
use std::path::Path;

use support::{ScratchRoot, controlled_command, git_init, run_ctx, symlink_node_modules, utf8};

/// A minimal PATH with no `node` on it, proving a command genuinely never
/// shells out to node when no `config.ts` exists.
fn scrubbed_path() -> &'static str {
    "/usr/bin:/bin"
}

fn scaffold_repo(scratch: &ScratchRoot, label: &str) -> std::path::PathBuf {
    let proj = scratch.home().join(label);
    fs::create_dir_all(proj.join(".ctx/traits")).unwrap();
    git_init(&proj);
    symlink_node_modules(&proj);
    proj
}

fn build_config(proj: &Path, home: &Path) -> std::process::Output {
    run_ctx(&["traits", "config", "build"], proj, home)
}

/// The `config.ts` this suite uses: a bare `[vendor]` table (`schema-version
/// = "0.1"`), the smallest legal `ConfigDocument`.
const CONFIG_TS: &str = "import { defineConfig } from \"@ctx-traits/config\";\n\
\n\
export default defineConfig({\n\
  vendor: { schemaVersion: \"0.1\" },\n\
} as never);\n";

/// 1. A `config.ts` project builds `generated/config.toml`, and a
///    resolution command (dependency listing) reads its `[vendor]` table.
#[test]
fn config_ts_builds_and_resolution_reads_generated_document() {
    let scratch = ScratchRoot::new("p0177-build-and-read");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(proj.join(".ctx/traits/config.ts"), CONFIG_TS).unwrap();

    let build = build_config(&proj, &scratch.home());
    let (stdout, stderr) = utf8(&build);
    assert_eq!(
        build.status.code(),
        Some(0),
        "config build failed: stdout={stdout} stderr={stderr}"
    );

    let generated_path = proj.join(".ctx/traits/generated/config.toml");
    let generated = fs::read_to_string(&generated_path).unwrap();
    assert!(
        generated.contains("[vendor]"),
        "generated document missing [vendor]: {generated}"
    );
    assert!(
        generated.contains("schema-version = \"0.1\""),
        "generated document missing schema-version: {generated}"
    );

    let outdated = run_ctx(
        &["traits", "dependency", "outdated", "--json"],
        &proj,
        &scratch.home(),
    );
    let (out_stdout, out_stderr) = utf8(&outdated);
    assert_eq!(
        outdated.status.code(),
        Some(0),
        "resolution over the generated document failed: stdout={out_stdout} stderr={out_stderr}"
    );
}

/// 2. Editing `config.ts` without rebuilding refuses, naming the source.
#[test]
fn stale_generated_document_refuses_naming_source() {
    let scratch = ScratchRoot::new("p0177-stale");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(proj.join(".ctx/traits/config.ts"), CONFIG_TS).unwrap();

    let build = build_config(&proj, &scratch.home());
    assert_eq!(build.status.code(), Some(0));

    fs::write(
        proj.join(".ctx/traits/config.ts"),
        "import { defineConfig } from \"@ctx-traits/config\";\n\
\n\
export default defineConfig({\n\
  vendor: { schemaVersion: \"0.2\" },\n\
} as never);\n",
    )
    .unwrap();

    let outdated = run_ctx(
        &["traits", "dependency", "outdated", "--json"],
        &proj,
        &scratch.home(),
    );
    let (stdout, stderr) = utf8(&outdated);
    assert_ne!(
        outdated.status.code(),
        Some(0),
        "stale generated document must refuse: stdout={stdout} stderr={stderr}"
    );
    assert!(
        stderr.contains("config.ts"),
        "refusal must name the source: {stderr}"
    );
    assert!(
        stderr.contains("ctx traits config build"),
        "refusal must state the rebuild command: {stderr}"
    );
}

/// 3. `config.ts` and a hand-authored `config.toml` both present refuses,
///    naming both paths.
#[test]
fn source_and_hand_authored_document_both_present_refuses() {
    let scratch = ScratchRoot::new("p0177-both-present");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(proj.join(".ctx/traits/config.ts"), CONFIG_TS).unwrap();
    fs::write(
        proj.join(".ctx/traits/config.toml"),
        "[vendor]\nschema-version = \"0.1\"\n",
    )
    .unwrap();

    let outdated = run_ctx(
        &["traits", "dependency", "outdated", "--json"],
        &proj,
        &scratch.home(),
    );
    let (stdout, stderr) = utf8(&outdated);
    assert_ne!(
        outdated.status.code(),
        Some(0),
        "both source and hand document present must refuse: stdout={stdout} stderr={stderr}"
    );
    assert!(
        stderr.contains(".ctx/traits/config.ts") && stderr.contains(".ctx/traits/config.toml"),
        "refusal must name both paths: {stderr}"
    );
}

/// 4. A TOML-only repo (no `config.ts`) resolves with `node` entirely
///    absent from `PATH`.
#[test]
fn toml_only_repo_never_shells_out_to_node() {
    let scratch = ScratchRoot::new("p0177-toml-only-no-node");
    let proj = scratch.home().join("repo");
    fs::create_dir_all(proj.join(".ctx/traits")).unwrap();
    git_init(&proj);
    fs::write(
        proj.join(".ctx/traits/config.toml"),
        "[vendor]\nschema-version = \"0.1\"\n",
    )
    .unwrap();

    let mut command = controlled_command(
        &support::ctx_bin(),
        &["traits", "dependency", "outdated", "--json"],
        &proj,
        &scratch.home(),
    );
    command.env("PATH", scrubbed_path());
    let output = command.output().unwrap();
    let (stdout, stderr) = utf8(&output);
    assert_eq!(
        output.status.code(),
        Some(0),
        "TOML-only resolution must never shell out to node: stdout={stdout} stderr={stderr}"
    );
}

/// 5. A fixture carrying only a legacy `.ctx/traits/vendor.toml` gets no
///    successful manifest read — proven behaviorally, not by grep.
#[test]
fn legacy_vendor_toml_has_no_read_path() {
    let scratch = ScratchRoot::new("p0177-legacy-vendor-toml");
    let proj = scratch.home().join("repo");
    fs::create_dir_all(proj.join(".ctx/traits")).unwrap();
    git_init(&proj);
    fs::write(
        proj.join(".ctx/traits/vendor.toml"),
        "schema-version = \"0.1\"\n",
    )
    .unwrap();

    let outdated = run_ctx(
        &["traits", "dependency", "outdated", "--json"],
        &proj,
        &scratch.home(),
    );
    let (stdout, _stderr) = utf8(&outdated);
    assert_eq!(
        outdated.status.code(),
        Some(0),
        "no manifest present must resolve as empty, not error: {stdout}"
    );
    assert!(
        stdout.trim() == "[]" || !stdout.contains("schema-version"),
        "a bare vendor.toml must never be read as the project manifest: {stdout}"
    );
}

/// 6. A repo still carrying the retired P457 `.ctx/config.ts` refuses
///    naming 0178's `runtime.ts`, and does NOT block the new `.ctx/traits/
///    config.ts` pathway (regression proof for the wrong-constant bug this
///    round fixed).
#[test]
fn retired_dot_ctx_config_ts_refuses_naming_0178() {
    let scratch = ScratchRoot::new("p0177-retired-source-refuses");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(proj.join(".ctx/config.ts"), "export default {};\n").unwrap();

    let doctor = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    let (stdout, stderr) = utf8(&doctor);
    assert_ne!(
        doctor.status.code(),
        Some(0),
        "a repo carrying the retired .ctx/config.ts must refuse: stdout={stdout} stderr={stderr}"
    );
    assert!(stderr.contains(".ctx/config.ts"), "{stderr}");
    assert!(stderr.contains("runtime.ts"), "{stderr}");
}

/// The new declarative `.ctx/traits/config.ts` pathway must never trip the
/// retired-source refusal meant for `.ctx/config.ts`.
#[test]
fn new_config_source_does_not_trip_retired_refusal() {
    let scratch = ScratchRoot::new("p0177-new-source-no-refusal");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(proj.join(".ctx/traits/config.ts"), CONFIG_TS).unwrap();
    let build = build_config(&proj, &scratch.home());
    assert_eq!(build.status.code(), Some(0));

    let doctor = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    let (stdout, stderr) = utf8(&doctor);
    assert_eq!(
        doctor.status.code(),
        Some(0),
        "the new config.ts source must never trip the retired-P457 refusal: stdout={stdout} stderr={stderr}"
    );
}

/// A global-scope config document declaring `[vendor]` refuses at build
/// time (deps are project identity, never a machine's).
#[test]
fn global_scope_vendor_declaration_refuses() {
    let scratch = ScratchRoot::new("p0177-global-vendor-refuses");
    let home = scratch.home();
    let global_dir = home.join("ctx");
    fs::create_dir_all(&global_dir).unwrap();
    git_init(&global_dir);
    symlink_node_modules(&global_dir);
    fs::write(global_dir.join("config.ts"), CONFIG_TS).unwrap();

    let source_path = global_dir.join("config.ts");
    let build = run_ctx(
        &["traits", "config", "build", source_path.to_str().unwrap()],
        &global_dir,
        &home,
    );
    let (stdout, stderr) = utf8(&build);
    assert_ne!(
        build.status.code(),
        Some(0),
        "a global-scope [vendor] declaration must refuse: stdout={stdout} stderr={stderr}"
    );
    assert!(stderr.contains("[vendor]"), "{stderr}");
}
