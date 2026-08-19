//! P535: project-scoped local `path:` package installation end-to-end proof.
//!
//! Drives the real `ctx` binary against two separate scratch repositories —
//! a producer (the trait package source) and a consumer (the project that
//! `dependency add`s it by relative path) — exactly the shape the owner-run
//! ctx-gate/ctx-trait cross-repository handoff documented in the phase
//! contract uses, minus the second real repository.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use support::{ScratchRoot, git_init, require_success, run_ctx, utf8};

fn trait_doc(id: &str, summary: &str) -> String {
    format!(
        "id = \"{id}\"\n\
         schema-version = \"0.2\"\n\
         version = \"0.1.0\"\n\
         name = \"Demo\"\n\
         summary = \"{summary}\"\n"
    )
}

/// A native family variant's canonical document (P535 fix): unlike
/// `trait_doc`, every variant of a real folded family package (e.g.
/// `.ctx/traits/authored/implement/`) shares the SAME `id` and is told
/// apart only by `variant` — never by encoding the variant name into the id
/// itself.
fn family_variant_trait_doc(id: &str, variant: &str, summary: &str) -> String {
    format!(
        "id = \"{id}\"\n\
         schema-version = \"0.3\"\n\
         version = \"0.1.0\"\n\
         name = \"Demo\"\n\
         summary = \"{summary}\"\n\
         variant = \"{variant}\"\n"
    )
}

/// Extract the `canonical-digest=<value>` token `ctx traits check --json`
/// prints in its `machine-trust` section summary, so a test can assert two
/// resolutions landed on the exact same (or a different) vendored variant
/// without depending on any other section's wording.
fn canonical_digest_from_check(stdout: &str) -> &str {
    let start = stdout
        .find("canonical-digest=")
        .expect("check output must report canonical-digest")
        + "canonical-digest=".len();
    let rest = &stdout[start..];
    let end = rest
        .find(|c: char| c == ')' || c == '"' || c.is_whitespace())
        .unwrap_or(rest.len());
    &rest[..end]
}

fn write_producer_package(root: &Path, id: &str, summary: &str) {
    fs::create_dir_all(root.join("generated")).unwrap();
    fs::write(
        root.join("trait.toml"),
        format!(
            "[package]\nid = \"{id}\"\nversion = \"0.1.0\"\nname = \"Demo\"\nstatus = \"ready\"\n"
        ),
    )
    .unwrap();
    fs::write(root.join("generated/index.toml"), trait_doc(id, summary)).unwrap();
}

/// Add by relative path, inspect the JSON report and committed
/// manifest/lock bytes, resolve/list the vendored trait, and confirm no
/// absolute temporary path leaks into committed evidence.
#[test]
fn path_dependency_installs_and_resolves_without_leaking_absolute_paths() {
    let scratch = ScratchRoot::new("path-distribution-basic");
    let home = scratch.home();
    let producer = home.join("producer/demo");
    write_producer_package(&producer, "path-demo", "v1");

    let consumer = home.join("consumer");
    fs::create_dir_all(&consumer).unwrap();
    git_init(&consumer);

    let add_stdout = require_success(
        "`ctx traits dependency add path:../producer/demo --json`",
        &[
            "traits",
            "dependency",
            "add",
            "path:../producer/demo",
            "--json",
        ],
        &consumer,
        &home,
    );
    assert!(
        add_stdout.contains("\"transport\": \"path\""),
        "install report did not report path transport: {add_stdout}"
    );
    assert!(
        add_stdout.contains("\"path\": \"../producer/demo\""),
        "install report did not record the authored relative path: {add_stdout}"
    );
    assert!(
        !add_stdout.contains("\"resolved-version\"")
            && !add_stdout.contains("\"integrity\"")
            && !add_stdout.contains("\"requested\""),
        "path install report must not print npm-only fields at all, even empty: {add_stdout}"
    );

    let outdated_stdout = require_success(
        "`ctx traits dependency outdated --json`",
        &["traits", "dependency", "outdated", "--json"],
        &consumer,
        &home,
    );
    assert!(
        outdated_stdout.contains("\"transport\": \"path\""),
        "outdated must report the path dependency instead of silently dropping it: {outdated_stdout}"
    );
    assert!(
        outdated_stdout.contains("\"drift\": false"),
        "outdated must report no drift for an unchanged path source: {outdated_stdout}"
    );
    assert!(
        !outdated_stdout.contains("\"wanted\"") && !outdated_stdout.contains("\"latest\""),
        "path outdated row must not print npm-only version fields: {outdated_stdout}"
    );

    // The live JSON report's `vendored-path` is an absolute filesystem path
    // (matching pre-P535 npm install reports) — a reporting convenience,
    // never committed. Committed evidence (manifest, lock) is what must
    // never carry an absolute temporary path.
    let manifest_text = fs::read_to_string(consumer.join(".ctx/traits/config.toml")).unwrap();
    assert!(
        manifest_text.contains("path = \"../producer/demo\""),
        "committed manifest does not record the path: dependency: {manifest_text}"
    );
    assert!(
        !manifest_text.contains(home.to_str().unwrap()),
        "committed manifest leaked an absolute temporary path: {manifest_text}"
    );

    let lock_text = fs::read_to_string(consumer.join(".ctx/traits/config.lock")).unwrap();
    assert!(
        lock_text.contains("transport = \"path\""),
        "committed lock does not record the path transport: {lock_text}"
    );
    assert!(
        lock_text.contains("path = \"../producer/demo\""),
        "committed lock does not record the authored relative path: {lock_text}"
    );
    assert!(
        !lock_text.contains("integrity") && !lock_text.contains("resolved-version"),
        "path lock entry must not fabricate npm SRI/registry evidence: {lock_text}"
    );
    assert!(
        !lock_text.contains(home.to_str().unwrap()),
        "committed lock leaked an absolute temporary path: {lock_text}"
    );

    let vendored = consumer.join(".ctx/traits/vendored/demo/generated/index.toml");
    assert!(vendored.is_file(), "vendored trait file was not written");

    let list_stdout = require_success(
        "`ctx traits list --json`",
        &["traits", "list", "--json"],
        &consumer,
        &home,
    );
    assert!(
        list_stdout.contains("\"id\": \"path-demo\""),
        "vendored trait did not resolve through the inventory: {list_stdout}"
    );

    let check = run_ctx(
        &["traits", "check", "path-demo", "--json"],
        &consumer,
        &home,
    );
    let (check_stdout, check_stderr) = utf8(&check);
    assert!(
        check.status.success(),
        "vendored path-transport trait failed to check\nstdout: {check_stdout}\nstderr: {check_stderr}"
    );
}

