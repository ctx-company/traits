//! P530 Stage B: `ctx traits build` publishes a native trait family
//! (`trait(id, { variants })`, `packages/cdk/src/variant.ts`'s
//! `resolveTraitFamily`) by writing each variant's canonical output under
//! `generated/<name>/` and refreshing the package's `[family]` manifest
//! table — not just detecting the envelope.

use std::fs;

use support::{ScratchRoot, git_init, repo_root, run_ctx, symlink_node_modules, utf8};

fn family_fixture_source() -> &'static str {
    "import { agent, port, procedure, prompt, sequence, slot, trait, variant } from \"@ctx-traits/cdk\";\n\
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
      prompt: prompt.text`Describe the task for this trait.`,\n\
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

/// `ctx traits build` on a `trait(id, { variants })` family source publishes
/// every variant's canonical output under `generated/<name>/index.toml`
/// (+ `index.map`) and writes a `[family]` table into the package's root
/// `trait.toml` naming the default variant and every variant's generated path and
/// legacy alias.
#[test]
fn build_publishes_native_family_variants_and_manifest_table() {
    let scratch = ScratchRoot::new("cdk-native-family-build");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);

    #[cfg(unix)]
    std::os::unix::fs::symlink(repo_root().join("node_modules"), proj.join("node_modules"))
        .unwrap_or_else(|error| panic!("cannot symlink node_modules: {error}"));
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(repo_root().join("node_modules"), proj.join("node_modules"))
        .unwrap_or_else(|error| panic!("cannot symlink node_modules: {error}"));

    let trait_id = "family-fixture";
    let init = run_ctx(&["traits", "init", trait_id], &proj, &home);
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
        &proj,
        &home,
    );
    let (stdout, stderr) = utf8(&build);
    assert!(
        build.status.success(),
        "expected `ctx traits build` to publish a native family\nstdout: {stdout}\nstderr: {stderr}"
    );

    let package_root = proj.join(format!(".ctx/traits/packages/{trait_id}"));
    for variant_name in ["default", "quick"] {
        let variant_toml = package_root.join(format!("generated/{variant_name}/index.toml"));
        let variant_map = package_root.join(format!("generated/{variant_name}/index.map"));
        assert!(
            variant_toml.is_file(),
            "expected {} to exist after build",
            variant_toml.display()
        );
        assert!(
            variant_map.is_file(),
            "expected {} to exist after build",
            variant_map.display()
        );
        let variant_text = fs::read_to_string(&variant_toml).unwrap();
        assert!(
            variant_text.contains("id = \"family-fixture\""),
            "variant {variant_name} canonical missing family id: {variant_text}"
        );
        assert!(
            variant_text.contains(&format!("variant = \"{variant_name}\"")),
            "variant {variant_name} canonical missing its own variant name: {variant_text}"
        );
    }

    let manifest_text = fs::read_to_string(package_root.join("package.toml")).unwrap();
    let manifest: toml::Value = toml::from_str(&manifest_text).unwrap();
    let family = manifest
        .get("family")
        .unwrap_or_else(|| panic!("trait.toml missing [family] table: {manifest_text}"));
    assert_eq!(
        family.get("default").and_then(toml::Value::as_str),
        Some("default"),
        "family.default should name the default variant: {manifest_text}"
    );
    let quick_variant = family
        .get("variant")
        .and_then(|variant| variant.get("quick"))
        .unwrap_or_else(|| panic!("trait.toml missing family.variant.quick: {manifest_text}"));
    assert_eq!(
        quick_variant.get("path").and_then(toml::Value::as_str),
        Some("generated/quick/index.toml"),
        "family.variant.quick.path should point at the quick variant's generated output: {manifest_text}"
    );
    let quick_aliases = quick_variant
        .get("aliases")
        .and_then(toml::Value::as_array)
        .unwrap_or_else(|| panic!("family.variant.quick.aliases missing: {manifest_text}"));
    assert!(
        quick_aliases
            .iter()
            .any(|alias| alias.as_str() == Some("family-fixture-quick")),
        "family.variant.quick.aliases should carry the legacy hyphenated id: {manifest_text}"
    );
    let default_variant = family
        .get("variant")
        .and_then(|variant| variant.get("default"))
        .unwrap_or_else(|| panic!("trait.toml missing family.variant.default: {manifest_text}"));
    assert!(
        default_variant
            .get("aliases")
            .and_then(toml::Value::as_array)
            .is_some_and(|aliases| aliases
                .iter()
                .any(|alias| alias.as_str() == Some("family-fixture-default"))),
        "the default variant must retain its legacy hyphenated alias: {manifest_text}"
    );
}

