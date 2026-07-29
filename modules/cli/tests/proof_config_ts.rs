//! Public-path proofs for P457's optional TypeScript config authoring
//! (`config.ts` -> `config.toml`) and its drift guard.

use std::fs;
use std::path::Path;

use support::{ScratchRoot, controlled_command, git_init, run_ctx, symlink_node_modules, utf8};

/// A minimal PATH with no `node` on it, proving a command genuinely never
/// shells out to node when no `config.ts` exists.
fn scrubbed_path() -> &'static str {
    "/usr/bin:/bin"
}

/// A `config.ts` importing a sibling `pools.ts` local module, so a build
/// captures a transitive (not just top-file) source manifest entry.
const CONFIG_TS: &str = "import { defineConfig } from \"@ctx-traits/config\";\n\
import { maxRetries } from \"./pools.ts\";\n\
\n\
export default defineConfig({\n\
  run: { maxRetries },\n\
});\n";

/// A `config.ts` with no local import at all — just the bare
/// `@ctx-traits/config` import — for the linked-dependency proof, where the
/// point is that nothing but the top file itself should ever be manifested.
const CONFIG_TS_NO_LOCAL_IMPORT: &str = "import { defineConfig } from \"@ctx-traits/config\";\n\
\n\
export default defineConfig({\n\
  run: { maxRetries: 3 },\n\
});\n";

fn pools_ts(max_retries: u32) -> String {
    format!("export const maxRetries = {max_retries};\n")
}

fn scaffold_repo(scratch: &ScratchRoot, label: &str) -> std::path::PathBuf {
    let proj = scratch.home().join(label);
    fs::create_dir_all(proj.join(".ctx")).unwrap();
    git_init(&proj);
    symlink_node_modules(&proj);
    proj
}

fn build_config(proj: &Path, home: &Path) -> std::process::Output {
    run_ctx(&["traits", "config", "build"], proj, home)
}

/// A TOML-only repo (no `config.ts`) runs every command with `node`
/// entirely absent from `PATH` and resolves identically to today — the
/// default, most common case, byte-identical to before this phase existed.
#[test]
fn toml_only_repo_never_shells_out_to_node() {
    let scratch = ScratchRoot::new("p457-toml-only-no-node");
    let proj = scratch.home().join("repo");
    fs::create_dir_all(proj.join(".ctx")).unwrap();
    git_init(&proj);
    fs::write(proj.join(".ctx/config.toml"), "[run]\nmax-in-flight = 2\n").unwrap();

    let mut command = controlled_command(
        &support::ctx_bin(),
        &["traits", "doctor", "--config"],
        &proj,
        &scratch.home(),
    );
    command.env("PATH", scrubbed_path());
    let output = command.output().unwrap();
    let (stdout, stderr) = utf8(&output);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout={stdout} stderr={stderr}"
    );
    assert!(stdout.contains("run.max-in-flight: 2"), "{stdout}");
}

/// `config build` on a `config.ts` importing a local `pools.ts` produces a
/// `config.toml` carrying the `# ctx:generated` marker and a manifest
/// naming both the top file and the imported module.
#[test]
fn build_emits_marker_and_transitive_manifest() {
    let scratch = ScratchRoot::new("p457-build-manifest");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(proj.join(".ctx/config.ts"), CONFIG_TS).unwrap();
    fs::write(proj.join(".ctx/pools.ts"), pools_ts(3)).unwrap();

    let output = build_config(&proj, &scratch.home());
    let (stdout, stderr) = utf8(&output);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout={stdout} stderr={stderr}"
    );

    let generated = fs::read_to_string(proj.join(".ctx/config.toml")).unwrap();
    assert!(generated.starts_with("# ctx:generated"), "{generated}");
    assert!(
        generated.contains("# ctx:source config.ts sha256:"),
        "{generated}"
    );
    assert!(
        generated.contains("# ctx:source pools.ts sha256:"),
        "{generated}"
    );
    assert!(generated.contains("max-retries = 3"), "{generated}");
}

