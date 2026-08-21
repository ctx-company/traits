//! Public-path proofs for 0178: `runtime.ts` -> `generated/runtime.toml`
//! at both tiers, `runtime.example.ts`/`.toml` acceptance, `[preferences]`
//! global-only enforcement, and `config init --global` scaffolding.

use std::fs;
use std::path::Path;

use support::{
    ScratchRoot, git_init, ready_hook_fixture_trait, run_ctx, symlink_node_modules, utf8,
    write_hook_fixture_trait,
};

fn scaffold_repo(scratch: &ScratchRoot, label: &str) -> std::path::PathBuf {
    let proj = scratch.home().join(label);
    fs::create_dir_all(proj.join(".ctx/traits")).unwrap();
    git_init(&proj);
    symlink_node_modules(&proj);
    proj
}

fn build_runtime(proj: &Path, home: &Path) -> std::process::Output {
    run_ctx(
        &[
            "traits",
            "internal",
            "config",
            "build",
            ".ctx/traits/runtime.ts",
        ],
        proj,
        home,
    )
}

/// A minimal, legal `runtime.ts`: an empty `RuntimeConfig` via a single
/// registrar call (every `RuntimeConfig` field is optional), wrapped in the
/// required default-exported build function.
const RUNTIME_TS: &str = "import { defineDrive } from \"@ctx-traits/config\";\n\
\n\
export default function () {\n\
  defineDrive({ wait: true });\n\
}\n";

const RUNTIME_TS_V2: &str = "import { defineDrive } from \"@ctx-traits/config\";\n\
\n\
export default function () {\n\
  defineDrive({ wait: false });\n\
}\n";

/// 1. `runtime.ts` compiles to `generated/runtime.toml`, and a resolution
///    command reads it with no `[vendor]`/`ConfigDocument` involvement.
#[test]
fn runtime_ts_builds_generated_document() {
    let scratch = ScratchRoot::new("p0178-build");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(proj.join(".ctx/traits/runtime.ts"), RUNTIME_TS).unwrap();

    let build = build_runtime(&proj, &scratch.home());
    let (stdout, stderr) = utf8(&build);
    assert_eq!(
        build.status.code(),
        Some(0),
        "runtime build failed: stdout={stdout} stderr={stderr}"
    );

    let generated = fs::read_to_string(proj.join(".ctx/traits/generated/runtime.toml")).unwrap();
    assert!(
        generated.contains("[drive]"),
        "generated document missing [drive]: {generated}"
    );

    let doctor = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    let (stdout, stderr) = utf8(&doctor);
    assert_eq!(
        doctor.status.code(),
        Some(0),
        "resolution over the generated runtime document failed: stdout={stdout} stderr={stderr}"
    );
}

/// 2. Editing `runtime.ts` without rebuilding refuses, naming the source.
#[test]
fn stale_generated_runtime_refuses_naming_source() {
    let scratch = ScratchRoot::new("p0178-stale");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(proj.join(".ctx/traits/runtime.ts"), RUNTIME_TS).unwrap();
    let build = build_runtime(&proj, &scratch.home());
    assert_eq!(build.status.code(), Some(0));

    fs::write(proj.join(".ctx/traits/runtime.ts"), RUNTIME_TS_V2).unwrap();

    let doctor = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    let (stdout, stderr) = utf8(&doctor);
    assert_ne!(
        doctor.status.code(),
        Some(0),
        "stale generated runtime document must refuse: stdout={stdout} stderr={stderr}"
    );
    assert!(stderr.contains("runtime.ts"), "{stderr}");
    assert!(
        stderr.contains("ctx traits internal config build"),
        "{stderr}"
    );
}

/// 3. `runtime.ts` and a hand-authored `runtime.toml` both present refuses,
///    naming both paths.
#[test]
fn source_and_hand_toml_both_present_refuses() {
    let scratch = ScratchRoot::new("p0178-both-present");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(proj.join(".ctx/traits/runtime.ts"), RUNTIME_TS).unwrap();
    fs::write(
        proj.join(".ctx/traits/runtime.toml"),
        "schema-version = \"0.1\"\n",
    )
    .unwrap();

    let doctor = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    let (stdout, stderr) = utf8(&doctor);
    assert_ne!(
        doctor.status.code(),
        Some(0),
        "runtime.ts + hand runtime.toml must refuse: stdout={stdout} stderr={stderr}"
    );
    assert!(stderr.contains("runtime.ts"), "{stderr}");
    assert!(stderr.contains("runtime.toml"), "{stderr}");
}