#[test]
fn building_a_variant_name_republishes_its_complete_native_family() {
    let scratch = ScratchRoot::new("cdk-native-family-build-variant-name");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    symlink_node_modules(&proj);

    let trait_id = "family-fixture";
    assert!(
        run_ctx(&["traits", "init", trait_id], &proj, &home)
            .status
            .success()
    );
    let source_path = proj.join(format!(".ctx/traits/packages/{trait_id}/source/index.ts"));
    fs::write(&source_path, family_fixture_source()).unwrap();
    assert!(
        run_ctx(
            &[
                "traits",
                "build",
                ".ctx/traits/packages/family-fixture/source/index.ts",
            ],
            &proj,
            &home,
        )
        .status
        .success()
    );

    fs::write(
        &source_path,
        family_fixture_source().replace("The ${name} variant.", "Rebuilt ${name} variant."),
    )
    .unwrap();
    let rebuild = run_ctx(&["traits", "build", "family-fixture:quick"], &proj, &home);
    let (stdout, stderr) = utf8(&rebuild);
    assert!(
        rebuild.status.success(),
        "named variant build failed\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("family: family-fixture")
            && stdout.contains("variant: default")
            && stdout.contains("variant: quick"),
        "named variant build did not report the complete family: {stdout}"
    );
    for variant_name in ["default", "quick"] {
        let variant = fs::read_to_string(proj.join(format!(
            ".ctx/traits/packages/{trait_id}/generated/{variant_name}/index.toml"
        )))
        .unwrap();
        assert!(
            variant.contains(&format!("summary = \"Rebuilt {variant_name} variant.\"")),
            "named variant build did not refresh {variant_name}: {variant}"
        );
    }
}

/// `ctx traits build` on a native family source refuses to publish when the
/// package has no root `trait.toml` at all: it must not create one
/// containing only `[family]` and no `[package]` table.
#[test]
fn build_refuses_native_family_with_no_package_manifest() {
    let scratch = ScratchRoot::new("cdk-native-family-no-manifest");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    symlink_node_modules(&proj);

    let trait_id = "family-fixture";
    let source_dir = proj.join(format!(".ctx/traits/packages/{trait_id}/source"));
    fs::create_dir_all(&source_dir).unwrap();
    fs::write(source_dir.join("index.ts"), family_fixture_source()).unwrap();

    let build = run_ctx(
        &[
            "traits",
            "build",
            &format!(".ctx/traits/packages/{trait_id}/source/index.ts"),
        ],
        &proj,
        &home,
    );
    let (stdout, stderr) = utf8(&build);
    assert!(
        !build.status.success(),
        "expected `ctx traits build` to refuse a family with no package manifest\nstdout: {stdout}\nstderr: {stderr}"
    );

    let package_root = proj.join(format!(".ctx/traits/packages/{trait_id}"));
    assert!(
        !package_root.join("package.toml").exists(),
        "build must not create a [family]-only trait.toml on refusal"
    );
    assert!(
        !package_root.join("generated").exists(),
        "build must not write any variant output on refusal"
    );
}

/// `ctx traits build` on a native family source refuses to publish when the
/// existing package manifest's `[package]` id disagrees with the family's
/// declared id — the same identity invariant the single-trait path enforces.
#[test]
fn build_refuses_native_family_with_mismatched_package_identity() {
    let scratch = ScratchRoot::new("cdk-native-family-mismatched-identity");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    symlink_node_modules(&proj);

    // Initialize a package under a different id than the family the source
    // declares, so [package].id disagrees with the built family id.
    let package_id = "other-package";
    let init = run_ctx(&["traits", "init", package_id], &proj, &home);
    assert!(
        init.status.success(),
        "`ctx traits init {package_id}` failed: {}",
        utf8(&init).1
    );

    let source_path = proj.join(format!(".ctx/traits/packages/{package_id}/source/index.ts"));
    fs::write(&source_path, family_fixture_source()).unwrap();

    let build = run_ctx(
        &[
            "traits",
            "build",
            &format!(".ctx/traits/packages/{package_id}/source/index.ts"),
        ],
        &proj,
        &home,
    );
    let (stdout, stderr) = utf8(&build);
    assert!(
        !build.status.success(),
        "expected `ctx traits build` to refuse a family/manifest identity mismatch\nstdout: {stdout}\nstderr: {stderr}"
    );

    let package_root = proj.join(format!(".ctx/traits/packages/{package_id}"));
    assert!(
        !package_root.join("generated").exists(),
        "build must not write any variant output on an identity-mismatch refusal"
    );
    let manifest_text = fs::read_to_string(package_root.join("package.toml")).unwrap();
    assert!(
        !manifest_text.contains("[family]"),
        "build must not write [family] into a manifest whose identity disagrees with it: {manifest_text}"
    );
}

