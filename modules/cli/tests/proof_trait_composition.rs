//! Task 0170: a variant canonical produced through the native-family build
//! path gets the same derived `[[dependency]]` stamp the single-trait path
//! gets when the build crosses a bare-specifier import into a sibling
//! `node_modules/<pkg>` trait package. Regression guard for
//! `family-path-not-stamped`: `synthesize_cdk_family` used to return before
//! `stamp_derived_dependencies` ran, so a family variant that inlined
//! dependency-trait content shipped with no stamp at all.

use std::fs;

use support::{ScratchRoot, git_init, repo_root, run_ctx, utf8};

/// A private `node_modules` for this fixture project: every entry of the
/// repository's own installed `node_modules` symlinked in individually
/// (never the whole directory — [`support::symlink_node_modules`] does that
/// and shares one real directory across every caller, which would make this
/// fixture's own `@fixture/dep` package land inside the *repository's* real
/// `node_modules`), plus this test's own `@fixture/dep` package written as a
/// real directory alongside them.
fn private_node_modules(proj: &std::path::Path) -> std::path::PathBuf {
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
/// itself — the dependency is a real trait package, built through the
/// ordinary `ctx traits init`/`build` lifecycle so its own `trait.lock` self
/// entry is written by production code (`ctx_traits_io::dependency::sync`'s
/// "self" load via `record_lock_evidence`), not hand-typed. `shared.mjs` is
/// the plain importable helper `@fixture/dep`'s *consumers* pull in — it is
/// not part of this trait's own canonical/port and is added to the package
/// root separately after this build.
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

/// Builds `@fixture/dep` as its own real trait project in a scratch
/// directory (via `ctx traits init` + `build`, so its `trait.lock` self
/// entry is production-written), then copies the resulting package —
/// `trait.toml`, `trait.lock`, `generated/`, `source/` — into `dep_root`
/// alongside a `package.json` and the plain `shared.mjs` helper consumers
/// import. The consumer-side dependency health gate (task 0170
/// `pinning-chain-missing`, done-when clause 4) verifies this shipped
/// `trait.lock` against the shipped `generated/` canonical, so a real build
/// output is required here rather than a hand-typed lock.
fn write_locked_dependency_fixture(dep_root: &std::path::Path, label: &str) {
    let dep_scratch = ScratchRoot::new(label);
    let dep_home = dep_scratch.home();
    let dep_proj = dep_home.join("dep-repo");
    fs::create_dir_all(&dep_proj).unwrap();
    git_init(&dep_proj);
    private_node_modules(&dep_proj);

    let init = run_ctx(&["traits", "init", "fixture-dep"], &dep_proj, &dep_home);
    assert!(
        init.status.success(),
        "`ctx traits init fixture-dep` for the dependency fixture failed: {}",
        utf8(&init).1
    );
    let dep_source_path = dep_proj.join(".ctx/traits/authored/fixture-dep/source/index.ts");
    fs::write(&dep_source_path, dep_trait_source()).unwrap();
    let build = run_ctx(
        &[
            "traits",
            "build",
            ".ctx/traits/authored/fixture-dep/source/index.ts",
        ],
        &dep_proj,
        &dep_home,
    );
    let (stdout, stderr) = utf8(&build);
    assert!(
        build.status.success(),
        "building the dependency fixture's own trait package failed\nstdout: {stdout}\nstderr: {stderr}"
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

    // `dep_scratch`'s tempdir is dropped (and cleaned up) here — everything
    // this fixture needs was already copied into `dep_root`, which lives
    // under the caller's own scratch root.
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) {
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

fn family_fixture_source() -> &'static str {
    "import { agent, input, port, procedure, sequence, slot, trait, variant } from \"@ctx-traits/cdk\";\n\
import { summaryText } from \"@fixture/dep/shared.mjs\";\n\
\n\
const summary = slot.text(\"summary\");\n\
const output = port.output.text({ id: \"summary\", value: summary });\n\
const worker = agent(\"worker\", { description: \"Completes the starter task.\" });\n\
\n\
const variantFixture = (name) => variant({\n\
  name,\n\
  summary: `The ${name} variant.`,\n\
  procedure: procedure({\n\
    description: summaryText,\n\
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
export const draft = trait(\"family-dep-fixture\", {\n\
  variants: {\n\
    default: variantFixture(\"default\").default(),\n\
    quick: variantFixture(\"quick\"),\n\
  },\n\
});\n"
}

/// `ctx traits build` on a family source whose variants import a bare
/// specifier resolving into a sibling `node_modules/<pkg>` trait package
/// (a package root shipping its own `trait.toml`) stamps a derived
/// `[[dependency]]` row — `{ id, version, source, digest }` — into every
/// variant's canonical, not just the default one and not just on the
/// single-trait build path.
#[test]
fn family_variant_canonicals_carry_the_derived_dependency_stamp() {
    let scratch = ScratchRoot::new("p0170-family-dependency-stamp");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    let node_modules = private_node_modules(&proj);

    let dep_root = node_modules.join("@fixture/dep");
    write_locked_dependency_fixture(&dep_root, "p0170-dep-fixture");

    let trait_id = "family-dep-fixture";
    let init = run_ctx(&["traits", "init", trait_id], &proj, &home);
    assert!(
        init.status.success(),
        "`ctx traits init {trait_id}` failed: {}",
        utf8(&init).1
    );

    let source_path = proj.join(format!(".ctx/traits/authored/{trait_id}/source/index.ts"));
    fs::write(&source_path, family_fixture_source()).unwrap();

    let build = run_ctx(
        &[
            "traits",
            "build",
            &format!(".ctx/traits/authored/{trait_id}/source/index.ts"),
        ],
        &proj,
        &home,
    );
    let (stdout, stderr) = utf8(&build);
    assert!(
        build.status.success(),
        "build failed\nstdout: {stdout}\nstderr: {stderr}"
    );

    for variant in ["default", "quick"] {
        let canonical_path = proj.join(format!(
            ".ctx/traits/authored/{trait_id}/generated/{variant}/index.toml"
        ));
        let canonical = fs::read_to_string(&canonical_path)
            .unwrap_or_else(|error| panic!("reading {canonical_path:?}: {error}"));
        assert!(
            canonical.contains("[[dependency]]"),
            "variant {variant:?} canonical is missing a derived [[dependency]] stamp:\n{canonical}"
        );
        assert!(
            canonical.contains("id = \"fixture-dep\""),
            "variant {variant:?} canonical's stamp does not name fixture-dep:\n{canonical}"
        );
        assert!(
            canonical.contains("digest = "),
            "variant {variant:?} canonical's stamp is missing its digest:\n{canonical}"
        );
        // `DEP_SHARED_MJS`'s `summaryText` is imported and used as the
        // procedure's `description`, so it is not enough for the canonical
        // to merely carry a `[[dependency]]` stamp — the dependency's own
        // content must actually be inlined into the variant's declarations.
        assert!(
            canonical.contains("Describe what this trait should accomplish."),
            "variant {variant:?} canonical does not inline @fixture/dep's imported summaryText:\n{canonical}"
        );
    }
}

/// Single-trait fixture source importing the same `@fixture/dep` bare
/// specifier as the family fixture above, used for the pinning-chain proofs
/// below (task 0170 `pinning-chain-missing`).
fn single_fixture_source() -> &'static str {
    "import { agent, input, port, procedure, sequence, slot, trait } from \"@ctx-traits/cdk\";\n\
import { summaryText } from \"@fixture/dep/shared.mjs\";\n\
\n\
const summary = slot.text(\"summary\");\n\
const output = port.output.text({ id: \"summary\", value: summary });\n\
const worker = agent(\"worker\", { description: \"Completes the starter task.\" });\n\
\n\
export const draft = trait({\n\
  id: \"single-dep-fixture\",\n\
  name: \"single-dep-fixture\",\n\
  description: summaryText,\n\
  port: output,\n\
  procedure: procedure({\n\
    description: summaryText,\n\
    sequence: sequence.prompt({\n\
      id: \"run\",\n\
      agent: worker,\n\
      prompt: input.prompt`Describe the task for this trait.`,\n\
      output: summary,\n\
    }),\n\
  }),\n\
});\n"
}

/// Scaffolds a scratch project with a private `node_modules/@fixture/dep`
/// trait package (with its own `trait.lock`) and the single-trait fixture
/// source importing it. Returns `(proj, home, dep_root)`.
fn scaffold_single_dep_project(
    label: &str,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let scratch = ScratchRoot::new(label);
    // Leak the scratch root so its tempdir outlives this function — the
    // proof functions below need the paths to stay valid for the whole
    // test body.
    let scratch = Box::leak(Box::new(scratch));
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    let node_modules = private_node_modules(&proj);

    let dep_root = node_modules.join("@fixture/dep");
    write_locked_dependency_fixture(&dep_root, "p0170-dep-fixture");

    let trait_id = "single-dep-fixture";
    let init = run_ctx(&["traits", "init", trait_id], &proj, &home);
    assert!(
        init.status.success(),
        "`ctx traits init {trait_id}` failed: {}",
        utf8(&init).1
    );
    let source_path = proj.join(format!(".ctx/traits/authored/{trait_id}/source/index.ts"));
    fs::write(&source_path, single_fixture_source()).unwrap();

    (proj, home, dep_root)
}

fn build_single_dep_fixture(
    proj: &std::path::Path,
    home: &std::path::Path,
) -> std::process::Output {
    run_ctx(
        &[
            "traits",
            "build",
            ".ctx/traits/authored/single-dep-fixture/source/index.ts",
        ],
        proj,
        home,
    )
}

/// First build of a trait that imports a `node_modules` trait dependency
/// writes a `trait.lock` entry pinning that dependency's id/version/digest
/// (task 0170 `pinning-chain-missing`, done-when clause 1).
#[test]
fn first_build_writes_a_dependency_lock_pin() {
    let (proj, home, _dep_root) = scaffold_single_dep_project("p0170-pin-first-build");
    let build = build_single_dep_fixture(&proj, &home);
    let (stdout, stderr) = utf8(&build);
    assert!(
        build.status.success(),
        "build failed\nstdout: {stdout}\nstderr: {stderr}"
    );
    let lock_path = proj.join(".ctx/traits/authored/single-dep-fixture/trait.lock");
    let lock = fs::read_to_string(&lock_path)
        .unwrap_or_else(|error| panic!("reading {lock_path:?}: {error}"));
    assert!(
        lock.contains("id = \"fixture-dep\""),
        "trait.lock is missing a pin for fixture-dep:\n{lock}"
    );
}

/// Tampering the dependency's content at the same version fails the
/// consumer's rebuild with an error naming the dependency, and `--relock`
/// rewrites the pin to accept it (task 0170 `pinning-chain-missing`,
/// done-when clauses 2-3).
#[test]
fn tampered_dependency_content_fails_rebuild_until_relocked() {
    let (proj, home, dep_root) = scaffold_single_dep_project("p0170-pin-tamper");
    let first = build_single_dep_fixture(&proj, &home);
    assert!(
        first.status.success(),
        "initial build failed: {}",
        utf8(&first).1
    );

    // Tamper `shared.mjs` — the plain importable helper this consumer
    // pulls in — at the same declared version. `shared.mjs` is not part of
    // `@fixture/dep`'s own canonical/port, so its own self-consistency
    // health gate (`dependency_shipped_lock_digest_mismatch_fails_the_consumer_build`
    // covers that orthogonal case) is untouched by this; only the
    // consumer's own pinned *consumed-content* digest goes stale.
    fs::write(
        dep_root.join("shared.mjs"),
        "export const summaryText = \"Tampered dependency content.\";\n",
    )
    .unwrap();

    let tampered = build_single_dep_fixture(&proj, &home);
    let (stdout, stderr) = utf8(&tampered);
    assert!(
        !tampered.status.success(),
        "rebuild over tampered dependency content unexpectedly succeeded\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("fixture-dep") && stderr.contains("--relock"),
        "refusal must name fixture-dep and point at --relock:\nstdout: {stdout}\nstderr: {stderr}"
    );

    let relocked = run_ctx(
        &[
            "traits",
            "build",
            ".ctx/traits/authored/single-dep-fixture/source/index.ts",
            "--relock",
        ],
        &proj,
        &home,
    );
    let (stdout, stderr) = utf8(&relocked);
    assert!(
        relocked.status.success(),
        "--relock rebuild failed\nstdout: {stdout}\nstderr: {stderr}"
    );
}

/// A dependency package with no `trait.lock` of its own fails the consumer
/// build, naming the dependency (task 0170 `pinning-chain-missing`,
/// done-when clause 4).
#[test]
fn missing_dependency_lock_fails_the_consumer_build() {
    let (proj, home, dep_root) = scaffold_single_dep_project("p0170-pin-missing-lock");
    fs::remove_file(dep_root.join("trait.lock")).unwrap();

    let build = build_single_dep_fixture(&proj, &home);
    let (stdout, stderr) = utf8(&build);
    assert!(
        !build.status.success(),
        "build over a dependency with no trait.lock unexpectedly succeeded\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("fixture-dep"),
        "refusal must name fixture-dep:\nstdout: {stdout}\nstderr: {stderr}"
    );
}

/// A dependency whose own shipped `trait.lock` no longer matches its own
/// shipped content — tampered in transit, independent of whether the
/// consumer already holds a pin for it — fails the consumer build, naming
/// the dependency (task 0170 `pinning-chain-missing`, done-when clause 4,
/// second half: the health gate must compare digests, not just check the
/// lock file exists).
#[test]
fn dependency_shipped_lock_digest_mismatch_fails_the_consumer_build() {
    let (proj, home, dep_root) = scaffold_single_dep_project("p0170-pin-lock-digest-mismatch");

    // Tamper the dependency's own shipped canonical *after* it was locked,
    // without re-locking — its `trait.lock` self entry now pins a source
    // digest its own shipped canonical no longer matches, exactly as if the
    // package's build output were tampered in transit after publish.
    // `shared.mjs` (the plain helper this consumer imports) is untouched, so
    // this is purely the dependency's own self-consistency failing, not the
    // consumer's pinned consumed-content digest.
    let dep_canonical_path = dep_root.join("generated/index.toml");
    let dep_canonical = fs::read_to_string(&dep_canonical_path).unwrap();
    fs::write(
        &dep_canonical_path,
        dep_canonical.replace(
            "A fixture dependency trait package.",
            "Tampered dependency canonical, never relocked.",
        ),
    )
    .unwrap();

    let build = build_single_dep_fixture(&proj, &home);
    let (stdout, stderr) = utf8(&build);
    assert!(
        !build.status.success(),
        "first build over a dependency whose shipped lock does not match its shipped content unexpectedly succeeded\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("fixture-dep"),
        "refusal must name fixture-dep:\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        !stderr.contains("--relock"),
        "a dependency-side lock/content mismatch is not fixed by the consumer's own --relock flag, so the message must not suggest it:\nstdout: {stdout}\nstderr: {stderr}"
    );
}

/// Task 0170 deliverable 4 ("rebuild-from-shipped-source byte-compare",
/// verification never delegates to npm): rebuilding a package from its own
/// shipped `source/` and byte-comparing against its shipped `generated/`
/// canonical is already the `cdk-drift` check section — `cdk_drift_check`
/// (`modules/cli/src/app/report_check.rs`) re-synthesizes straight from the
/// package's `source/` tree on disk and byte-compares `response.output_text`
/// against what's committed at `generated/index.toml`, and `ctx traits
/// publish` (`modules/cli/src/app/distribution.rs::handle_publish`) already
/// runs `ctx traits check --locked` and refuses to publish when it fails.
/// This proof is the missing evidence for that existing wiring: a shipped
/// canonical tampered independently of its source fails `ctx traits check
/// --locked`, naming the drift, and a clean rebuild restores a passing check.
#[test]
fn tampered_shipped_canonical_fails_locked_check_until_rebuilt() {
    let (proj, home, _dep_root) = scaffold_single_dep_project("p0170-rebuild-byte-compare");
    let build = build_single_dep_fixture(&proj, &home);
    assert!(
        build.status.success(),
        "initial build failed: {}",
        utf8(&build).1
    );

    let canonical_path = proj.join(".ctx/traits/authored/single-dep-fixture/generated/index.toml");
    let canonical = fs::read_to_string(&canonical_path).unwrap();

    // Tamper the shipped canonical without touching source/ at all.
    fs::write(
        &canonical_path,
        canonical.replace(
            "Describe what this trait should accomplish.",
            "Tampered canonical content.",
        ),
    )
    .unwrap();

    let checked = run_ctx(
        &[
            "traits",
            "check",
            "--file",
            ".ctx/traits/authored/single-dep-fixture/generated/index.toml",
            "--locked",
            "--json",
        ],
        &proj,
        &home,
    );
    let (stdout, stderr) = utf8(&checked);
    assert!(
        !checked.status.success(),
        "locked check over a tampered shipped canonical unexpectedly passed\nstdout: {stdout}\nstderr: {stderr}"
    );
    let report: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("check --json produced invalid JSON: {error}\n{stdout}"));
    let cdk_drift_ok = report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|section| section["name"] == "cdk-drift")
        .unwrap_or_else(|| panic!("no cdk-drift section in check report:\n{stdout}"))["ok"]
        .as_bool()
        .unwrap();
    assert!(
        !cdk_drift_ok,
        "cdk-drift section must fail on a shipped canonical tampered independently of source:\n{stdout}"
    );

    let rebuilt = build_single_dep_fixture(&proj, &home);
    assert!(
        rebuilt.status.success(),
        "rebuild over a tampered canonical failed: {}",
        utf8(&rebuilt).1
    );
    let recheck = run_ctx(
        &[
            "traits",
            "check",
            "--file",
            ".ctx/traits/authored/single-dep-fixture/generated/index.toml",
            "--locked",
        ],
        &proj,
        &home,
    );
    let (stdout, stderr) = utf8(&recheck);
    assert!(
        recheck.status.success(),
        "locked check after rebuild unexpectedly failed\nstdout: {stdout}\nstderr: {stderr}"
    );
}

