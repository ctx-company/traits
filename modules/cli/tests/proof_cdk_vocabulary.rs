//! P412: a leaf owned by exactly one `intent` facet is reachable both
//! qualified (`intent.avoid.ScopeCreep`) and bare (`intent.ScopeCreep`) —
//! this is authoring sugar only, so a real `ctx traits build` of two
//! otherwise-identical CDK sources (one qualified, one bare) must emit
//! byte-identical canonical TOML and report the same canonical digest.

use std::fs;

use support::{ScratchRoot, git_init, repo_root, require_success};

/// A CDK source that declares `intent.avoid` either through the qualified
/// `intent.avoid.ScopeCreep` or the bare `intent.ScopeCreep` alias — the only
/// difference between the two fixtures this test builds.
fn intent_fixture_source(trait_id: &str, intent_expression: &str) -> String {
    format!(
        "import {{ agent, intent, port, procedure, prompt, sequence, slot, trait }} from \"@ctx-traits/cdk\";\n\
\n\
const summary = slot.text(\"summary\");\n\
const output = port.output.text({{ id: \"summary\", value: summary }});\n\
const worker = agent(\"worker\", {{ description: \"Completes the starter task.\" }});\n\
\n\
export const draft = trait({{\n\
  id: \"{trait_id}\",\n\
  name: \"{trait_id}\",\n\
  description: \"Fixture proving bare vs qualified intent leaf resolution.\",\n\
  intent: {{ avoid: [{intent_expression}] }},\n\
  procedure: procedure({{\n\
    description: \"Describe what this trait should accomplish.\",\n\
    output,\n\
    sequence: sequence.prompt({{\n\
      id: \"run\",\n\
      agent: worker,\n\
      prompt: prompt.text`Describe the task for this trait.`,\n\
      output: summary,\n\
    }}),\n\
  }}),\n\
}});\n"
    )
}

/// Scaffold and build `trait_id` from `intent_expression`'s CDK source under
/// a fresh scratch project, returning the built canonical TOML bytes and the
/// canonical digest `ctx traits check --json` reports for it.
fn build_intent_fixture(label: &str, trait_id: &str, intent_expression: &str) -> (Vec<u8>, String) {
    let scratch = ScratchRoot::new(label);
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

    require_success(
        &format!("`ctx traits init {trait_id}`"),
        &["traits", "init", trait_id],
        &proj,
        &home,
    );

    let source_path = proj.join(format!(".ctx/traits/{trait_id}/source/index.ts"));
    fs::write(
        &source_path,
        intent_fixture_source(trait_id, intent_expression),
    )
    .unwrap_or_else(|error| panic!("cannot write {}: {error}", source_path.display()));

    require_success(
        &format!("`ctx traits build` for {trait_id}"),
        &[
            "traits",
            "build",
            &format!(".ctx/traits/{trait_id}/source/index.ts"),
        ],
        &proj,
        &home,
    );

    let canonical_toml =
        fs::read(proj.join(format!(".ctx/traits/{trait_id}/generated/index.toml")))
            .unwrap_or_else(|error| panic!("cannot read canonical output for {trait_id}: {error}"));

    let check_stdout = require_success(
        &format!("`ctx traits check {trait_id} --json`"),
        &["traits", "check", trait_id, "--json"],
        &proj,
        &home,
    );
    let check_json: serde_json::Value = serde_json::from_str(&check_stdout).unwrap_or_else(|error| {
        panic!("`ctx traits check {trait_id} --json` did not emit JSON: {error}\nstdout: {check_stdout}")
    });
    let canonical_digest = check_json["synth-provenance"][0]["canonical-digest"]
        .as_str()
        .unwrap_or_else(|| panic!("no synth-provenance[0].canonical-digest in {check_json}"))
        .to_string();

    (canonical_toml, canonical_digest)
}

/// The vocabulary slice absorbed into P458: an unambiguous facet leaf
/// (`avoid.ScopeCreep`) resolved bare at the `intent` root must build to
/// byte-identical canonical TOML, and the same canonical digest, as the
/// qualified form — this sugar never reaches normalization or digest
/// calculation.
#[test]
fn bare_and_qualified_intent_leaf_build_to_identical_canonical_output() {
    // Both fixtures use the same trait id: they build in fully isolated
    // scratch projects, so nothing collides, and using the same id means the
    // canonical TOML and digest are directly comparable without normalizing
    // away an expected difference.
    let (qualified_toml, qualified_digest) = build_intent_fixture(
        "cdk-vocabulary-qualified",
        "intent-vocab-fixture",
        "intent.avoid.ScopeCreep",
    );
    let (bare_toml, bare_digest) = build_intent_fixture(
        "cdk-vocabulary-bare",
        "intent-vocab-fixture",
        "intent.ScopeCreep",
    );

    assert_eq!(
        qualified_toml, bare_toml,
        "bare `intent.ScopeCreep` and qualified `intent.avoid.ScopeCreep` built different canonical TOML"
    );
    assert!(
        String::from_utf8_lossy(&qualified_toml).contains("scope-creep"),
        "expected the avoid facet's scope-creep slug in canonical output"
    );
    assert_eq!(
        qualified_digest, bare_digest,
        "bare and qualified intent leaves reported different canonical digests"
    );
}