/// Editing only the imported `pools.ts` (not the top `config.ts` file)
/// still trips the drift guard — the crux the transitive manifest exists
/// to catch; a top-file-only manifest would miss this.
#[test]
fn transitive_import_drift_is_caught() {
    let scratch = ScratchRoot::new("p457-transitive-drift");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(proj.join(".ctx/config.ts"), CONFIG_TS).unwrap();
    fs::write(proj.join(".ctx/pools.ts"), pools_ts(3)).unwrap();
    let build = build_config(&proj, &scratch.home());
    assert_eq!(build.status.code(), Some(0));

    fs::write(proj.join(".ctx/pools.ts"), pools_ts(4)).unwrap();

    let output = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    let (_, stderr) = utf8(&output);
    assert_ne!(output.status.code(), Some(0));
    assert!(stderr.contains("pools.ts"), "{stderr}");
    assert!(stderr.contains("ctx traits config build"), "{stderr}");
}

/// Editing the top `config.ts` file itself trips the drift guard.
#[test]
fn top_file_drift_is_caught() {
    let scratch = ScratchRoot::new("p457-top-file-drift");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(proj.join(".ctx/config.ts"), CONFIG_TS).unwrap();
    fs::write(proj.join(".ctx/pools.ts"), pools_ts(3)).unwrap();
    let build = build_config(&proj, &scratch.home());
    assert_eq!(build.status.code(), Some(0));

    fs::write(
        proj.join(".ctx/config.ts"),
        format!("{CONFIG_TS}// touched\n"),
    )
    .unwrap();

    let output = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    let (_, stderr) = utf8(&output);
    assert_ne!(output.status.code(), Some(0));
    assert!(stderr.contains("config.ts"), "{stderr}");
}

/// A `config.toml` beside a `config.ts` with no `# ctx:generated` marker
/// refuses, naming the file and both remedies (rebuild, or delete
/// `config.ts` to go TOML-first).
#[test]
fn hand_edited_generated_file_is_rejected() {
    let scratch = ScratchRoot::new("p457-hand-edited");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(proj.join(".ctx/config.ts"), CONFIG_TS).unwrap();
    fs::write(proj.join(".ctx/config.toml"), "[run]\nmax-retries = 9\n").unwrap();

    let output = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    let (_, stderr) = utf8(&output);
    assert_ne!(output.status.code(), Some(0));
    assert!(stderr.contains("config.toml"), "{stderr}");
    assert!(stderr.contains("ctx traits config build"), "{stderr}");
    assert!(stderr.contains("config.ts"), "{stderr}");
}

/// A `config.ts` present with no `config.toml` at all (never built)
/// refuses.
#[test]
fn never_built_is_rejected() {
    let scratch = ScratchRoot::new("p457-never-built");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(proj.join(".ctx/config.ts"), CONFIG_TS).unwrap();

    let output = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    let (_, stderr) = utf8(&output);
    assert_ne!(output.status.code(), Some(0));
    assert!(stderr.contains("config build"), "{stderr}");
}

/// A marked `config.toml` with no sibling `config.ts` (the seeded-worktree
/// case) loads normally, with no refusal.
#[test]
fn seeded_worktree_without_source_loads_normally() {
    let scratch = ScratchRoot::new("p457-seeded-worktree");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(proj.join(".ctx/config.ts"), CONFIG_TS).unwrap();
    fs::write(proj.join(".ctx/pools.ts"), pools_ts(3)).unwrap();
    let build = build_config(&proj, &scratch.home());
    assert_eq!(build.status.code(), Some(0));

    fs::remove_file(proj.join(".ctx/config.ts")).unwrap();
    fs::remove_file(proj.join(".ctx/pools.ts")).unwrap();

    let output = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    let (stdout, stderr) = utf8(&output);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout={stdout} stderr={stderr}"
    );
    assert!(stdout.contains("run.max-retries: 3"), "{stdout}");
}

