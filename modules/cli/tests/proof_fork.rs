//! 0213: `ctx traits fork <id>` end-to-end proofs.
//!
//! Drives the real `ctx` binary across a producer repository (a real,
//! CDK-buildable authored package created via `ctx traits init` + `ctx
//! traits build`) and a consumer repository that path-installs it, forks
//! it, and asserts the full transaction: the authored fork builds and
//! checks, carries `[forked-from]` provenance, and the vendored dependency
//! (manifest declaration, project lock entry, vendored tree) is gone.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use support::{
    ScratchRoot, git_init, private_node_modules, require_success, run_ctx, symlink_node_modules,
    utf8, write_locked_dependency_fixture,
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

/// A lock-finalization failure (a conflicting dependency declaration
/// surfaced against the projected post-detach dependency set) must not
/// strand the fork half-done: since finalization now runs against a scratch
/// manifest projection strictly *before* `distribution::remove`, the
/// project manifest, project lock, and vendored tree must never be touched
/// at all (nothing to restore), and no authored package must be left
/// behind. Regression guard for the transaction ordering: finalization is
/// the fallible step and it must resolve before `distribution::remove`
/// becomes reachable, never after.
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
    let vendored_tree_before = collect_tree(&vendored_path);
    // `install_path_dependency` above already appended its own `Install`
    // audit record, so the journal is non-empty by this point — snapshot
    // its full contents (not mere existence) and assert byte-identical
    // after the forced failure, proving no `Remove` line was appended.
    let audit_root = home.join("ctx/audit");
    let audit_before = collect_tree(&audit_root);

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
    assert_eq!(
        vendored_tree_before,
        collect_tree(&vendored_path),
        "a fork that fails on lock finalization must leave the vendored tree \
         full-tree-byte-identical, not merely present"
    );
    assert_eq!(
        audit_before,
        collect_tree(&audit_root),
        "a fork that fails on lock finalization must append no Remove audit record"
    );
    let leftover_scratch_manifests: Vec<_> = fs::read_dir(manifest_path.parent().unwrap())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains(".fork-projection-")
        })
        .collect();
    assert!(
        leftover_scratch_manifests.is_empty(),
        "a fork that fails on lock finalization must leave no scratch manifest projection \
         sibling: {leftover_scratch_manifests:?}"
    );
    let leftover_scratch_vendor_roots: Vec<_> = fs::read_dir(manifest_path.parent().unwrap())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains(".fork-vendor-scratch-")
        })
        .collect();
    assert!(
        leftover_scratch_vendor_roots.is_empty(),
        "a fork that fails on lock finalization must leave no scratch vendor-root sibling: \
         {leftover_scratch_vendor_roots:?}"
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

/// Lock finalization (both the single-trait and family branches) sync-writes
/// the *full* merged project dependency set, not just the package being
/// forked — so a second, unrelated project dependency is present in that
/// set. If its live source changes after install but before a fork that
/// later fails (here, the same malformed-trust-store failure as
/// `fork_trust_read_failure_preserves_installed_dependency`), the unrelated
/// dependency's installed vendored tree must stay exactly as it was: lock
/// finalization vendors into a scratch directory
/// (`fork_vendor_scratch_root`), never the live `.ctx/traits/vendored` tree,
/// so there is no window in which an unrelated dependency could be
/// refreshed from its live source mid-fork and left that way by a later
/// failure.
#[test]
fn fork_lock_finalization_leaves_other_vendored_dependencies_untouched_even_when_their_live_source_changes()
 {
    let scratch = ScratchRoot::new("p0213-fork-unrelated-dep-untouched");
    let home = scratch.home();
    let id = "fixture-fork-unrelated-dep-target";
    let other_id = "fixture-fork-unrelated-dep-sibling";
    let producer_root = build_forkable_producer(&home.join("producer"), &home, id);
    let other_producer_root =
        build_forkable_producer(&home.join("other-producer"), &home, other_id);

    let consumer = home.join("consumer");
    fs::create_dir_all(&consumer).unwrap();
    git_init(&consumer);
    symlink_node_modules(&consumer);
    let alias = install_path_dependency(&consumer, &home, &producer_root, None);
    assert_eq!(alias, id);
    let other_spec = format!("path:../other-producer/.ctx/traits/authored/{other_id}");
    require_success(
        "`ctx traits dependency add path:<other-producer>`",
        &["traits", "dependency", "add", other_spec.as_str()],
        &consumer,
        &home,
    );

    let other_vendored_path = consumer.join(".ctx/traits/vendored").join(other_id);
    assert!(
        other_vendored_path.is_dir(),
        "precondition: {other_id} must be vendored before forking {id}"
    );
    let other_vendored_before = collect_tree(&other_vendored_path);

    // Change the unrelated dependency's *live* source after install: absent
    // scratch redirection, fork's lock finalization would detect this via
    // `package_subset_matches` and re-copy it straight into the live
    // vendored tree while finalizing the forked package's own lock.
    let other_manifest = other_producer_root.join("trait.toml");
    let mut other_manifest_text = fs::read_to_string(&other_manifest).unwrap();
    other_manifest_text.push_str("\n# live source changed after install, before fork\n");
    fs::write(&other_manifest, &other_manifest_text).unwrap();

    let trust_store_path = home.join("ctx/trust.toml");
    fs::create_dir_all(trust_store_path.parent().unwrap()).unwrap();
    fs::write(&trust_store_path, "this is not valid toml [[[\n").unwrap();

    let result = run_ctx(&["traits", "fork", id], &consumer, &home);
    let (stdout, stderr) = utf8(&result);
    assert!(
        !result.status.success(),
        "fork must fail loudly when the trust store cannot be decoded\nstdout: {stdout}\nstderr: {stderr}"
    );

    assert_eq!(
        other_vendored_before,
        collect_tree(&other_vendored_path),
        "a fork that fails after lock finalization must leave every other project dependency's \
         installed vendored tree full-tree-byte-identical, even when that dependency's live \
         source changed before the fork ran"
    );
}

/// Lock finalization's `dependency::sync` write pass unconditionally
/// establishes the nested-ignore invariant (`ensure_nested_gitignore`) on
/// its target root whenever the merged `dependencies` set is non-empty.
/// That set is *hand-authored* project/package declarations only —
/// build-derived (digest-carrying) rows like `@fixture/dep` are filtered
/// out of it and handled separately via `stamped_dependencies`, and the
/// forked alias itself is removed from the projected manifest before this
/// pass runs — so an unrelated, ordinary path dependency that survives the
/// fork is installed here specifically to keep the post-detach
/// `dependencies` set non-empty and actually exercise the guarded branch.
/// Absent `vendor_root_override` redirection of that branch, the real
/// consumer's `.ctx/.gitignore` would be created or appended to mid-fork,
/// with nothing to undo it when a later step aborts the fork.
///
/// The failure is forced the same way as
/// `fork_remove_failure_leaves_no_audit_or_scratch_residue` — denying write
/// on the vendored root's parent so `distribution::remove`'s vendor-backup
/// rename fails — rather than a malformed trust store: a malformed trust
/// store fails inside `dependency::sync` itself (every write pass resolves
/// the target trait's own trust verdict via `resolve_named` before this
/// guarded line is ever reached), so it can never actually exercise lock
/// finalization succeeding and a *later* step failing. The vendor-root
/// permission denial only ever blocks `distribution::remove`, which runs
/// strictly after lock finalization has already completed.
///
/// Deletes the file outright before forking (the strongest form of
/// "canonical entries absent") and asserts it is still absent afterward —
/// not merely unchanged content, but unchanged existence.
#[test]
fn fork_lock_finalization_failure_leaves_ctx_gitignore_untouched() {
    let scratch = ScratchRoot::new("p0213-fork-gitignore-untouched");
    let home = scratch.home();
    let id = "fixture-fork-gitignore-untouched";
    let other_id = "fixture-fork-gitignore-untouched-sibling";

    let producer = home.join("producer");
    fs::create_dir_all(&producer).unwrap();
    git_init(&producer);
    let producer_node_modules = private_node_modules(&producer);
    write_locked_dependency_fixture(
        &producer_node_modules.join("@fixture/dep"),
        "p0213-fork-gitignore-dep-producer",
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
    build_forkable_producer(&home.join("other-producer"), &home, other_id);

    let consumer = home.join("consumer");
    fs::create_dir_all(&consumer).unwrap();
    git_init(&consumer);
    let consumer_node_modules = private_node_modules(&consumer);
    write_locked_dependency_fixture(
        &consumer_node_modules.join("@fixture/dep"),
        "p0213-fork-gitignore-dep-consumer",
    );

    let alias = install_path_dependency(&consumer, &home, &producer_root, None);
    assert_eq!(alias, id);

    // A project-level `[[dependency]]` entry (the legacy hand-authored
    // table `dependency::sync`'s merged `dependencies` set is built from —
    // distinct from the `[dependencies]` npm/path-install table `dependency
    // add` writes, which this guarded branch never sees) pointing at the
    // unrelated `other-producer` package, unconnected to the fork target,
    // so `dependencies` remains genuinely non-empty after `id` is removed
    // from the projected post-detach manifest. Hand-appended since no CLI
    // surface writes this legacy array table.
    let manifest_path = consumer.join(".ctx/traits/config.toml");
    let mut config_text = fs::read_to_string(&manifest_path).unwrap();
    config_text.push_str(&format!(
        "\n[[vendor.dependency]]\nalias = \"other\"\nid = \"{other_id}\"\nversion = \"0.1.0\"\n\n[vendor.dependency.source]\npath = \"../other-producer/.ctx/traits/authored/{other_id}\"\n"
    ));
    fs::write(&manifest_path, &config_text).unwrap();

    // Force the strongest precondition: no `.ctx/.gitignore` at all, not
    // merely one missing a canonical entry.
    let gitignore_path = consumer.join(".ctx/.gitignore");
    let _ = fs::remove_file(&gitignore_path);
    assert!(
        !gitignore_path.exists(),
        "precondition: .ctx/.gitignore must be absent before forking"
    );

    // Deny write on the vendored root's parent directory: `distribution::
    // remove`'s vendor-backup rename needs to add/remove entries there, so
    // this fails that rename specifically, after fork's own copy/build/
    // lock-finalization steps have already succeeded.
    let vendor_root = consumer.join(".ctx/traits/vendored");
    let original_mode = fs::metadata(&vendor_root).unwrap().permissions().mode();
    fs::set_permissions(&vendor_root, fs::Permissions::from_mode(0o555)).unwrap();

    let result = run_ctx(&["traits", "fork", id], &consumer, &home);

    fs::set_permissions(&vendor_root, fs::Permissions::from_mode(original_mode)).unwrap();

    let (stdout, stderr) = utf8(&result);
    assert!(
        !result.status.success(),
        "fork must fail loudly when distribution::remove itself cannot detach\nstdout: {stdout}\nstderr: {stderr}"
    );

    assert!(
        !gitignore_path.exists(),
        "a fork that fails after lock finalization must not have created .ctx/.gitignore \
         from an owned scratch-directory sync pass"
    );
}

/// A native `trait(id, { variants })` family fixture source with two
/// variants, `id` parameterized so each test gets a distinct alias/package
/// id — used by the family fork-conflict regression below.
fn family_fixture_source(id: &str) -> String {
    format!(
        "import {{ agent, input, port, procedure, sequence, slot, trait, variant }} from \"@ctx-traits/cdk\";\n\
\n\
const summary = slot.text(\"summary\");\n\
const output = port.output.text({{ id: \"summary\", value: summary }});\n\
const worker = agent(\"worker\", {{ description: \"Completes the starter task.\" }});\n\
\n\
const variantFixture = (name) => variant({{\n\
  name,\n\
  summary: `The ${{name}} variant.`,\n\
  procedure: procedure({{\n\
    description: \"Describe what this trait should accomplish.\",\n\
    output,\n\
    sequence: sequence.prompt({{\n\
      id: \"run\",\n\
      agent: worker,\n\
      prompt: input.prompt`Describe the task for this trait.`,\n\
      output: summary,\n\
    }}),\n\
  }}),\n\
}});\n\
\n\
export const draft = trait(\"{id}\", {{\n\
  variants: {{\n\
    default: variantFixture(\"default\").default(),\n\
    quick: variantFixture(\"quick\"),\n\
  }},\n\
}});\n"
    )
}

/// A real, CDK-buildable native-family authored package at
/// `<repo>/.ctx/traits/authored/<id>`, analogous to
/// [`build_forkable_producer`] but publishing two variants under
/// `generated/<name>/` instead of a single `generated/index.toml`.
fn build_forkable_family_producer(repo: &Path, home: &Path, id: &str) -> PathBuf {
    fs::create_dir_all(repo).unwrap();
    git_init(repo);
    symlink_node_modules(repo);
    require_success(
        "`ctx traits init <id>`",
        &["traits", "init", id],
        repo,
        home,
    );
    let source_rel = format!(".ctx/traits/authored/{id}/source/index.ts");
    fs::write(repo.join(&source_rel), family_fixture_source(id)).unwrap();
    require_success(
        "initial family `ctx traits build`",
        &["traits", "build", &source_rel],
        repo,
        home,
    );
    repo.join(".ctx/traits/authored").join(id)
}

/// A family fork whose post-detach lock finalization fails on a conflicting
/// dependency declaration (mirroring
/// `fork_lock_finalization_failure_preserves_installed_dependency`, but
/// forcing the failure through the *family* branch's per-variant lock sync)
/// must not have refreshed the forked alias's own installed vendored tree
/// from its live source first: with lock finalization for every variant
/// routed through the single projected-manifest sync pass (never an interim
/// pass against the live project manifest), there is no window in which a
/// later failure could have already re-vendored `id`'s installed copy.
/// Regression guard distinguishing this from the single-trait case: a
/// family's canonical digest used to be discovered by a *separate*,
/// pre-projection sync pass per variant before this fix.
#[test]
fn fork_family_lock_finalization_failure_leaves_vendored_tree_untouched() {
    let scratch = ScratchRoot::new("p0213-fork-family-lock-conflict");
    let home = scratch.home();
    let id = "fixture-fork-family-conflict";
    let producer_root = build_forkable_family_producer(&home.join("producer"), &home, id);

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
    let vendored_tree_before = collect_tree(&vendored_path);

    let result = run_ctx(&["traits", "fork", id], &consumer, &home);
    let (stdout, stderr) = utf8(&result);
    assert!(
        !result.status.success(),
        "family fork must fail loudly on a post-detach dependency conflict\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("conflicting") || stdout.contains("conflicting"),
        "family fork failure did not name the dependency conflict\nstdout: {stdout}\nstderr: {stderr}"
    );

    assert!(
        !consumer.join(".ctx/traits/authored").join(id).exists(),
        "a family fork that fails on lock finalization must leave no authored package"
    );
    assert_eq!(
        manifest_before,
        fs::read(&manifest_path).unwrap(),
        "a family fork that fails on lock finalization must leave the project manifest untouched"
    );
    assert_eq!(
        lock_before,
        fs::read(&lock_path).unwrap(),
        "a family fork that fails on lock finalization must leave the project lock untouched"
    );
    assert_eq!(
        vendored_tree_before,
        collect_tree(&vendored_path),
        "a family fork that fails on lock finalization must leave the installed vendored tree \
         full-tree-byte-identical — it must never have been refreshed from its live source by an \
         interim, pre-projection lock sync"
    );
}

/// Every regular file under `dir`, keyed by its path relative to `dir` and
/// sorted, so two trees compare byte-for-byte regardless of directory-entry
/// iteration order.
fn collect_tree(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_path_buf();
                out.push((rel, fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// `distribution::remove` failing on its own (a permission-denied rename,
/// unrelated to fork's own lock-finalization projection, which by this
/// point has already succeeded) must leave no residue: the project
/// manifest, project lock, and vendored tree stay full-tree-byte-identical,
/// no authored package survives, no `Remove` audit record is appended (the
/// audit journal directory never even gets created), and no
/// `*.fork-projection-*` scratch sibling is left next to the project
/// manifest. Regression guard for treating `distribution::remove` as
/// fork's one true, last, self-rolling-back mutation: this proof forces
/// *that* step itself to fail, distinct from
/// `fork_lock_finalization_failure_preserves_installed_dependency` (which
/// forces the projected-manifest finalization step, strictly before
/// `remove` is ever reached, to fail instead).
#[test]
fn fork_remove_failure_leaves_no_audit_or_scratch_residue() {
    let scratch = ScratchRoot::new("p0213-fork-remove-failure");
    let home = scratch.home();
    let id = "fixture-fork-remove-failure";
    let producer_root = build_forkable_producer(&home.join("producer"), &home, id);

    let consumer = home.join("consumer");
    fs::create_dir_all(&consumer).unwrap();
    git_init(&consumer);
    symlink_node_modules(&consumer);
    install_path_dependency(&consumer, &home, &producer_root, None);

    let manifest_path = consumer.join(".ctx/traits/config.toml");
    let lock_path = consumer.join(".ctx/traits/config.lock");
    let vendor_root = consumer.join(".ctx/traits/vendored");
    let vendored_path = vendor_root.join(id);
    let manifest_before = fs::read(&manifest_path).unwrap();
    let lock_before = fs::read(&lock_path).unwrap();
    let vendored_tree_before = collect_tree(&vendored_path);
    // `install_path_dependency` above already appended its own `Install`
    // audit record, so the journal is non-empty by this point — snapshot
    // its full contents (not mere existence) and assert byte-identical
    // after the forced failure, proving no `Remove` line was appended.
    let audit_root = home.join("ctx/audit");
    let audit_before = collect_tree(&audit_root);

    // Deny write on the vendored root's parent directory: `distribution::
    // remove`'s vendor-backup rename needs to add/remove entries there, so
    // this fails that rename specifically, after fork's own copy/build/
    // lock-finalization steps (which only ever read `vendored_path`) have
    // already succeeded.
    let original_mode = fs::metadata(&vendor_root).unwrap().permissions().mode();
    fs::set_permissions(&vendor_root, fs::Permissions::from_mode(0o555)).unwrap();

    let result = run_ctx(&["traits", "fork", id], &consumer, &home);

    fs::set_permissions(&vendor_root, fs::Permissions::from_mode(original_mode)).unwrap();

    let (stdout, stderr) = utf8(&result);
    assert!(
        !result.status.success(),
        "fork must fail loudly when distribution::remove itself cannot detach\nstdout: {stdout}\nstderr: {stderr}"
    );

    assert!(
        !consumer.join(".ctx/traits/authored").join(id).exists(),
        "a fork that fails inside distribution::remove must leave no authored package"
    );
    assert_eq!(
        manifest_before,
        fs::read(&manifest_path).unwrap(),
        "a fork that fails inside distribution::remove must leave the project manifest untouched"
    );
    assert_eq!(
        lock_before,
        fs::read(&lock_path).unwrap(),
        "a fork that fails inside distribution::remove must leave the project lock untouched"
    );
    assert!(
        vendored_path.is_dir(),
        "a fork that fails inside distribution::remove must leave the vendored dependency in place"
    );
    assert_eq!(
        vendored_tree_before,
        collect_tree(&vendored_path),
        "a fork that fails inside distribution::remove must leave the vendored tree \
         full-tree-byte-identical, not merely present"
    );
    assert_eq!(
        audit_before,
        collect_tree(&audit_root),
        "a fork that fails inside distribution::remove must append no Remove audit record"
    );
    let leftover_scratch_manifests: Vec<_> = fs::read_dir(manifest_path.parent().unwrap())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains(".fork-projection-")
        })
        .collect();
    assert!(
        leftover_scratch_manifests.is_empty(),
        "a fork that fails inside distribution::remove must leave no scratch manifest \
         projection sibling: {leftover_scratch_manifests:?}"
    );
}
