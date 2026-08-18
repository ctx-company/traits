//! 0213: `ctx traits fork <id>` end-to-end proofs.
//!
//! Drives the real `ctx` binary across a producer repository (a real,
//! CDK-buildable authored package created via `ctx traits init` + `ctx
//! traits build`) and a consumer repository that path-installs it, forks
//! it, and asserts the full transaction: the authored fork builds and
//! checks, carries `[forked-from]` provenance, and the vendored dependency
//! (manifest declaration, project lock entry, vendored tree) is gone.

use std::fs;
use std::path::{Path, PathBuf};

use support::{
    ScratchRoot, git_init, repo_root, require_success, run_ctx, symlink_node_modules, utf8,
};

/// A real, CDK-buildable authored package at
/// `<repo>/.ctx/traits/authored/<id>`, via the same `init` + `build` path
/// `build_default_output_matches_the_panel_registry_shape` uses. Returns the
/// authored package root, so the caller can path-install it from elsewhere.
fn build_forkable_producer(repo: &Path, home: &Path, id: &str) -> PathBuf {
    fs::create_dir_all(repo).unwrap();
    git_init(repo);
    symlink_node_modules(repo);
    require_success(
        "`ctx traits init <id>`",
        &["traits", "init", id],
        repo,
        home,
    );
    require_success(
        "initial explicit-path `ctx traits build`",
        &[
            "traits",
            "build",
            &format!(".ctx/traits/authored/{id}/source/index.ts"),
        ],
        repo,
        home,
    );
    repo.join(".ctx/traits/authored").join(id)
}

/// `path:` spec pointing from `<home>/consumer` at
/// `<home>/producer/.ctx/traits/authored/<id>` — both fixtures below always
/// use exactly this layout, so the relative path is known statically rather
/// than computed with an extra dependency.
fn producer_path_spec(id: &str) -> String {
    format!("path:../producer/.ctx/traits/authored/{id}")
}

/// Install `producer_root` into `consumer` by relative path (see
/// [`producer_path_spec`]) and return the alias `dependency add` chose (the
/// path's last component, per P535), optionally overridden by `alias`.
fn install_path_dependency(
    consumer: &Path,
    home: &Path,
    producer_root: &Path,
    alias: Option<&str>,
) -> String {
    let id = producer_root.file_name().unwrap().to_str().unwrap();
    let spec = producer_path_spec(id);
    let mut args = vec!["traits", "dependency", "add", spec.as_str()];
    if let Some(alias) = alias {
        args.push("--alias");
        args.push(alias);
    }
    require_success(
        "`ctx traits dependency add path:<producer>`",
        &args,
        consumer,
        home,
    );
    alias.unwrap_or(id).to_string()
}

/// A private `node_modules` for a fixture project: every entry of the
/// repository's own installed `node_modules` symlinked in individually
/// (never the whole directory — [`support::symlink_node_modules`] does that
/// and shares one real directory across every caller, which would collide
/// with this fixture's own `@fixture/dep` package), so a caller can add its
/// own real package directories alongside them. Mirrors
/// `proof_trait_composition.rs`'s helper of the same shape; kept local
/// since integration-test binaries share no lib.
fn private_node_modules(proj: &Path) -> PathBuf {
    let node_modules = proj.join("node_modules");
    fs::create_dir_all(&node_modules).unwrap();
    for entry in fs::read_dir(repo_root().join("node_modules")).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        #[cfg(unix)]
        std::os::unix::fs::symlink(entry.path(), node_modules.join(&name))
            .unwrap_or_else(|error| panic!("cannot symlink {name:?}: {error}"));
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(entry.path(), node_modules.join(&name))
            .unwrap_or_else(|error| panic!("cannot symlink {name:?}: {error}"));
    }
    node_modules
}

const DEP_PACKAGE_JSON: &str = "{\n  \"name\": \"@fixture/dep\",\n  \"version\": \"1.0.0\"\n}\n";
const DEP_SHARED_MJS: &str =
    "export const summaryText = \"Describe what this trait should accomplish.\";\n";

