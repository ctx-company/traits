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

use support::{ScratchRoot, git_init, require_success, run_ctx, symlink_node_modules, utf8};

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
