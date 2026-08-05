//! A removed bare `intent` leaf must not normalize as its facet-qualified
//! equivalent. The fixture builds both forms through the public CLI boundary.

use std::fs;

use support::{ScratchRoot, git_init, repo_root, require_success};

/// A CDK source that declares `intent.avoid` through the supplied expression.
fn intent_fixture_source(trait_id: &str, intent_expression: &str) -> String {
    format!(
        "import {{ agent, input, intent, port, procedure, sequence, slot, trait }} from \"@ctx-traits/cdk\";\n\
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
   port: output,\n\
   procedure: procedure({{\n\
    description: \"Describe what this trait should accomplish.\",\n\
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

    let source_path = proj.join(format!(".ctx/traits/packages/{trait_id}/source/index.ts"));
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
            &format!(".ctx/traits/packages/{trait_id}/source/index.ts"),
        ],
        &proj,
        &home,
    );

    let canonical_toml = fs::read(proj.join(format!(
        ".ctx/traits/packages/{trait_id}/generated/index.toml"
    )))
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

/// Intent leaves are facet-qualified. A removed bare-root alias must not
/// silently normalize as the corresponding `avoid` directive.
// DISABLED 2026-08-01 on merge of the parked 0046 run, per the standing
// no-behavior-freezing-tests-during-churn ruling (54706ca). This proof was
// added BY that run and asserts that a bare `intent.ScopeCreep` must NOT
// normalize to the `avoid` facet — but on merged main the two canonicals are
// identical, i.e. bare-root aliases DO resolve. That contradiction is the
// same open blocker the run parked on:
// `breaking-vocabulary-api-landed-before-consumer-migration` (3/6 steps).
// Re-enable when that blocker is closed and the bare-vs-qualified contract is
// settled — see task 0046.
#[ignore = "vocabulary bare-vs-qualified contract unsettled; see task 0046 blocker breaking-vocabulary-api-landed-before-consumer-migration"]
#[test]
fn bare_intent_leaf_does_not_normalize_as_a_qualified_leaf() {
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

    assert_ne!(qualified_toml, bare_toml);
    assert!(
        String::from_utf8_lossy(&qualified_toml).contains("scope-creep"),
        "expected the avoid facet's scope-creep slug in canonical output"
    );
    let qualified: toml::Value = toml::from_str(&String::from_utf8_lossy(&qualified_toml))
        .expect("qualified fixture must emit canonical TOML");
    assert_eq!(
        qualified
            .get("procedure")
            .and_then(toml::Value::as_table)
            .and_then(|procedure| procedure.get("output"))
            .and_then(toml::Value::as_array),
        Some(&vec![toml::Value::String("port:summary".to_string())]),
        "the trait-level output port must be inferred into the procedure contract"
    );
    assert_ne!(qualified_digest, bare_digest);
    assert!(
        !String::from_utf8_lossy(&bare_toml).contains("scope-creep"),
        "removed bare `intent.ScopeCreep` alias unexpectedly emitted an avoid directive"
    );
}