/// A minimal, independently synthesizable CDK source for `@fixture/dep`
/// itself, built through the ordinary `ctx traits init`/`build` lifecycle
/// so its own `trait.lock` self entry is production-written.
fn dep_trait_source() -> &'static str {
    "import { agent, input, port, procedure, sequence, slot, trait } from \"@ctx-traits/cdk\";\n\
\n\
const summary = slot.text(\"summary\");\n\
const output = port.output.text({ id: \"summary\", value: summary });\n\
const worker = agent(\"worker\", { description: \"Completes the starter task.\" });\n\
\n\
export const draft = trait({\n\
  id: \"fixture-dep\",\n\
  name: \"fixture-dep\",\n\
  description: \"A fixture dependency trait package.\",\n\
  port: output,\n\
  procedure: procedure({\n\
    description: \"A fixture dependency trait package.\",\n\
    sequence: sequence.prompt({\n\
      id: \"run\",\n\
      agent: worker,\n\
      prompt: input.prompt`Describe the task for this trait.`,\n\
      output: summary,\n\
    }),\n\
  }),\n\
});\n"
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let dest_path = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            fs::create_dir_all(&dest_path).unwrap();
            copy_dir_recursive(&entry.path(), &dest_path);
        } else {
            fs::copy(entry.path(), &dest_path).unwrap();
        }
    }
}

/// Builds `@fixture/dep` as its own real trait project in a scratch
/// directory, then copies the resulting package (`trait.toml`,
/// `trait.lock`, `generated/`, `source/`) into `dep_root` alongside a
/// `package.json` and the plain `shared.mjs` helper consumers import —
/// exactly `proof_trait_composition.rs`'s `write_locked_dependency_fixture`.
fn write_locked_dependency_fixture(dep_root: &Path, label: &str) {
    let dep_scratch = ScratchRoot::new(label);
    let dep_home = dep_scratch.home();
    let dep_proj = dep_home.join("dep-repo");
    fs::create_dir_all(&dep_proj).unwrap();
    git_init(&dep_proj);
    private_node_modules(&dep_proj);

    require_success(
        "`ctx traits init fixture-dep` for the dependency fixture",
        &["traits", "init", "fixture-dep"],
        &dep_proj,
        &dep_home,
    );
    let dep_source_path = dep_proj.join(".ctx/traits/authored/fixture-dep/source/index.ts");
    fs::write(&dep_source_path, dep_trait_source()).unwrap();
    require_success(
        "building the dependency fixture's own trait package",
        &[
            "traits",
            "build",
            ".ctx/traits/authored/fixture-dep/source/index.ts",
        ],
        &dep_proj,
        &dep_home,
    );

    let dep_package_root = dep_proj.join(".ctx/traits/authored/fixture-dep");
    fs::create_dir_all(dep_root).unwrap();
    copy_dir_recursive(&dep_package_root, dep_root);
    fs::write(dep_root.join("package.json"), DEP_PACKAGE_JSON).unwrap();
    fs::write(dep_root.join("shared.mjs"), DEP_SHARED_MJS).unwrap();
    assert!(
        dep_root.join("trait.lock").exists(),
        "copying the dependency fixture's build output did not carry a trait.lock"
    );
}

/// Single-trait fixture source that imports the `@fixture/dep` bare
/// specifier, so a build over it stamps a build-derived `[[dependency]]`
/// pin — used by the lock-finalization proof below.
fn derived_dep_fixture_source(id: &str) -> String {
    format!(
        "import {{ agent, input, port, procedure, sequence, slot, trait }} from \"@ctx-traits/cdk\";\n\
import {{ summaryText }} from \"@fixture/dep/shared.mjs\";\n\
\n\
const summary = slot.text(\"summary\");\n\
const output = port.output.text({{ id: \"summary\", value: summary }});\n\
const worker = agent(\"worker\", {{ description: \"Completes the starter task.\" }});\n\
\n\
export const draft = trait({{\n\
  id: \"{id}\",\n\
  name: \"{id}\",\n\
  description: summaryText,\n\
  port: output,\n\
  procedure: procedure({{\n\
    description: summaryText,\n\
    sequence: sequence.prompt({{\n\
      id: \"run\",\n\
      agent: worker,\n\
      prompt: input.prompt`Describe the task for this trait.`,\n\
      output: summary,\n\
    }}),\n\
  }}),\n\
}});\n"
    )
}