fn assert_family_check(proj: &std::path::Path, home: &std::path::Path, passes: bool, case: &str) {
    let check = run_ctx(&["traits", "check", "family-fixture", "--json"], proj, home);
    let (stdout, stderr) = utf8(&check);
    assert_eq!(
        check.status.success(),
        passes,
        "unexpected family drift result for {case}\nstdout: {stdout}\nstderr: {stderr}"
    );
}

/// Checking the default variant verifies the complete synthesized family, so a
/// clean default cannot hide drift in another canonical, the topology table,
/// a missing variant, or another variant's source map.
#[test]
fn default_variant_check_covers_complete_family_drift() {
    let scratch = ScratchRoot::new("cdk-native-family-drift");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    symlink_node_modules(&proj);
    let init = run_ctx(&["traits", "init", "family-fixture"], &proj, &home);
    assert!(init.status.success(), "init failed: {}", utf8(&init).1);
    let source = proj.join(".ctx/traits/packages/family-fixture/source/index.ts");
    fs::write(&source, family_fixture_source()).unwrap();
    let build = run_ctx(
        &[
            "traits",
            "build",
            ".ctx/traits/packages/family-fixture/source/index.ts",
        ],
        &proj,
        &home,
    );
    assert!(build.status.success(), "build failed: {}", utf8(&build).1);
    assert_family_check(&proj, &home, true, "baseline");

    let root = proj.join(".ctx/traits/packages/family-fixture");
    let manifest = root.join("package.toml");
    let manifest_text = fs::read_to_string(&manifest).unwrap();
    let with_run_config = manifest_text.replace(
        "[family.variant.quick]\npath = \"generated/quick/index.toml\"",
        "[family.variant.quick]\npath = \"generated/quick/index.toml\"\nrun-config = \"run-config/quick.toml\"",
    );
    fs::create_dir_all(root.join("run-config")).unwrap();
    fs::write(
        root.join("run-config/quick.toml"),
        "schema-version = \"0.1\"\n\n[budget]\nmax-frames = 10\n",
    )
    .unwrap();
    fs::write(&manifest, &with_run_config).unwrap();
    let rebuild = run_ctx(
        &[
            "traits",
            "build",
            ".ctx/traits/packages/family-fixture/source/index.ts",
        ],
        &proj,
        &home,
    );
    assert!(
        rebuild.status.success(),
        "rebuild with authored run-config failed: {}",
        utf8(&rebuild).1
    );
    let post_rebuild_manifest = fs::read_to_string(&manifest).unwrap();
    assert!(
        !post_rebuild_manifest.contains("run-config"),
        "rebuild must stop emitting run-config declarations: {post_rebuild_manifest}"
    );
    let runtime_toml = fs::read_to_string(root.join("runtime.toml")).unwrap();
    assert!(
        runtime_toml.contains("[variant.quick]") && runtime_toml.contains("max-frames = 10"),
        "rebuild must consolidate the authored run-config into runtime.toml: {runtime_toml}"
    );
    assert_family_check(&proj, &home, true, "consolidated runtime.toml");

    let missing_alias = post_rebuild_manifest.replace("aliases = [\"family-fixture-quick\"]\n", "");
    assert_ne!(
        missing_alias, post_rebuild_manifest,
        "fixture must contain quick alias"
    );
    fs::write(&manifest, missing_alias).unwrap();
    assert_family_check(&proj, &home, false, "missing generated alias");
    fs::write(&manifest, &post_rebuild_manifest).unwrap();

    let quick = root.join("generated/quick/index.toml");
    let quick_text = fs::read_to_string(&quick).unwrap();
    fs::write(&quick, format!("{quick_text}\n# drift\n")).unwrap();
    assert_family_check(&proj, &home, false, "non-default canonical");
    fs::write(&quick, &quick_text).unwrap();

    fs::remove_file(&quick).unwrap();
    assert_family_check(&proj, &home, false, "missing variant");
    fs::write(&quick, &quick_text).unwrap();

    let map = root.join("generated/quick/index.map");
    let map_text = fs::read_to_string(&map).unwrap();
    fs::write(&map, format!("{map_text} ")).unwrap();
    assert_family_check(&proj, &home, false, "non-default source map");
    fs::write(&map, &map_text).unwrap();

    let expanded = family_fixture_source().replace(
        "quick: variantFixture(\"quick\"),",
        "quick: variantFixture(\"quick\"),\n    smart: variantFixture(\"smart\"),",
    );
    fs::write(&source, expanded).unwrap();
    assert_family_check(
        &proj,
        &home,
        false,
        "new synthesized variant absent from manifest",
    );
}

