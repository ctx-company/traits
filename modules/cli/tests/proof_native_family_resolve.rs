//! P531 Stage 1: `family:variant` resolves straight to a native family
//! package's leaf (`generated/<selector>/index.toml`) via the `[family]`
//! table `ctx traits build` writes (P530 Stage B) — not the legacy
//! `family-variant` sibling-directory shape. Reuses the `family-fixture`
//! source from `proof_cdk_native_family_build.rs`.

use std::fs;

use support::{ScratchRoot, git_init, run_ctx, symlink_node_modules, utf8};

fn family_fixture_source() -> &'static str {
    "import { agent, port, procedure, prompt, sequence, slot, trait, variant } from \"@ctx-traits/cdk\";\n\
\n\
const summary = slot.text(\"summary\");\n\
const output = port.output.text({ id: \"summary\", value: summary });\n\
const worker = agent(\"worker\", { description: \"Completes the starter task.\" });\n\
\n\
const leaf = (name) => variant({\n\
  name,\n\
  summary: `The ${name} leaf.`,\n\
  procedure: procedure({\n\
    description: \"Describe what this trait should accomplish.\",\n\
    output,\n\
    sequence: sequence.prompt({\n\
      id: \"run\",\n\
      agent: worker,\n\
      prompt: prompt.text`Describe the task for this trait.`,\n\
      output: summary,\n\
    }),\n\
  }),\n\
});\n\
\n\
export const draft = trait(\"family-fixture\", {\n\
  variants: {\n\
    default: leaf(\"default\").default(),\n\
    quick: leaf(\"quick\"),\n\
  },\n\
});\n"
}

fn build_family_fixture(proj: &std::path::Path, home: &std::path::Path) {
    fs::create_dir_all(proj).unwrap();
    git_init(proj);
    symlink_node_modules(proj);

    let trait_id = "family-fixture";
    let init = run_ctx(&["traits", "init", trait_id], proj, home);
    assert!(
        init.status.success(),
        "`ctx traits init {trait_id}` failed: {}",
        utf8(&init).1
    );

    let source_path = proj.join(format!(".ctx/traits/packages/{trait_id}/source/index.ts"));
    fs::write(&source_path, family_fixture_source())
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", source_path.display()));

    let build = run_ctx(
        &[
            "traits",
            "build",
            &format!(".ctx/traits/packages/{trait_id}/source/index.ts"),
        ],
        proj,
        home,
    );
    let (stdout, stderr) = utf8(&build);
    assert!(
        build.status.success(),
        "expected `ctx traits build` to publish a native family\nstdout: {stdout}\nstderr: {stderr}"
    );
}

fn add_family_leaf_dependencies(proj: &std::path::Path) {
    for (variant, alias) in [
        ("default", "family-dep-default"),
        ("quick", "family-dep-quick"),
    ] {
        let dependency_root = proj.join(format!(".ctx/traits/packages/{alias}"));
        fs::create_dir_all(dependency_root.join("generated")).unwrap();
        fs::write(
            dependency_root.join("trait.toml"),
            format!(
                "[package]\nid = \"{alias}\"\nversion = \"0.1.0\"\nname = \"{alias}\"\nstatus = \"ready\"\n"
            ),
        )
        .unwrap();
        fs::write(
            dependency_root.join("generated/index.toml"),
            format!(
                "id = \"{alias}\"\nschema-version = \"0.2\"\nversion = \"0.1.0\"\nname = \"{alias}\"\nsummary = \"Family vendor dependency fixture.\"\n"
            ),
        )
        .unwrap();

        let leaf_path = proj.join(format!(
            ".ctx/traits/packages/family-fixture/generated/{variant}/index.toml"
        ));
        let mut leaf = fs::read_to_string(&leaf_path).unwrap();
        leaf.push_str(&format!(
            "\n[[dependency]]\nalias = \"{alias}\"\nid = \"{alias}\"\nversion = \"0.1.0\"\n\n[dependency.source]\npath = \"../{alias}\"\n"
        ));
        fs::write(leaf_path, leaf).unwrap();
    }
}