/// `config build` twice on unchanged input produces byte-identical output
/// (sorted-key emission is idempotent regardless of JS object insertion
/// order or repeated runs).
#[test]
fn build_is_byte_idempotent() {
    let scratch = ScratchRoot::new("p457-idempotent");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(proj.join(".ctx/config.ts"), CONFIG_TS).unwrap();
    fs::write(proj.join(".ctx/pools.ts"), pools_ts(3)).unwrap();

    let first = build_config(&proj, &scratch.home());
    assert_eq!(first.status.code(), Some(0));
    let first_bytes = fs::read(proj.join(".ctx/config.toml")).unwrap();

    let second = build_config(&proj, &scratch.home());
    assert_eq!(second.status.code(), Some(0));
    let second_bytes = fs::read(proj.join(".ctx/config.toml")).unwrap();

    assert_eq!(first_bytes, second_bytes);
}

/// A `config.ts` emitting an unknown key fails `config build` naming the
/// field, through the same `deny_unknown_fields` decode the TOML loader
/// uses — no second schema.
#[test]
fn invalid_config_is_caught_at_build() {
    let scratch = ScratchRoot::new("p457-invalid-build");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(
        proj.join(".ctx/config.ts"),
        "export default { run: { notARealField: 1 } };\n",
    )
    .unwrap();

    let output = build_config(&proj, &scratch.home());
    let (_, stderr) = utf8(&output);
    assert_ne!(output.status.code(), Some(0));
    assert!(
        stderr.contains("notARealField") || stderr.contains("not-a-real-field"),
        "{stderr}"
    );
}

/// Bootstrap-deadlock guard: `config build` must succeed on its own even
/// when the existing generated `config.toml` has drifted — it never reads
/// through the guarded loader.
#[test]
fn build_recovers_a_drifted_config_without_deadlocking() {
    let scratch = ScratchRoot::new("p457-bootstrap-deadlock");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(proj.join(".ctx/config.ts"), CONFIG_TS).unwrap();
    fs::write(proj.join(".ctx/pools.ts"), pools_ts(3)).unwrap();
    let build = build_config(&proj, &scratch.home());
    assert_eq!(build.status.code(), Some(0));

    fs::write(proj.join(".ctx/pools.ts"), pools_ts(7)).unwrap();
    let drifted = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    assert_ne!(drifted.status.code(), Some(0));

    let rebuild = build_config(&proj, &scratch.home());
    let (stdout, stderr) = utf8(&rebuild);
    assert_eq!(
        rebuild.status.code(),
        Some(0),
        "stdout={stdout} stderr={stderr}"
    );

    let generated = fs::read_to_string(proj.join(".ctx/config.toml")).unwrap();
    assert!(generated.contains("max-retries = 7"), "{generated}");
}

/// The guard fires independently for the global layer
/// (`~/.config/ctx/config.ts`): a drifted global layer refuses even when
/// the repo layer is clean.
#[test]
fn global_layer_drift_is_guarded_independently_of_the_repo_layer() {
    let scratch = ScratchRoot::new("p457-global-layer");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(proj.join(".ctx/config.toml"), "[run]\nmax-in-flight = 1\n").unwrap();

    let global_dir = scratch.home().join("ctx");
    fs::create_dir_all(&global_dir).unwrap();
    symlink_node_modules(&global_dir);
    fs::write(global_dir.join("config.ts"), CONFIG_TS).unwrap();
    fs::write(global_dir.join("pools.ts"), pools_ts(2)).unwrap();

    let build = run_ctx(
        &[
            "traits",
            "config",
            "build",
            global_dir.join("config.ts").to_str().unwrap(),
        ],
        &proj,
        &scratch.home(),
    );
    let (stdout, stderr) = utf8(&build);
    assert_eq!(
        build.status.code(),
        Some(0),
        "stdout={stdout} stderr={stderr}"
    );

    let clean = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    assert_eq!(clean.status.code(), Some(0));

    fs::write(global_dir.join("pools.ts"), pools_ts(5)).unwrap();

    let drifted = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    let (_, stderr) = utf8(&drifted);
    assert_ne!(drifted.status.code(), Some(0));
    assert!(stderr.contains("pools.ts"), "{stderr}");
}

