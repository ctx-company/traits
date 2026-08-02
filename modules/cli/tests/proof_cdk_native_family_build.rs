//! P530 Stage B: `ctx traits build` publishes a native trait family
//! (`trait(id, { variants })`, `packages/cdk/src/variant.ts`'s
//! `resolveTraitFamily`) by writing each leaf's canonical output under
//! `generated/<selector>/` and refreshing the package's `[family]` manifest
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

/// `ctx traits build` on a `trait(id, { variants })` family source publishes
/// every leaf's canonical output under `generated/<selector>/index.toml`
/// (+ `index.map`) and writes a `[family]` table into the package's root
/// `trait.toml` naming the default leaf and every leaf's generated path and
/// legacy alias.
#[test]
fn build_publishes_native_family_leaves_and_manifest_table() {
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
    for selector in ["default", "quick"] {
        let leaf_toml = package_root.join(format!("generated/{selector}/index.toml"));
        let leaf_map = package_root.join(format!("generated/{selector}/index.map"));
        assert!(
            leaf_toml.is_file(),
            "expected {} to exist after build",
            leaf_toml.display()
        );
        assert!(
            leaf_map.is_file(),
            "expected {} to exist after build",
            leaf_map.display()
        );
        let leaf_text = fs::read_to_string(&leaf_toml).unwrap();
        assert!(
            leaf_text.contains("id = \"family-fixture\""),
            "leaf {selector} canonical missing family id: {leaf_text}"
        );
        assert!(
            leaf_text.contains(&format!("variant = \"{selector}\"")),
            "leaf {selector} canonical missing its own variant selector: {leaf_text}"
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
        "family.default should name the default leaf: {manifest_text}"
    );
    let quick_leaf = family
        .get("leaf")
        .and_then(|leaf| leaf.get("quick"))
        .unwrap_or_else(|| panic!("trait.toml missing family.leaf.quick: {manifest_text}"));
    assert_eq!(
        quick_leaf.get("path").and_then(toml::Value::as_str),
        Some("generated/quick/index.toml"),
        "family.leaf.quick.path should point at the quick leaf's generated output: {manifest_text}"
    );
    let quick_aliases = quick_leaf
        .get("aliases")
        .and_then(toml::Value::as_array)
        .unwrap_or_else(|| panic!("family.leaf.quick.aliases missing: {manifest_text}"));
    assert!(
        quick_aliases
            .iter()
            .any(|alias| alias.as_str() == Some("family-fixture-quick")),
        "family.leaf.quick.aliases should carry the legacy hyphenated id: {manifest_text}"
    );
    let default_leaf = family
        .get("leaf")
        .and_then(|leaf| leaf.get("default"))
        .unwrap_or_else(|| panic!("trait.toml missing family.leaf.default: {manifest_text}"));
    assert!(
        default_leaf
            .get("aliases")
            .and_then(toml::Value::as_array)
            .is_some_and(|aliases| aliases
                .iter()
                .any(|alias| alias.as_str() == Some("family-fixture-default"))),
        "the default leaf must retain its legacy hyphenated selector: {manifest_text}"
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
        family_fixture_source().replace("The ${name} leaf.", "Rebuilt ${name} leaf."),
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
            && stdout.contains("leaf: default")
            && stdout.contains("leaf: quick"),
        "named variant build did not report the complete family: {stdout}"
    );
    for selector in ["default", "quick"] {
        let leaf = fs::read_to_string(proj.join(format!(
            ".ctx/traits/packages/{trait_id}/generated/{selector}/index.toml"
        )))
        .unwrap();
        assert!(
            leaf.contains(&format!("summary = \"Rebuilt {selector} leaf.\"")),
            "named variant build did not refresh {selector}: {leaf}"
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
        "build must not write any leaf output on refusal"
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
        "build must not write any leaf output on an identity-mismatch refusal"
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

/// Checking the default leaf verifies the complete synthesized family, so a
/// clean default cannot hide drift in another canonical, the topology table,
/// a missing leaf, or another leaf's source map.
#[test]
fn default_leaf_check_covers_complete_family_drift() {
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
        "[family.leaf.quick]\npath = \"generated/quick/index.toml\"",
        "[family.leaf.quick]\npath = \"generated/quick/index.toml\"\nrun-config = \"run-config/quick.toml\"",
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
    let with_run_config = fs::read_to_string(&manifest).unwrap();
    assert!(
        with_run_config.contains("run-config = \"run-config/quick.toml\""),
        "rebuild must preserve authored run-config: {with_run_config}"
    );
    assert_family_check(&proj, &home, true, "authored run-config");

    let missing_alias = with_run_config.replace("aliases = [\"family-fixture-quick\"]\n", "");
    assert_ne!(
        missing_alias, with_run_config,
        "fixture must contain quick alias"
    );
    fs::write(&manifest, missing_alias).unwrap();
    assert_family_check(&proj, &home, false, "missing generated alias");
    fs::write(&manifest, &with_run_config).unwrap();

    let quick = root.join("generated/quick/index.toml");
    let quick_text = fs::read_to_string(&quick).unwrap();
    fs::write(&quick, format!("{quick_text}\n# drift\n")).unwrap();
    assert_family_check(&proj, &home, false, "non-default canonical");
    fs::write(&quick, &quick_text).unwrap();

    fs::remove_file(&quick).unwrap();
    assert_family_check(&proj, &home, false, "missing leaf");
    fs::write(&quick, &quick_text).unwrap();

    let map = root.join("generated/quick/index.map");
    let map_text = fs::read_to_string(&map).unwrap();
    fs::write(&map, format!("{map_text} ")).unwrap();
    assert_family_check(&proj, &home, false, "non-default source map");
    fs::write(&map, &map_text).unwrap();

    let expanded = family_fixture_source().replace(
        "quick: leaf(\"quick\"),",
        "quick: leaf(\"quick\"),\n    smart: leaf(\"smart\"),",
    );
    fs::write(&source, expanded).unwrap();
    assert_family_check(
        &proj,
        &home,
        false,
        "new synthesized leaf absent from manifest",
    );
}