/// `family:quick` resolves straight to the family package's
/// `generated/quick/index.toml` leaf — never to a `family-quick` sibling
/// directory, which does not exist in this fixture.
#[test]
fn variant_ref_resolves_to_family_leaf() {
    let scratch = ScratchRoot::new("native-family-resolve-variant");
    let home = scratch.home();
    let proj = home.join("repo");
    build_family_fixture(&proj, &home);

    let check = run_ctx(
        &["traits", "check", "family-fixture:quick", "--json"],
        &proj,
        &home,
    );
    let (stdout, stderr) = utf8(&check);
    assert!(
        check.status.success(),
        "expected `ctx traits check family-fixture:quick` to resolve the quick leaf\nstdout: {stdout}\nstderr: {stderr}"
    );
}

/// Bare `family-fixture` (and `family-fixture:default`) resolve via
/// `[family].default` to the same leaf.
#[test]
fn bare_id_resolves_to_family_default_leaf() {
    let scratch = ScratchRoot::new("native-family-resolve-default");
    let home = scratch.home();
    let proj = home.join("repo");
    build_family_fixture(&proj, &home);

    for id in ["family-fixture", "family-fixture:default"] {
        let check = run_ctx(&["traits", "check", id, "--json"], &proj, &home);
        let (stdout, stderr) = utf8(&check);
        assert!(
            check.status.success(),
            "expected `ctx traits check {id}` to resolve the default leaf\nstdout: {stdout}\nstderr: {stderr}"
        );
    }
}

/// Every published leaf alias resolves through the family manifest after the
/// former sibling package has disappeared.
#[test]
fn legacy_hyphenated_alias_resolves_to_family_leaf() {
    let scratch = ScratchRoot::new("native-family-resolve-alias");
    let home = scratch.home();
    let proj = home.join("repo");
    build_family_fixture(&proj, &home);

    for id in ["family-fixture-default", "family-fixture-quick"] {
        let check = run_ctx(&["traits", "check", id, "--json"], &proj, &home);
        let (stdout, stderr) = utf8(&check);
        assert!(
            check.status.success(),
            "expected {id} to resolve through the family manifest\nstdout: {stdout}\nstderr: {stderr}"
        );
    }
}

/// A variant name absent from the family's `[family]` table produces the
/// typo/variant diagnostic listing the family's real leaves (read from
/// `[family]`, not scraped `-suffix` directories, since none exist here).
#[test]
fn unknown_variant_lists_family_leaves() {
    let scratch = ScratchRoot::new("native-family-resolve-unknown-variant");
    let home = scratch.home();
    let proj = home.join("repo");
    build_family_fixture(&proj, &home);

    let check = run_ctx(
        &["traits", "check", "family-fixture:bogus", "--json"],
        &proj,
        &home,
    );
    let (stdout, stderr) = utf8(&check);
    assert!(
        !check.status.success(),
        "expected `ctx traits check family-fixture:bogus` to fail\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("default") && stderr.contains("quick"),
        "expected the diagnostic to list the family's leaves (default, quick): {stderr}"
    );
}