#[test]
fn fork_builds_checkable_authored_package_with_provenance_and_detaches_vendor() {
    let scratch = ScratchRoot::new("p0213-fork-happy-path");
    let home = scratch.home();
    let id = "fixture-fork-happy";
    let producer_root = build_forkable_producer(&home.join("producer"), &home, id);

    let consumer = home.join("consumer");
    fs::create_dir_all(&consumer).unwrap();
    git_init(&consumer);
    symlink_node_modules(&consumer);
    let alias = install_path_dependency(&consumer, &home, &producer_root, None);
    assert_eq!(alias, id);

    let fork_stdout = require_success(
        "`ctx traits fork <id> --json`",
        &["traits", "fork", id, "--json"],
        &consumer,
        &home,
    );
    assert!(
        fork_stdout.contains(&format!("\"id\": \"{id}\"")),
        "fork report did not name the forked package: {fork_stdout}"
    );

    let authored_manifest = consumer
        .join(".ctx/traits/authored")
        .join(id)
        .join("trait.toml");
    let manifest_text = fs::read_to_string(&authored_manifest).unwrap();
    assert!(
        manifest_text.contains("[forked-from]"),
        "authored trait.toml is missing forked-from provenance: {manifest_text}"
    );
    assert!(
        manifest_text.contains(&format!("id = \"{id}\"")),
        "forked-from provenance does not record the vendored package id: {manifest_text}"
    );

    assert!(
        !consumer.join(".ctx/traits/vendored").join(id).exists(),
        "vendored tree for {id} was not removed by fork"
    );
    let config_text = fs::read_to_string(consumer.join(".ctx/traits/config.toml")).unwrap();
    assert!(
        !config_text.contains(id),
        "detached alias {id} still appears in the manifest: {config_text}"
    );
    let lock_text = fs::read_to_string(consumer.join(".ctx/traits/config.lock")).unwrap();
    assert!(
        !lock_text.contains(&format!("alias = \"{id}\"")),
        "detached alias {id} still has a project lock entry: {lock_text}"
    );

    let check = run_ctx(&["traits", "check", id, "--json"], &consumer, &home);
    let (check_stdout, check_stderr) = utf8(&check);
    assert!(
        check.status.success(),
        "forked authored package failed to check\nstdout: {check_stdout}\nstderr: {check_stderr}"
    );
}

#[test]
fn fork_of_an_already_forked_id_errors_loudly_without_touching_the_authored_tree() {
    let scratch = ScratchRoot::new("p0213-fork-rerun");
    let home = scratch.home();
    let id = "fixture-fork-rerun";
    let producer_root = build_forkable_producer(&home.join("producer"), &home, id);

    let consumer = home.join("consumer");
    fs::create_dir_all(&consumer).unwrap();
    git_init(&consumer);
    symlink_node_modules(&consumer);
    install_path_dependency(&consumer, &home, &producer_root, None);
    require_success(
        "first `ctx traits fork`",
        &["traits", "fork", id],
        &consumer,
        &home,
    );

    let authored_manifest = consumer
        .join(".ctx/traits/authored")
        .join(id)
        .join("trait.toml");
    let bytes_before = fs::read(&authored_manifest).unwrap();

    // Re-install the same producer under a fresh alias so the second fork
    // attempt has an installed dependency to resolve against, and hits the
    // real refusal this test is about: an authored package already exists
    // at the target id.
    install_path_dependency(
        &consumer,
        &home,
        &producer_root,
        Some("fixture-fork-rerun-again"),
    );

    let second = run_ctx(
        &["traits", "fork", "fixture-fork-rerun-again"],
        &consumer,
        &home,
    );
    let (stdout, stderr) = utf8(&second);
    assert!(
        !second.status.success(),
        "re-forking to an already-authored id must fail loudly\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("already exists") || stdout.contains("already exists"),
        "fork failure did not explain the collision\nstdout: {stdout}\nstderr: {stderr}"
    );

    let bytes_after = fs::read(&authored_manifest).unwrap();
    assert_eq!(
        bytes_before, bytes_after,
        "a failed fork must never touch the existing authored package"
    );
}

#[test]
fn fork_of_an_uninstalled_id_errors_loudly_and_writes_nothing() {
    let scratch = ScratchRoot::new("p0213-fork-not-installed");
    let home = scratch.home();
    let consumer = home.join("consumer");
    fs::create_dir_all(&consumer).unwrap();
    git_init(&consumer);

    let result = run_ctx(&["traits", "fork", "never-installed"], &consumer, &home);
    let (stdout, stderr) = utf8(&result);
    assert!(
        !result.status.success(),
        "forking an uninstalled id must fail loudly\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        !consumer
            .join(".ctx/traits/authored/never-installed")
            .exists(),
        "fork of an uninstalled id must write no authored package"
    );
}

