//! 0230: `ctx traits build <path>/source/index.ts` resolves its package root
//! to the package, not to `source/`.
//!
//! The failure this pins was silent in the ordinary case and fatal in one
//! specific case. A path build of a recognized package wrote its canonical
//! beside the source (`source/index.toml`) instead of under `generated/`,
//! and that mis-placed canonical then became the root every relative
//! dependency resolved from — so a package declaring `path = "../sibling"`
//! looked for it at `<package>/source/../sibling`, which folds to
//! `<package>/sibling`, and could not be built by path at all.
//!
//! Building by id was always correct, which is why this survived: the
//! repository's own gate for the built-in packages worked around it by
//! copying every package into a flat scratch root and building by id there.
//!
//! A dependency-free package proves nothing here — it builds by path either
//! way, because nothing ever asks what the root was. So this fixture is two
//! sibling packages where one declares the other by relative path.

use std::fs;
use std::path::Path;

use support::{ScratchRoot, git_init, run_ctx, symlink_node_modules, utf8};

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().expect("path has a parent")).expect("create parent");
    fs::write(path, contents).expect("write fixture file");
}

/// A package root under `.ctx/traits/authored/<id>` — the shape
/// `package_root_for_manifest` recognizes as canonical.
fn write_package(authored: &Path, id: &str, name: &str, source: &str, dependencies: &str) {
    let root = authored.join(id);
    write(
        &root.join("trait.toml"),
        &format!(
            "[package]\n\
             id = \"{id}\"\n\
             version = \"0.1.0\"\n\
             name = \"{name}\"\n\
             description = \"Fixture package for the 0230 path-build proof.\"\n\
             status = \"draft\"\n\
             {dependencies}"
        ),
    );
    write(&root.join("source/index.ts"), source);
}

const LIBRARY_SOURCE: &str = r#"import * as cdk from "@ctx-traits/cdk";

const note = cdk.port.output.text({
  id: "note",
  description: "Nothing runs here; this package exists to be depended on.",
});

export default function () {
  cdk.defineTrait("Fixture Library", {
    version: "0.1.0",
    description: "A dependency-only fixture package.",
  });
  return { note };
}
"#;

const CONSUMER_SOURCE: &str = r#"import * as cdk from "@ctx-traits/cdk";

const worker = cdk.agent.worker("worker", { description: "Does the fixture work." });

const goal = cdk.port.input.text({ id: "goal", description: "What to do." });
const result = cdk.slot.text({ id: "result", description: "What was done." });
const report = cdk.port.output.text({
  id: "report",
  description: "What was done.",
  value: result,
});

export default function () {
  cdk.defineTrait("Fixture Consumer", {
    version: "0.1.0",
    description: "A fixture package that declares a sibling by relative path.",
  });
  worker.prompt("Do the work", { input: cdk.input.prompt`Do ${goal}.`, output: result });
  return { report };
}
"#;

#[test]
fn a_path_built_package_resolves_its_sibling_path_dependency() {
    let scratch = ScratchRoot::new("source-path-build-root");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).expect("create project dir");
    git_init(&proj);
    symlink_node_modules(&proj);

    let authored = proj.join(".ctx/traits/authored");
    write_package(
        &authored,
        "fixture-library",
        "Fixture Library",
        LIBRARY_SOURCE,
        "",
    );
    write_package(
        &authored,
        "fixture-consumer",
        "Fixture Consumer",
        CONSUMER_SOURCE,
        "\n[dependencies]\nfixture-library = { version = \"0.1.0\", path = \"../fixture-library\" }\n",
    );

    // The dependency has to be built before anything can consume it: a
    // dependency package must ship its own canonical and lock.
    let built_library = run_ctx(
        &[
            "traits",
            "build",
            ".ctx/traits/authored/fixture-library/source/index.ts",
        ],
        &proj,
        &home,
    );
    let (library_out, library_err) = utf8(&built_library);
    assert!(
        built_library.status.success(),
        "building the dependency by source path failed: {library_out}{library_err}"
    );

    // The canonical lands under `generated/`, never beside the source. This
    // is the defect's visible half: `source/index.toml` and a stray
    // `source/trait.lock` are what a mis-rooted build leaves behind.
    assert!(
        authored
            .join("fixture-library/generated/index.toml")
            .is_file(),
        "a path build must write its canonical under generated/"
    );
    assert!(
        !authored.join("fixture-library/source/index.toml").exists(),
        "a path build must not write its canonical beside the source"
    );
    assert!(
        !authored.join("fixture-library/source/trait.lock").exists(),
        "a path build must not write its lock beside the source"
    );

    // The defect's fatal half: `../fixture-library` is resolved from the
    // package root, so it finds its sibling. Resolved from `source/` it
    // would fold to `<package>/fixture-library` and fail with "dependency
    // package does not contain trait.toml".
    let built_consumer = run_ctx(
        &[
            "traits",
            "build",
            ".ctx/traits/authored/fixture-consumer/source/index.ts",
        ],
        &proj,
        &home,
    );
    let (consumer_out, consumer_err) = utf8(&built_consumer);
    assert!(
        built_consumer.status.success(),
        "building a package with a sibling path dependency by source path failed: \
         {consumer_out}{consumer_err}"
    );
    assert!(
        !format!("{consumer_out}{consumer_err}").contains("does not contain trait.toml"),
        "the sibling dependency resolved from the wrong root: {consumer_out}{consumer_err}"
    );
    assert!(
        authored
            .join("fixture-consumer/generated/index.toml")
            .is_file(),
        "the consumer's canonical must land under generated/ too"
    );
}
