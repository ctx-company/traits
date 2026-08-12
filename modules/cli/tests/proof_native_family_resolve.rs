//! P531 Stage 1: `family:variant` resolves straight to a native family
//! package's variant (`generated/<name>/index.toml`) via the `[family]`
//! table `ctx traits build` writes (P530 Stage B) — not the legacy
//! `family-variant` sibling-directory shape. Reuses the `family-fixture`
//! source from `proof_cdk_native_family_build.rs`.

use std::fs;

use support::{ScratchRoot, git_init, run_ctx, symlink_node_modules, utf8};

fn family_fixture_source() -> &'static str {
    "import { agent, input, port, procedure, sequence, slot, trait, variant } from \"@ctx-traits/cdk\";\n\
\n\
const summary = slot.text(\"summary\");\n\
const output = port.output.text({ id: \"summary\", value: summary });\n\
const worker = agent(\"worker\", { description: \"Completes the starter task.\" });\n\
\n\
const variantFixture = (name) => variant({\n\
  name,\n\
  summary: `The ${name} variant.`,\n\
  procedure: procedure({\n\
    description: \"Describe what this trait should accomplish.\",\n\
    output,\n\
    sequence: sequence.prompt({\n\
      id: \"run\",\n\
      agent: worker,\n\
      prompt: input.prompt`Describe the task for this trait.`,\n\
      output: summary,\n\
    }),\n\
  }),\n\
});\n\
\n\
export const draft = trait(\"family-fixture\", {\n\
  variants: {\n\
    default: variantFixture(\"default\").default(),\n\
    quick: variantFixture(\"quick\"),\n\
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

    // What this file proves is variant resolution and start-time trust, not
    // harness discovery — so the fixture's one role is pinned rather than
    // left to probe PATH. Unpinned, `ctx traits run` refuses before it ever
    // reaches the trust check on any machine with no coding agent installed,
    // which is every CI runner and no developer laptop.
    fs::write(
        proj.join(".ctx/config.toml"),
        "[agent.role.worker]\nharness = \"claude-code\"\n",
    )
    .unwrap();

    let source_path = proj.join(format!(".ctx/traits/authored/{trait_id}/source/index.ts"));
    fs::write(&source_path, family_fixture_source())
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", source_path.display()));

    let build = run_ctx(
        &[
            "traits",
            "build",
            &format!(".ctx/traits/authored/{trait_id}/source/index.ts"),
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

fn add_family_variant_dependencies(proj: &std::path::Path) {
    for (variant, alias) in [
        ("default", "family-dep-default"),
        ("quick", "family-dep-quick"),
    ] {
        let dependency_root = proj.join(format!(".ctx/traits/authored/{alias}"));
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

        let variant_path = proj.join(format!(
            ".ctx/traits/authored/family-fixture/generated/{variant}/index.toml"
        ));
        let mut variant_text = fs::read_to_string(&variant_path).unwrap();
        variant_text.push_str(&format!(
            "\n[[dependency]]\nalias = \"{alias}\"\nid = \"{alias}\"\nversion = \"0.1.0\"\n\n[dependency.source]\npath = \"../{alias}\"\n"
        ));
        fs::write(variant_path, variant_text).unwrap();
    }
}

/// `family:quick` resolves straight to the family package's
/// `generated/quick/index.toml` variant — never to a `family-quick` sibling
/// directory, which does not exist in this fixture.
#[test]
fn variant_ref_resolves_to_family_variant() {
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
        "expected `ctx traits check family-fixture:quick` to resolve the quick variant\nstdout: {stdout}\nstderr: {stderr}"
    );
}

/// Bare `family-fixture` (and `family-fixture:default`) resolve via
/// `[family].default` to the same variant.
#[test]
fn bare_id_resolves_to_family_default_variant() {
    let scratch = ScratchRoot::new("native-family-resolve-default");
    let home = scratch.home();
    let proj = home.join("repo");
    build_family_fixture(&proj, &home);

    for id in ["family-fixture", "family-fixture:default"] {
        let check = run_ctx(&["traits", "check", id, "--json"], &proj, &home);
        let (stdout, stderr) = utf8(&check);
        assert!(
            check.status.success(),
            "expected `ctx traits check {id}` to resolve the default variant\nstdout: {stdout}\nstderr: {stderr}"
        );
    }
}

/// Every published variant alias resolves through the family manifest after the
/// former sibling package has disappeared.
#[test]
fn legacy_hyphenated_alias_resolves_to_family_variant() {
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

/// A package still on the pre-rename manifest shape (`[family.leaf.<selector>]`
/// instead of `[family.variant.<name>]`) resolves exactly like a current one,
/// by name and by legacy alias — the compat read 0026 requires so an
/// already-published package never breaks underfoot.
#[test]
fn legacy_leaf_table_manifest_still_resolves() {
    let scratch = ScratchRoot::new("native-family-resolve-legacy-leaf-table");
    let home = scratch.home();
    let proj = home.join("repo");
    build_family_fixture(&proj, &home);

    let manifest_path = proj.join(".ctx/traits/authored/family-fixture/trait.toml");
    let manifest_text = fs::read_to_string(&manifest_path).unwrap();
    let legacy_text = manifest_text
        .replace("[family.variant.", "[family.leaf.")
        .replace("[family.variant]", "[family.leaf]");
    assert_ne!(
        legacy_text, manifest_text,
        "fixture manifest must contain [family.variant.*] tables to downgrade"
    );
    fs::write(&manifest_path, &legacy_text).unwrap();

    let check = run_ctx(
        &["traits", "check", "family-fixture:quick", "--json"],
        &proj,
        &home,
    );
    let (stdout, stderr) = utf8(&check);
    assert!(
        check.status.success(),
        "expected `family-fixture:quick` to resolve against a legacy [family.leaf.*] manifest\nstdout: {stdout}\nstderr: {stderr}"
    );

    let alias_check = run_ctx(
        &["traits", "check", "family-fixture-quick", "--json"],
        &proj,
        &home,
    );
    let (stdout, stderr) = utf8(&alias_check);
    assert!(
        alias_check.status.success(),
        "expected the legacy alias to resolve against a legacy [family.leaf.*] manifest\nstdout: {stdout}\nstderr: {stderr}"
    );
}

/// A variant name absent from the family's `[family]` table produces the
/// typo/variant diagnostic listing the family's real variants (read from
/// `[family]`, not scraped `-suffix` directories, since none exist here).
#[test]
fn unknown_variant_lists_family_variants() {
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
        "expected the diagnostic to list the family's variants (default, quick): {stderr}"
    );
}

/// Operand-less vendor expands a native package root to every canonical variant,
/// records both variants in one package lock, preserves variant-local evidence,
/// and verifies the resulting lock without one variant overwriting another.
#[test]
fn operandless_vendor_locks_every_family_variant() {
    let scratch = ScratchRoot::new("native-family-vendor");
    let home = scratch.home();
    let proj = home.join("repo");
    build_family_fixture(&proj, &home);
    add_family_variant_dependencies(&proj);

    let vendor = run_ctx(&["traits", "vendor", "--json"], &proj, &home);
    let (stdout, stderr) = utf8(&vendor);
    assert!(
        vendor.status.success(),
        "operand-less vendor failed\nstdout: {stdout}\nstderr: {stderr}"
    );
    let lock_path = proj.join(".ctx/traits/authored/family-fixture/trait.lock");
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
    assert!(
        proj.join(".ctx/traits/vendored/family-dep-default")
            .is_dir()
    );
    assert!(proj.join(".ctx/traits/vendored/family-dep-quick").is_dir());

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
    let package_manifest = proj.join(".ctx/traits/authored/family-fixture/trait.toml");
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
            "approved {variant} variant failed start-time trust\nstdout: {stdout}\nstderr: {stderr}"
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

/// With only an ordinary `<id>-default` package installed (no `<id>` package
/// at all), a bare `<id>` reference must stay unresolved rather than
/// silently falling back to the unrelated `-default` sibling — that legacy
/// suffix fallback exists only for an *explicit* `<id>:default` reference
/// (P535 review blocker: bare-id-unintentionally-gains-default-suffix-fallback).
#[test]
fn bare_id_does_not_fall_back_to_unrelated_default_suffix_package() {
    let scratch = ScratchRoot::new("bare-id-no-default-suffix-fallback");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    symlink_node_modules(&proj);

    let ordinary_id = "standalone-default";
    let init = run_ctx(&["traits", "init", ordinary_id], &proj, &home);
    assert!(init.status.success(), "init failed: {}", utf8(&init).1);
    let build = run_ctx(
        &[
            "traits",
            "build",
            &format!(".ctx/traits/authored/{ordinary_id}/source/index.ts"),
        ],
        &proj,
        &home,
    );
    assert!(build.status.success(), "build failed: {}", utf8(&build).1);

    // No `standalone` package exists at all: the bare id must not resolve
    // to the unrelated `standalone-default` package.
    let bare = run_ctx(&["traits", "check", "standalone", "--json"], &proj, &home);
    let (stdout, stderr) = utf8(&bare);
    assert!(
        !bare.status.success(),
        "bare `standalone` unexpectedly resolved via the `-default` suffix fallback\nstdout: {stdout}\nstderr: {stderr}"
    );

    // The explicit `:default` reference still uses the legacy suffix
    // fallback, since that behavior predates P535.
    let explicit = run_ctx(
        &["traits", "check", "standalone:default", "--json"],
        &proj,
        &home,
    );
    let (stdout, stderr) = utf8(&explicit);
    assert!(
        explicit.status.success(),
        "expected `standalone:default` to resolve via the legacy `-default` suffix fallback\nstdout: {stdout}\nstderr: {stderr}"
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
            ".ctx/traits/authored/source-less/source/index.ts",
        ],
        &proj,
        &home,
    );
    assert!(build.status.success(), "build failed: {}", utf8(&build).1);
    fs::remove_dir_all(proj.join(".ctx/traits/authored/source-less/source")).unwrap();

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
    let rebuild = run_ctx(&["traits", "build", "source-less"], &proj, &home);
    let (stdout, stderr) = utf8(&rebuild);
    assert!(
        !rebuild.status.success(),
        "named build unexpectedly succeeded without an authoring source\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("resolves to")
            && stderr.contains("no TypeScript or JavaScript authoring source"),
        "named build must explain the source-less package refusal:\n{stderr}"
    );
}