/// The authored `trait.lock` a fork produces must represent the *completed,
/// post-detach* dependency set: no stale row for the alias the fork just
/// detached (project-level `[dependencies]` still declared it when the
/// interim, pre-detach lock sync ran), and the build-derived pin for a real
/// import (`@fixture/dep`) preserved — proving fork routes through the same
/// shared lock finalization (`record_lock_evidence`) `ctx traits build`
/// uses, run again after detach against the reconciled dependency set.
#[test]
fn fork_lock_matches_post_detach_dependencies_and_preserves_derived_pins() {
    let scratch = ScratchRoot::new("p0213-fork-lock-finalization");
    let home = scratch.home();
    let id = "fixture-fork-derived-dep";

    let producer = home.join("producer");
    fs::create_dir_all(&producer).unwrap();
    git_init(&producer);
    let producer_node_modules = private_node_modules(&producer);
    write_locked_dependency_fixture(
        &producer_node_modules.join("@fixture/dep"),
        "p0213-fork-dep-producer",
    );
    require_success(
        "`ctx traits init <id>`",
        &["traits", "init", id],
        &producer,
        &home,
    );
    let producer_source_rel = format!(".ctx/traits/authored/{id}/source/index.ts");
    fs::write(
        producer.join(&producer_source_rel),
        derived_dep_fixture_source(id),
    )
    .unwrap();
    require_success(
        "initial `ctx traits build` with a derived dependency",
        &["traits", "build", &producer_source_rel],
        &producer,
        &home,
    );
    let producer_root = producer.join(".ctx/traits/authored").join(id);

    let consumer = home.join("consumer");
    fs::create_dir_all(&consumer).unwrap();
    git_init(&consumer);
    let consumer_node_modules = private_node_modules(&consumer);
    write_locked_dependency_fixture(
        &consumer_node_modules.join("@fixture/dep"),
        "p0213-fork-dep-consumer",
    );

    let alias = install_path_dependency(&consumer, &home, &producer_root, None);
    assert_eq!(alias, id);

    require_success(
        "`ctx traits fork <id>` over a derived-dependency source",
        &["traits", "fork", id],
        &consumer,
        &home,
    );

    let authored_root = consumer.join(".ctx/traits/authored").join(id);
    let lock_text = fs::read_to_string(authored_root.join("trait.lock")).unwrap();
    assert!(
        lock_text.contains("fixture-dep"),
        "authored trait.lock is missing the build-derived dependency pin for fixture-dep: {lock_text}"
    );
    assert!(
        lock_text.contains("digest"),
        "authored trait.lock's dependency pin is missing a digest: {lock_text}"
    );
    assert!(
        !lock_text.contains(&format!("alias = \"{id}\"")),
        "authored trait.lock still carries a dependency row for the detached alias {id:?} it was \
         forked from — the lock was finalized before detach against a stale project-dependency \
         set: {lock_text}"
    );

    let check = run_ctx(&["traits", "check", id, "--json"], &consumer, &home);
    let (check_stdout, check_stderr) = utf8(&check);
    assert!(
        check.status.success(),
        "forked authored package with a derived dependency failed to check\nstdout: {check_stdout}\nstderr: {check_stderr}"
    );
}