/// 4. A TOML-only fixture (no `runtime.ts`) resolves with no Node
///    invocation — the zero-Node path stays intact.
#[test]
fn toml_only_resolves_without_node() {
    let scratch = ScratchRoot::new("p0178-toml-only");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(
        proj.join(".ctx/traits/runtime.toml"),
        "schema-version = \"0.1\"\n",
    )
    .unwrap();
    // No node_modules symlinked — a Node shell-out would fail hard, not just
    // silently no-op, catching a regression that always tries to build.
    fs::remove_file(proj.join("node_modules")).ok();
    fs::remove_dir_all(proj.join("node_modules")).ok();

    let doctor = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    let (stdout, stderr) = utf8(&doctor);
    assert_eq!(
        doctor.status.code(),
        Some(0),
        "TOML-only runtime config must resolve: stdout={stdout} stderr={stderr}"
    );
}

/// 5. `[preferences]` in a repo-scope runtime source refuses at build time.
#[test]
fn preferences_in_repo_scope_ts_refuses_at_build() {
    let scratch = ScratchRoot::new("p0178-preferences-ts-refuses");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(
        proj.join(".ctx/traits/runtime.ts"),
        "import { definePreferences } from \"@ctx-traits/config\";\n\
\n\
export default function () {\n\
  definePreferences({ configFormat: \"toml\" });\n\
}\n",
    )
    .unwrap();

    let build = build_runtime(&proj, &scratch.home());
    let (stdout, stderr) = utf8(&build);
    assert_ne!(
        build.status.code(),
        Some(0),
        "[preferences] in a repo-scope runtime.ts must refuse: stdout={stdout} stderr={stderr}"
    );
    assert!(stderr.contains("preferences"), "{stderr}");
}

/// 6. `[preferences]` in a repo-scope hand-authored `runtime.toml` refuses
///    at resolve time (the hand-TOML path, not just the TS build path).
#[test]
fn preferences_in_repo_scope_hand_toml_refuses_at_resolve() {
    let scratch = ScratchRoot::new("p0178-preferences-toml-refuses");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(
        proj.join(".ctx/traits/runtime.toml"),
        "[preferences]\nconfig-format = \"toml\"\n",
    )
    .unwrap();

    let doctor = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    let (stdout, stderr) = utf8(&doctor);
    assert_ne!(
        doctor.status.code(),
        Some(0),
        "[preferences] in a repo-scope hand runtime.toml must refuse: stdout={stdout} stderr={stderr}"
    );
    assert!(stderr.contains("preferences"), "{stderr}");
}

/// 7. `ctx traits internal config init --global` scaffolds a config-home
///    `traits/package.json`, and a global `runtime.ts` placed there
///    compiles.
#[test]
fn global_scaffold_and_runtime_ts_compile() {
    let scratch = ScratchRoot::new("p0178-global-scaffold");
    let home = scratch.home();
    fs::create_dir_all(&home).unwrap();

    let init = run_ctx(
        &["traits", "internal", "config", "init", "--global"],
        &home,
        &home,
    );
    let (stdout, stderr) = utf8(&init);
    assert_eq!(
        init.status.code(),
        Some(0),
        "config init --global failed: stdout={stdout} stderr={stderr}"
    );

    let package_json = home.join("ctx/traits/package.json");
    assert!(
        package_json.exists(),
        "config init --global must scaffold traits/package.json"
    );
    let contents = fs::read_to_string(&package_json).unwrap();
    assert!(
        contents.contains("@ctx-traits/config"),
        "package.json must pin @ctx-traits/config: {contents}"
    );

    // config init without --global refuses (no repo-tier scaffold, 0179 owns
    // init/layout rework).
    let repo = scaffold_repo(&scratch, "repo-no-global");
    let init_local = run_ctx(&["traits", "internal", "config", "init"], &repo, &home);
    assert_ne!(
        init_local.status.code(),
        Some(0),
        "config init without --global must refuse"
    );
}