/// Ordinary reconciliation (`dependency install`) never propagates a
/// producer rebuild; only an explicit `dependency update <alias>` accepts
/// the new source bytes and rewrites the vendored snapshot.
#[test]
fn ordinary_reconcile_ignores_producer_rebuild_but_explicit_update_accepts_it() {
    let scratch = ScratchRoot::new("path-distribution-propagation");
    let home = scratch.home();
    let producer = home.join("producer/demo");
    write_producer_package(&producer, "path-demo", "v1");

    let consumer = home.join("consumer");
    fs::create_dir_all(&consumer).unwrap();
    git_init(&consumer);

    require_success(
        "`ctx traits dependency add path:../producer/demo`",
        &["traits", "dependency", "add", "path:../producer/demo"],
        &consumer,
        &home,
    );
    let vendored = consumer.join(".ctx/traits/vendored/demo/generated/index.toml");
    let vendored_before = fs::read_to_string(&vendored).unwrap();
    assert!(vendored_before.contains("v1"));

    // Producer rebuilds to a new version.
    write_producer_package(&producer, "path-demo", "v2");

    // Ordinary reconciliation (`dependency install` with no operand) must
    // leave the consumer's vendored copy untouched.
    require_success(
        "`ctx traits dependency install`",
        &["traits", "dependency", "install"],
        &consumer,
        &home,
    );
    let vendored_after_install = fs::read_to_string(&vendored).unwrap();
    assert_eq!(
        vendored_before, vendored_after_install,
        "ordinary reconciliation must not propagate a producer rebuild"
    );

    // Explicit update is the sole path that accepts the new bytes.
    require_success(
        "`ctx traits dependency update demo --json`",
        &["traits", "dependency", "update", "demo", "--json"],
        &consumer,
        &home,
    );
    let lock_text = fs::read_to_string(consumer.join(".ctx/traits/config.lock")).unwrap();
    assert!(
        !lock_text.contains(home.to_str().unwrap()),
        "committed lock leaked an absolute temporary path after update: {lock_text}"
    );
    let vendored_after_update = fs::read_to_string(&vendored).unwrap();
    assert!(
        vendored_after_update.contains("v2"),
        "explicit update did not adopt the rebuilt producer source: {vendored_after_update}"
    );
}