/// Operand-less vendor expands a native package root to every canonical leaf,
/// records both variants in one package lock, preserves leaf-local evidence,
/// and verifies the resulting lock without one variant overwriting another.
#[test]
fn operandless_vendor_locks_every_family_leaf() {
    let scratch = ScratchRoot::new("native-family-vendor");
    let home = scratch.home();
    let proj = home.join("repo");
    build_family_fixture(&proj, &home);
    add_family_leaf_dependencies(&proj);

    let vendor = run_ctx(&["traits", "vendor", "--json"], &proj, &home);
    let (stdout, stderr) = utf8(&vendor);
    assert!(
        vendor.status.success(),
        "operand-less vendor failed\nstdout: {stdout}\nstderr: {stderr}"
    );
    let lock_path = proj.join(".ctx/traits/packages/family-fixture/package.lock");
    assert!(
        lock_path.is_file(),
        "vendor did not create {}\nstdout: {stdout}\nstderr: {stderr}",
        lock_path.display()
    );
    let lock_text = fs::read_to_string(&lock_path).unwrap();
    assert_eq!(lock_text.matches("id = \"family-fixture\"").count(), 2);
    assert!(lock_text.contains("variant = \"default\""));
    assert!(lock_text.contains("variant = \"quick\""));
    assert!(lock_text.contains("alias = \"family-dep-default\""));
    assert!(lock_text.contains("alias = \"family-dep-quick\""));
    assert!(proj.join(".ctx/traits/vendor/family-dep-default").is_dir());
    assert!(proj.join(".ctx/traits/vendor/family-dep-quick").is_dir());

    fs::write(
        &lock_path,
        format!(
            "{lock_text}\n[[trait.export]]\ntarget = \"proof\"\npath = \"proof.txt\"\ndigest = \"sha256:proof\"\n\n[[trait.projection]]\ntarget-profile = \"agent-skills\"\nrenderer-version = \"proof\"\nprofile-version = \"proof\"\n[trait.projection.digests]\nsource = \"sha256:proof\"\ncanonical = \"sha256:proof\"\nmodel-visible = \"sha256:proof\"\nrender = \"sha256:proof\"\n[trait.projection.static-output]\ndrift-status = \"output-not-written\"\n[trait.projection.provenance]\ncommand = \"proof\"\nprofile = \"proof\"\n"
        ),
    )
    .unwrap();

    let refresh = run_ctx(&["traits", "vendor", "--json"], &proj, &home);
    assert!(
        refresh.status.success(),
        "vendor refresh failed: {}",
        utf8(&refresh).1
    );
    let refreshed = fs::read_to_string(&lock_path).unwrap();
    assert!(refreshed.contains("target = \"proof\""));
    assert!(refreshed.contains("command = \"proof\""));
    assert_eq!(refreshed.matches("id = \"family-fixture\"").count(), 2);

    let locked = run_ctx(&["traits", "vendor", "--locked", "--json"], &proj, &home);
    let (stdout, stderr) = utf8(&locked);
    assert!(
        locked.status.success(),
        "operand-less vendor --locked failed\nstdout: {stdout}\nstderr: {stderr}"
    );

    let approve = run_ctx(
        &["traits", "trust", "approve", "--all-current", "--json"],
        &proj,
        &home,
    );
    let (stdout, stderr) = utf8(&approve);
    assert!(
        approve.status.success(),
        "bulk trust approval failed\nstdout: {stdout}\nstderr: {stderr}"
    );
    let package_manifest = proj.join(".ctx/traits/packages/family-fixture/package.toml");
    let package_text = fs::read_to_string(&package_manifest).unwrap();
    fs::write(
        package_manifest,
        package_text.replace("status = \"draft\"", "status = \"ready\""),
    )
    .unwrap();
    for variant in ["default", "quick"] {
        let start = run_ctx(
            &[
                "traits",
                "run",
                &format!("family-fixture:{variant}"),
                "--no-drive",
                "--ephemeral",
                "--json",
            ],
            &proj,
            &home,
        );
        let (stdout, stderr) = utf8(&start);
        assert!(
            start.status.success(),
            "approved {variant} leaf failed start-time trust\nstdout: {stdout}\nstderr: {stderr}"
        );
    }
    let stale = run_ctx(
        &["traits", "trust", "list", "--stale", "--json"],
        &proj,
        &home,
    );
    let (stdout, stderr) = utf8(&stale);
    assert!(
        stale.status.success(),
        "trust list --stale failed\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        !stdout.contains("family-fixture"),
        "current family approvals were classified stale: {stdout}"
    );
}

/// The family-aware package sweep must not turn CDK source into a package
/// requirement: an ordinary authored package remains visible and checkable
/// after its canonical output is retained but its source tree is absent.
#[test]
fn source_less_canonical_package_remains_valid() {
    let scratch = ScratchRoot::new("source-less-package");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    symlink_node_modules(&proj);
    let init = run_ctx(&["traits", "init", "source-less"], &proj, &home);
    assert!(init.status.success(), "init failed: {}", utf8(&init).1);
    let build = run_ctx(
        &[
            "traits",
            "build",
            ".ctx/traits/packages/source-less/source/index.ts",
        ],
        &proj,
        &home,
    );
    assert!(build.status.success(), "build failed: {}", utf8(&build).1);
    fs::remove_dir_all(proj.join(".ctx/traits/packages/source-less/source")).unwrap();

    let list = run_ctx(&["traits", "list", "--json"], &proj, &home);
    let (stdout, stderr) = utf8(&list);
    assert!(
        list.status.success() && stdout.contains("\"id\": \"source-less\""),
        "source-less package disappeared from list\nstdout: {stdout}\nstderr: {stderr}"
    );
    let check = run_ctx(&["traits", "check", "source-less", "--json"], &proj, &home);
    let (stdout, stderr) = utf8(&check);
    assert!(
        check.status.success(),
        "source-less package check failed\nstdout: {stdout}\nstderr: {stderr}"
    );
}