/// A repo-layer `config.ts` importing a local module that lives well
/// outside `.ctx/` (above the repo root a naive "inferred root" filter
/// would have picked) is still manifested and still guarded: the manifest
/// names it, and editing it afterwards trips drift. This is the escaping
/// -import case a prior version silently dropped from the manifest.
#[test]
fn escaping_repo_import_is_manifested_and_guarded() {
    let scratch = ScratchRoot::new("p457-escaping-repo-import");
    let proj = scratch.home().join("repo");
    fs::create_dir_all(proj.join(".ctx")).unwrap();
    git_init(&proj);
    symlink_node_modules(&proj);

    let outside = scratch.home().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("ext.ts"), "export const maxRetries = 3;\n").unwrap();

    fs::write(
        proj.join(".ctx/config.ts"),
        "import { defineConfig } from \"@ctx-traits/config\";\n\
import { maxRetries } from \"../../outside/ext.ts\";\n\
\n\
export default defineConfig({\n\
  run: { maxRetries },\n\
});\n",
    )
    .unwrap();

    let build = build_config(&proj, &scratch.home());
    let (stdout, stderr) = utf8(&build);
    assert_eq!(
        build.status.code(),
        Some(0),
        "stdout={stdout} stderr={stderr}"
    );

    let generated = fs::read_to_string(proj.join(".ctx/config.toml")).unwrap();
    assert!(
        generated.contains("# ctx:source ../../outside/ext.ts sha256:"),
        "{generated}"
    );
    assert!(generated.contains("max-retries = 3"), "{generated}");
    assert_no_dependency_sources(&generated);

    let clean = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    assert_eq!(clean.status.code(), Some(0));

    fs::write(outside.join("ext.ts"), "export const maxRetries = 9;\n").unwrap();

    let drifted = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    let (_, stderr) = utf8(&drifted);
    assert_ne!(drifted.status.code(), Some(0));
    assert!(stderr.contains("ext.ts"), "{stderr}");
    assert!(stderr.contains("ctx traits config build"), "{stderr}");
}

/// The same escaping-import property holds for the global layer: a
/// `~/.config/ctx/config.ts` importing `../pools/p.ts` (above the config
/// -home directory, which has no repository at all) is manifested and
/// guarded exactly like the repo-layer case above.
#[test]
fn escaping_global_import_is_manifested_and_guarded() {
    let scratch = ScratchRoot::new("p457-escaping-global-import");
    let proj = scaffold_repo(&scratch, "repo");
    fs::write(proj.join(".ctx/config.toml"), "[run]\nmax-in-flight = 1\n").unwrap();

    let global_dir = scratch.home().join("ctx");
    fs::create_dir_all(&global_dir).unwrap();
    symlink_node_modules(&global_dir);

    let pools_dir = scratch.home().join("pools");
    fs::create_dir_all(&pools_dir).unwrap();
    fs::write(pools_dir.join("p.ts"), "export const maxRetries = 2;\n").unwrap();

    fs::write(
        global_dir.join("config.ts"),
        "import { defineConfig } from \"@ctx-traits/config\";\n\
import { maxRetries } from \"../pools/p.ts\";\n\
\n\
export default defineConfig({\n\
  run: { maxRetries },\n\
});\n",
    )
    .unwrap();

    let build = run_ctx(
        &[
            "traits",
            "config",
            "build",
            global_dir.join("config.ts").to_str().unwrap(),
        ],
        &proj,
        &scratch.home(),
    );
    let (stdout, stderr) = utf8(&build);
    assert_eq!(
        build.status.code(),
        Some(0),
        "stdout={stdout} stderr={stderr}"
    );

    let generated = fs::read_to_string(global_dir.join("config.toml")).unwrap();
    assert!(
        generated.contains("# ctx:source ../pools/p.ts sha256:"),
        "{generated}"
    );
    assert_no_dependency_sources(&generated);

    let clean = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    assert_eq!(clean.status.code(), Some(0));

    fs::write(pools_dir.join("p.ts"), "export const maxRetries = 8;\n").unwrap();

    let drifted = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    let (_, stderr) = utf8(&drifted);
    assert_ne!(drifted.status.code(), Some(0));
    assert!(stderr.contains("p.ts"), "{stderr}");
}