/// Installing the folded `.ctx/traits/authored/implement/`-shaped native
/// family package by relative path vendors every declared variant, not just
/// one — and every variant shares one `id`, exactly the real folded packages'
/// shape (differentiated by `variant`, never by encoding the variant name into
/// the id). `family-demo` must resolve the declared default variant,
/// `family-demo:quick` must resolve the quick variant, and the legacy
/// hyphenated alias `family-demo-quick` must resolve the same quick variant —
/// all from the vendored package, with distinct canonical digests.
#[test]
fn path_dependency_installs_every_family_variant() {
    let scratch = ScratchRoot::new("path-distribution-family");
    let home = scratch.home();
    let producer = home.join("producer/family-demo");
    fs::create_dir_all(producer.join("generated/quick")).unwrap();
    fs::create_dir_all(producer.join("generated/default")).unwrap();
    fs::write(
        producer.join("trait.toml"),
        "[package]\n\
         id = \"family-demo\"\n\
         version = \"0.1.0\"\n\
         name = \"Family Demo\"\n\
         status = \"ready\"\n\
         \n\
         [family]\n\
         default = \"default\"\n\
         \n\
         [family.variant.default]\n\
         path = \"generated/default/index.toml\"\n\
         \n\
         [family.variant.quick]\n\
         path = \"generated/quick/index.toml\"\n\
         aliases = [\"family-demo-quick\"]\n",
    )
    .unwrap();
    fs::write(
        producer.join("generated/default/index.toml"),
        format!(
            "{}\n[[resource]]\nid = \"default-resource\"\npath = \"resources/default.txt\"\ntrigger = \"on-demand\"\n\n[[resource]]\nid = \"consumer-resource\"\npath = \"consumer-resource.txt\"\nroot = \"repo\"\ntrigger = \"on-demand\"\n",
            family_variant_trait_doc("family-demo", "default", "default variant")
        ),
    )
    .unwrap();
    fs::write(
        producer.join("generated/quick/index.toml"),
        format!(
            "{}\n[[resource]]\nid = \"quick-resource\"\npath = \"resources/quick.txt\"\ntrigger = \"on-demand\"\n",
            family_variant_trait_doc("family-demo", "quick", "quick variant")
        ),
    )
    .unwrap();
    fs::create_dir_all(producer.join("resources")).unwrap();
    fs::write(producer.join("resources/default.txt"), "default resource\n").unwrap();
    fs::write(producer.join("resources/quick.txt"), "quick resource\n").unwrap();

    let consumer = home.join("consumer");
    fs::create_dir_all(&consumer).unwrap();
    git_init(&consumer);
    fs::write(
        consumer.join("consumer-resource.txt"),
        "consumer-owned resource\n",
    )
    .unwrap();

    let add_stdout = require_success(
        "`ctx traits dependency add path:../producer/family-demo --alias family-install --json`",
        &[
            "traits",
            "dependency",
            "add",
            "path:../producer/family-demo",
            "--alias",
            "family-install",
            "--json",
        ],
        &consumer,
        &home,
    );
    let id_occurrences = add_stdout.matches("\"id\": \"family-demo\"").count();
    assert!(
        id_occurrences >= 2,
        "install report is missing one of the two family-demo variant entries: {add_stdout}"
    );
    let vendored_root = consumer.join(".ctx/traits/vendored/family-install");
    for path in [
        "trait.toml",
        "generated/default/index.toml",
        "generated/quick/index.toml",
        "resources/default.txt",
        "resources/quick.txt",
    ] {
        assert!(
            vendored_root.join(path).is_file(),
            "package install did not mirror {path}"
        );
    }
    let lock_text = fs::read_to_string(consumer.join(".ctx/traits/config.lock")).unwrap();
    let canonical_digests = lock_text
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("canonical-digest = \"")?
                .strip_suffix('"')
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        canonical_digests.len(),
        2,
        "lock evidence must contain distinct canonical digests for both variants: {lock_text}"
    );

    let list_stdout = require_success(
        "`ctx traits list --json`",
        &["traits", "list", "--json"],
        &consumer,
        &home,
    );
    assert!(list_stdout.contains("\"id\": \"family-demo\""));
    assert!(
        list_stdout.contains("\"status\": \"ready\""),
        "vendored family did not retain package status=ready: {list_stdout}"
    );

    // Bare id resolves the declared default variant.
    let default_check = run_ctx(
        &["traits", "check", "family-demo", "--json"],
        &consumer,
        &home,
    );
    let (default_stdout, default_stderr) = utf8(&default_check);
    assert!(
        default_check.status.success(),
        "default variant failed to check\nstdout: {default_stdout}\nstderr: {default_stderr}"
    );
    let default_digest = canonical_digest_from_check(&default_stdout);

    // `family:variant` resolves the named variant.
    let quick_check = run_ctx(
        &["traits", "check", "family-demo:quick", "--json"],
        &consumer,
        &home,
    );
    let (quick_stdout, quick_stderr) = utf8(&quick_check);
    assert!(
        quick_check.status.success(),
        "family-demo:quick failed to check\nstdout: {quick_stdout}\nstderr: {quick_stderr}"
    );
    let quick_digest = canonical_digest_from_check(&quick_stdout);
    assert_ne!(
        default_digest, quick_digest,
        "bare id and family-demo:quick must resolve different canonical digests"
    );

    // The legacy hyphenated alias resolves the same quick variant.
    let alias_check = run_ctx(
        &["traits", "check", "family-demo-quick", "--json"],
        &consumer,
        &home,
    );
    let (alias_stdout, alias_stderr) = utf8(&alias_check);
    assert!(
        alias_check.status.success(),
        "legacy alias family-demo-quick failed to check\nstdout: {alias_stdout}\nstderr: {alias_stderr}"
    );
    let alias_digest = canonical_digest_from_check(&alias_stdout);
    assert_eq!(
        quick_digest, alias_digest,
        "legacy alias family-demo-quick did not resolve the same quick variant as family-demo:quick"
    );

    for reference in ["family-demo", "family-demo:quick"] {
        require_success(
            &format!("`ctx traits trust --approved {reference}`"),
            &["traits", "trust", "--approved", reference],
            &consumer,
            &home,
        );
    }
}