/// Consolidation must carry an authored `[defaults.port]` on the default
/// variant's declared run-config forward into `runtime.toml`, not silently
/// drop it — `[defaults.port]` is a live capability of the new shape, not
/// out-of-scope authored configuration.
#[test]
fn rebuild_consolidation_carries_default_variant_defaults_forward() {
    let scratch = ScratchRoot::new("cdk-native-family-consolidate-defaults");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    symlink_node_modules(&proj);
    let init = run_ctx(&["traits", "init", "family-fixture"], &proj, &home);
    assert!(init.status.success(), "init failed: {}", utf8(&init).1);
    let source = proj.join(".ctx/traits/packages/family-fixture/source/index.ts");
    fs::write(&source, family_fixture_source()).unwrap();
    let build = run_ctx(
        &[
            "traits",
            "build",
            ".ctx/traits/packages/family-fixture/source/index.ts",
        ],
        &proj,
        &home,
    );
    assert!(build.status.success(), "build failed: {}", utf8(&build).1);

    let root = proj.join(".ctx/traits/packages/family-fixture");
    let manifest = root.join("package.toml");
    let manifest_text = fs::read_to_string(&manifest).unwrap();
    let with_run_config = manifest_text.replace(
        "[family.variant.default]\npath = \"generated/default/index.toml\"",
        "[family.variant.default]\npath = \"generated/default/index.toml\"\nrun-config = \"run-config/default.toml\"",
    );
    assert_ne!(
        with_run_config, manifest_text,
        "fixture must declare a default variant path to rewrite"
    );
    fs::create_dir_all(root.join("run-config")).unwrap();
    fs::write(
        root.join("run-config/default.toml"),
        "schema-version = \"0.1\"\n\n[budget]\nmax-frames = 10\n\n[defaults.port]\nplan = \"sidecar\"\n",
    )
    .unwrap();
    fs::write(&manifest, &with_run_config).unwrap();
    let rebuild = run_ctx(
        &[
            "traits",
            "build",
            ".ctx/traits/packages/family-fixture/source/index.ts",
        ],
        &proj,
        &home,
    );
    assert!(
        rebuild.status.success(),
        "rebuild with authored default-variant defaults failed: {}",
        utf8(&rebuild).1
    );
    let runtime_toml = fs::read_to_string(root.join("runtime.toml")).unwrap();
    assert!(
        runtime_toml.contains("[defaults.port]") && runtime_toml.contains("plan = \"sidecar\""),
        "rebuild must carry the default variant's [defaults.port] into runtime.toml: {runtime_toml}"
    );
}

/// A non-default variant's declared run-config cannot carry `[defaults.port]`
/// forward — `runtime.toml`'s `[variant.<vid>]` tables are budget-only, so
/// this is unrepresentable in the new shape and must hard-error rather than
/// silently drop the authored defaults.
#[test]
fn rebuild_consolidation_rejects_non_default_variant_defaults() {
    let scratch = ScratchRoot::new("cdk-native-family-consolidate-defaults-reject");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    symlink_node_modules(&proj);
    let init = run_ctx(&["traits", "init", "family-fixture"], &proj, &home);
    assert!(init.status.success(), "init failed: {}", utf8(&init).1);
    let source = proj.join(".ctx/traits/packages/family-fixture/source/index.ts");
    fs::write(&source, family_fixture_source()).unwrap();
    let build = run_ctx(
        &[
            "traits",
            "build",
            ".ctx/traits/packages/family-fixture/source/index.ts",
        ],
        &proj,
        &home,
    );
    assert!(build.status.success(), "build failed: {}", utf8(&build).1);

    let root = proj.join(".ctx/traits/packages/family-fixture");
    let manifest = root.join("package.toml");
    let manifest_text = fs::read_to_string(&manifest).unwrap();
    let with_run_config = manifest_text.replace(
        "[family.variant.quick]\npath = \"generated/quick/index.toml\"",
        "[family.variant.quick]\npath = \"generated/quick/index.toml\"\nrun-config = \"run-config/quick.toml\"",
    );
    fs::create_dir_all(root.join("run-config")).unwrap();
    fs::write(
        root.join("run-config/quick.toml"),
        "schema-version = \"0.1\"\n\n[budget]\nmax-frames = 10\n\n[defaults.port]\nplan = \"sidecar\"\n",
    )
    .unwrap();
    fs::write(&manifest, &with_run_config).unwrap();
    let rebuild = run_ctx(
        &[
            "traits",
            "build",
            ".ctx/traits/packages/family-fixture/source/index.ts",
        ],
        &proj,
        &home,
    );
    assert!(
        !rebuild.status.success(),
        "rebuild must refuse to consolidate a non-default variant's [defaults.port]"
    );
    let (stdout, stderr) = utf8(&rebuild);
    assert!(
        (stdout.contains("quick") && stdout.contains("defaults.port"))
            || (stderr.contains("quick") && stderr.contains("defaults.port")),
        "refusal must name the offending variant and field\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        !root.join("runtime.toml").exists(),
        "refused consolidation must not partially write runtime.toml"
    );
}