/// 8. `ctx traits internal config accept --yes` materializes the machine copy and
///    stamps the example's digest; a run refuses before acceptance and
///    proceeds after.
#[test]
fn acceptance_gates_run_and_materializes_machine_copy() {
    let scratch = ScratchRoot::new("p0178-accept-gate");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(proj.join(".ctx/traits/runtime.example.ts"), RUNTIME_TS).unwrap();

    let doctor_before = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    let (_stdout, stderr) = utf8(&doctor_before);
    // doctor is read-only and must not be gated.
    assert_eq!(
        doctor_before.status.code(),
        Some(0),
        "doctor must never be gated by unaccepted example: {stderr}"
    );

    let accept = run_ctx(
        &["traits", "internal", "config", "accept", "--yes"],
        &proj,
        &scratch.home(),
    );
    let (stdout, stderr) = utf8(&accept);
    assert_eq!(
        accept.status.code(),
        Some(0),
        "config accept --yes failed: stdout={stdout} stderr={stderr}"
    );

    let machine_copy = proj.join(".ctx/traits/runtime.ts");
    assert!(
        machine_copy.exists(),
        "accept must materialize the machine-local runtime.ts copy"
    );
    assert_eq!(
        fs::read_to_string(&machine_copy).unwrap(),
        RUNTIME_TS,
        "materialized copy must match the accepted example byte-for-byte"
    );
    let generated = proj.join(".ctx/traits/generated/runtime.toml");
    assert!(
        generated.exists(),
        "accept must build the accepted TS machine copy"
    );

    // Accepting again with nothing changed refuses ("nothing to do").
    let reaccept = run_ctx(
        &["traits", "internal", "config", "accept", "--yes"],
        &proj,
        &scratch.home(),
    );
    assert_ne!(
        reaccept.status.code(),
        Some(0),
        "accepting an already-accepted example must refuse"
    );

    // Editing the machine copy (not the example) never re-triggers a
    // refusal — the stamp covers the example, not the copy.
    fs::write(&machine_copy, RUNTIME_TS_V2).unwrap();
    let build_after_edit = build_runtime(&proj, &scratch.home());
    let (stdout, stderr) = utf8(&build_after_edit);
    assert_eq!(
        build_after_edit.status.code(),
        Some(0),
        "editing the machine copy must never re-trigger acceptance: stdout={stdout} stderr={stderr}"
    );

    // Mutating the example refuses again until re-accepted.
    fs::write(proj.join(".ctx/traits/runtime.example.ts"), RUNTIME_TS_V2).unwrap();
    let reaccept_after_mutation = run_ctx(
        &["traits", "internal", "config", "accept", "--yes"],
        &proj,
        &scratch.home(),
    );
    let (stdout, stderr) = utf8(&reaccept_after_mutation);
    assert_eq!(
        reaccept_after_mutation.status.code(),
        Some(0),
        "re-accepting a mutated example must succeed: stdout={stdout} stderr={stderr}"
    );
}

/// 9. `ctx traits run <trait> --no-drive` refuses before dispatch when a
///    committed example has not been accepted (non-TTY, naming `ctx traits
///    config accept`), and proceeds once accepted — read-only `doctor`
///    stays ungated throughout.
#[test]
fn run_dispatch_gated_by_acceptance() {
    let scratch = ScratchRoot::new("p0178-run-gate");
    let proj = scaffold_repo(&scratch, "repo");
    write_hook_fixture_trait(&proj, "demo", "Demo", "demo", "demo trait");
    ready_hook_fixture_trait(&proj, &scratch.home(), "demo");
    fs::write(proj.join(".ctx/traits/runtime.example.ts"), RUNTIME_TS).unwrap();

    let doctor = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    assert_eq!(
        doctor.status.code(),
        Some(0),
        "doctor must never be gated by an unaccepted example"
    );

    let run_before = run_ctx(
        &["traits", "run", "demo", "--no-drive"],
        &proj,
        &scratch.home(),
    );
    let (stdout, stderr) = utf8(&run_before);
    assert_ne!(
        run_before.status.code(),
        Some(0),
        "run must refuse before acceptance: stdout={stdout} stderr={stderr}"
    );
    assert!(
        stderr.contains("ctx traits internal config accept"),
        "refusal must name the accept command: {stderr}"
    );

    let accept = run_ctx(
        &["traits", "internal", "config", "accept", "--yes"],
        &proj,
        &scratch.home(),
    );
    assert_eq!(accept.status.code(), Some(0));

    let run_after = run_ctx(
        &["traits", "run", "demo", "--no-drive"],
        &proj,
        &scratch.home(),
    );
    let (stdout, stderr) = utf8(&run_after);
    assert!(
        !stderr.contains("has not been accepted"),
        "run must proceed past the acceptance gate once accepted: stdout={stdout} stderr={stderr}"
    );
}