/// A post-detach lock-finalization failure (a conflicting dependency
/// declaration surfaced only after the alias is removed from project
/// config) must not strand the fork half-done: the project manifest,
/// project lock, and vendored tree must all be restored to exactly what
/// they were before `fork` ran, and no authored package must be left
/// behind. The single-trait branch runs no lock sync before the detach, so
/// this is the first point a conflicting `[[dependency]]`/`[dependencies]`
/// pair can surface — regression guard for treating
/// `distribution::remove` as a step in the middle of the transaction
/// (post-detach finalization can still fail) rather than its last one.
#[test]
fn fork_lock_finalization_failure_preserves_installed_dependency() {
    let scratch = ScratchRoot::new("p0213-fork-lock-finalization-failure");
    let home = scratch.home();
    let id = "fixture-fork-lock-conflict";
    let producer_root = build_forkable_producer(&home.join("producer"), &home, id);

    // A package-level `[dependencies]` entry the forked package's own
    // trait.toml carries forward unchanged from the vendored copy — its
    // default `id` (the alias, `conflict-dep`) will not match the
    // project-level declaration below, so `merge_declarations` refuses
    // the pair exactly when the post-detach `record_lock_evidence` call
    // reads the package manifest's dependencies.
    let producer_manifest = producer_root.join("trait.toml");
    let mut producer_manifest_text = fs::read_to_string(&producer_manifest).unwrap();
    producer_manifest_text.push_str(
        "\n[dependencies.conflict-dep]\nversion = \"1.0.0\"\nnpm = \"conflict-dep-pkg\"\n",
    );
    fs::write(&producer_manifest, &producer_manifest_text).unwrap();

    let consumer = home.join("consumer");
    fs::create_dir_all(&consumer).unwrap();
    git_init(&consumer);
    symlink_node_modules(&consumer);
    let alias = install_path_dependency(&consumer, &home, &producer_root, None);
    assert_eq!(alias, id);

    // A project-level `[[dependency]]` entry for the same alias with a
    // different `id`, unrelated to the alias being forked — hand-appended
    // since no CLI surface writes this legacy array table.
    let manifest_path = consumer.join(".ctx/traits/config.toml");
    let mut config_text = fs::read_to_string(&manifest_path).unwrap();
    config_text.push_str(
        "\n[[vendor.dependency]]\nalias = \"conflict-dep\"\nid = \"conflict-dep-other-id\"\nversion = \"2.0.0\"\n",
    );
    fs::write(&manifest_path, &config_text).unwrap();

    let manifest_before = fs::read(&manifest_path).unwrap();
    let lock_path = consumer.join(".ctx/traits/config.lock");
    let lock_before = fs::read(&lock_path).unwrap();
    let vendored_path = consumer.join(".ctx/traits/vendored").join(id);
    assert!(
        vendored_path.is_dir(),
        "precondition: {id} must be vendored before forking it"
    );

    let result = run_ctx(&["traits", "fork", id], &consumer, &home);
    let (stdout, stderr) = utf8(&result);
    assert!(
        !result.status.success(),
        "fork must fail loudly on a post-detach dependency conflict\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("conflicting") || stdout.contains("conflicting"),
        "fork failure did not name the dependency conflict\nstdout: {stdout}\nstderr: {stderr}"
    );

    assert!(
        !consumer.join(".ctx/traits/authored").join(id).exists(),
        "a fork that fails on lock finalization must leave no authored package"
    );
    assert_eq!(
        manifest_before,
        fs::read(&manifest_path).unwrap(),
        "a fork that fails on lock finalization must restore the project manifest"
    );
    assert_eq!(
        lock_before,
        fs::read(&lock_path).unwrap(),
        "a fork that fails on lock finalization must restore the project lock"
    );
    assert!(
        vendored_path.is_dir(),
        "a fork that fails on lock finalization must restore the vendored dependency"
    );
}

/// A trust-store read failure (malformed `trust.toml`) must abort `fork`
/// entirely before the irreversible detach: the project manifest, project
/// lock, and vendored tree must stay byte-identical, and no authored
/// package must be left behind. Regression guard for ordering the trust
/// verdict resolution — a fallible read of external, user-editable state —
/// strictly before `distribution::remove`.
#[test]
fn fork_trust_read_failure_preserves_installed_dependency() {
    let scratch = ScratchRoot::new("p0213-fork-trust-read-failure");
    let home = scratch.home();
    let id = "fixture-fork-trust-failure";
    let producer_root = build_forkable_producer(&home.join("producer"), &home, id);

    let consumer = home.join("consumer");
    fs::create_dir_all(&consumer).unwrap();
    git_init(&consumer);
    symlink_node_modules(&consumer);
    install_path_dependency(&consumer, &home, &producer_root, None);

    let manifest_path = consumer.join(".ctx/traits/config.toml");
    let lock_path = consumer.join(".ctx/traits/config.lock");
    let vendored_path = consumer.join(".ctx/traits/vendored").join(id);
    let manifest_before = fs::read(&manifest_path).unwrap();
    let lock_before = fs::read(&lock_path).unwrap();
    assert!(
        vendored_path.is_dir(),
        "precondition: {id} must be vendored before forking it"
    );

    let trust_store_path = home.join("ctx/trust.toml");
    fs::create_dir_all(trust_store_path.parent().unwrap()).unwrap();
    fs::write(&trust_store_path, "this is not valid toml [[[\n").unwrap();

    let result = run_ctx(&["traits", "fork", id], &consumer, &home);
    let (stdout, stderr) = utf8(&result);
    assert!(
        !result.status.success(),
        "fork must fail loudly when the trust store cannot be decoded\nstdout: {stdout}\nstderr: {stderr}"
    );

    assert!(
        !consumer.join(".ctx/traits/authored").join(id).exists(),
        "a fork that fails on trust resolution must leave no authored package"
    );
    assert_eq!(
        manifest_before,
        fs::read(&manifest_path).unwrap(),
        "a fork that fails on trust resolution must leave the project manifest untouched"
    );
    assert_eq!(
        lock_before,
        fs::read(&lock_path).unwrap(),
        "a fork that fails on trust resolution must leave the project lock untouched"
    );
    assert!(
        vendored_path.is_dir(),
        "a fork that fails on trust resolution must leave the vendored dependency in place"
    );
}