/// No `# ctx:source` line names a `dist/` build artifact or anything under
/// `@ctx-traits/config`'s own package tree — only author-owned files ever
/// enter the manifest.
fn assert_no_dependency_sources(generated: &str) {
    assert!(!generated.contains("dist/"), "{generated}");
    assert!(!generated.contains("@ctx-traits"), "{generated}");
}

/// `@ctx-traits/config` linked into `node_modules` as a symlink to a
/// package checkout (the pnpm workspace/link shape, i.e. this repo's own
/// dogfood and any monorepo consumer) reports its files by their *resolved*
/// realpath through node's `registerHooks({ load })` — with no
/// `node_modules` path component left to match on. A build must still
/// exclude them (by specifier bareness, not by path shape), and editing the
/// linked package's build output afterwards must never make a
/// config-loading command falsely refuse. Uses a fully isolated fake
/// `@ctx-traits/config` package under the scratch root (never the real
/// checkout's `packages/config/`) so this test cannot race concurrently
/// running tests that also build against the real package.
#[test]
fn linked_dependency_files_never_enter_the_manifest_or_trip_drift() {
    let scratch = ScratchRoot::new("p457-linked-dependency");
    let proj = scratch.home().join("repo");
    fs::create_dir_all(proj.join(".ctx")).unwrap();
    git_init(&proj);

    let fake_package = scratch.home().join("fake-ctx-traits-config");
    fs::create_dir_all(fake_package.join("dist")).unwrap();
    fs::write(
        fake_package.join("package.json"),
        "{\"name\": \"@ctx-traits/config\", \"version\": \"0.0.0\", \"type\": \"module\", \"main\": \"dist/index.js\"}\n",
    )
    .unwrap();
    fs::write(
        fake_package.join("dist/index.js"),
        "export function defineConfig(config) {\n  return config;\n}\n",
    )
    .unwrap();

    fs::create_dir_all(proj.join("node_modules/@ctx-traits")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&fake_package, proj.join("node_modules/@ctx-traits/config"))
        .unwrap_or_else(|error| panic!("cannot symlink fake package: {error}"));
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&fake_package, proj.join("node_modules/@ctx-traits/config"))
        .unwrap_or_else(|error| panic!("cannot symlink fake package: {error}"));

    fs::write(proj.join(".ctx/config.ts"), CONFIG_TS_NO_LOCAL_IMPORT).unwrap();

    let build = build_config(&proj, &scratch.home());
    let (stdout, stderr) = utf8(&build);
    assert_eq!(
        build.status.code(),
        Some(0),
        "stdout={stdout} stderr={stderr}"
    );

    let generated = fs::read_to_string(proj.join(".ctx/config.toml")).unwrap();
    assert_no_dependency_sources(&generated);
    assert!(
        generated.contains("# ctx:source config.ts sha256:"),
        "{generated}"
    );

    let clean = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    assert_eq!(clean.status.code(), Some(0));

    // Editing the linked dependency's own build output must never trip the
    // guard — a routine dependency rebuild must never invalidate every
    // user's built config.
    fs::write(
        fake_package.join("dist/index.js"),
        "export function defineConfig(config) {\n  return config;\n}\n// touched\n",
    )
    .unwrap();

    let after_dependency_edit = run_ctx(&["traits", "doctor", "--config"], &proj, &scratch.home());
    let (stdout, stderr) = utf8(&after_dependency_edit);
    assert_eq!(
        after_dependency_edit.status.code(),
        Some(0),
        "stdout={stdout} stderr={stderr}"
    );
}