/// A `node_modules/@fixture/dep` entry that is a symlink to a real package
/// directory *outside* `node_modules` (the classic `npm link`/local-path
/// install shape) resolves via the same realpath-based package-root
/// detection the JS resolve tracker uses for ordinary installs
/// (`RESOLVE_TRACKER_PRELUDE`'s `fs.realpathSync(dir)`), and is stamped with
/// a `local` `[[dependency]].source` instead of `npm` — the same composition
/// (declarations inlined, digest computed over the same consumed files)
/// otherwise byte-identical modulo that one field (task 0170
/// `composition-proofs-missing`, local-path-vs-npm equivalence, and a
/// symlinked-package-root classification case in one proof: the symlink
/// *is* what makes this a local-path install here).
#[test]
fn local_path_symlinked_dependency_yields_an_identical_canonical_modulo_source() {
    let scratch = ScratchRoot::new("p0170-local-path-dep");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    let node_modules = private_node_modules(&proj);

    // The real package directory lives *outside* `node_modules` entirely —
    // a sibling of the project, exactly like a local-path/`npm link`
    // dependency a consumer never fetched from a registry.
    let real_dep_root = home.join("local-packages").join("fixture-dep");
    write_locked_dependency_fixture(&real_dep_root, "p0170-local-path-dep-build");

    let linked_dep_root = node_modules.join("@fixture/dep");
    fs::create_dir_all(linked_dep_root.parent().unwrap()).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real_dep_root, &linked_dep_root).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&real_dep_root, &linked_dep_root).unwrap();

    let trait_id = "single-dep-fixture";
    let init = run_ctx(&["traits", "init", trait_id], &proj, &home);
    assert!(
        init.status.success(),
        "`ctx traits init {trait_id}` failed: {}",
        utf8(&init).1
    );
    let source_path = proj.join(format!(".ctx/traits/authored/{trait_id}/source/index.ts"));
    fs::write(&source_path, single_fixture_source()).unwrap();

    let build = build_single_dep_fixture(&proj, &home);
    let (stdout, stderr) = utf8(&build);
    assert!(
        build.status.success(),
        "build over a symlinked local-path dependency failed\nstdout: {stdout}\nstderr: {stderr}"
    );

    let canonical_path = proj.join(format!(
        ".ctx/traits/authored/{trait_id}/generated/index.toml"
    ));
    let canonical = fs::read_to_string(&canonical_path).unwrap();
    assert!(
        canonical.contains("[[dependency]]") && canonical.contains("id = \"fixture-dep\""),
        "local-path build's canonical is missing the derived [[dependency]] stamp:\n{canonical}"
    );
    assert!(
        canonical.contains("source = \"local\"")
            || canonical.contains("kind = \"local\"")
            || (canonical.contains("[dependency.source]") && canonical.contains("local")),
        "a symlinked local-path dependency (realpath outside node_modules) must stamp a local source, not npm:\n{canonical}"
    );
    assert!(
        !canonical.contains("\"npm\""),
        "a symlinked local-path dependency must not be misclassified as an npm source:\n{canonical}"
    );

    // Build the SAME consumer fixture again, this time with `@fixture/dep`
    // installed as an ordinary npm-shape directory under `node_modules`
    // (`scaffold_single_dep_project`'s normal shape), and assert the two
    // generated canonicals are byte-identical once the `[[dependency]]`
    // `source` field is normalized away — proving no transport-dependent
    // bytes (npm-normalized `package.json`, absolute symlink-vs-real paths)
    // leak into the stamped consumed-content digest (task 0170
    // `composition-proofs-missing`, local-vs-npm equivalence).
    let (npm_proj, npm_home, _npm_dep_root) =
        scaffold_single_dep_project("p0170-local-path-dep-npm-shape");
    let npm_build = build_single_dep_fixture(&npm_proj, &npm_home);
    let (npm_stdout, npm_stderr) = utf8(&npm_build);
    assert!(
        npm_build.status.success(),
        "npm-shape build over the same consumer fixture failed\nstdout: {npm_stdout}\nstderr: {npm_stderr}"
    );
    let npm_canonical_path = npm_proj.join(format!(
        ".ctx/traits/authored/{trait_id}/generated/index.toml"
    ));
    let npm_canonical = fs::read_to_string(&npm_canonical_path).unwrap();

    // The stamped `[dependency.source]` table has different keys entirely
    // across transports (`registry`/`package` for npm vs `path` for local),
    // not just a differing value of a shared field — drop the whole table
    // and everything through the next blank line before comparing.
    fn normalize_dependency_source(canonical: &str) -> String {
        let mut normalized = String::new();
        let mut skipping = false;
        for line in canonical.lines() {
            if line == "[dependency.source]" {
                skipping = true;
                normalized.push_str("[dependency.source]\n<normalized>\n");
                continue;
            }
            if skipping {
                if line.is_empty() {
                    skipping = false;
                } else {
                    continue;
                }
            }
            normalized.push_str(line);
            normalized.push('\n');
        }
        normalized
    }

    let local_normalized = normalize_dependency_source(&canonical);
    let npm_normalized = normalize_dependency_source(&npm_canonical);
    assert!(
        local_normalized.contains("[[dependency]]") && npm_normalized.contains("[[dependency]]"),
        "both canonicals must carry the derived [[dependency]] stamp before comparing:\nlocal:\n{canonical}\nnpm:\n{npm_canonical}"
    );
    assert_eq!(
        local_normalized, npm_normalized,
        "local-path and npm-shape builds of the same consumer must yield identical canonicals modulo [[dependency]].source (a transport-dependent byte would break this):\nlocal (normalized):\n{local_normalized}\nnpm (normalized):\n{npm_normalized}"
    );
}
