//! Task 0066.2: `ctx traits generate --candidate` reports its single round
//! through the same `RoundEvidence` shape the guarded loop's default path
//! uses — zero model calls, zero provider config, per the parent contract's
//! cheap-test requirement.

use std::fs;

use support::{ScratchRoot, git_init, run_ctx, symlink_node_modules, utf8};

#[test]
fn candidate_path_reports_one_round_round_evidence() {
    let scratch = ScratchRoot::new("generate-candidate-round-evidence");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    symlink_node_modules(&proj);

    let candidate_path = proj.join("broken-candidate.ts");
    fs::write(&candidate_path, "this is not valid TypeScript {{{")
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", candidate_path.display()));

    let output = run_ctx(
        &[
            "traits",
            "generate",
            "Candidate Round Evidence Fixture",
            "Broken TS candidate — proves the --candidate evidence shape.",
            "--candidate",
            candidate_path.to_str().expect("candidate path is UTF-8"),
            "--json",
        ],
        &proj,
        &home,
    );
    let (stdout, stderr) = utf8(&output);

    assert!(
        !output.status.success(),
        "a candidate that fails at the build rung must exit non-zero\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let evidence: serde_json::Value =
        serde_json::from_str(&stdout).expect("--json output must be one RoundEvidence document");
    assert_eq!(evidence["converged"], serde_json::json!(false));
    assert_eq!(evidence["rounds-spent"], serde_json::json!(1));
    assert_eq!(evidence["failing-rung"], serde_json::json!("build"));
    assert!(
        evidence
            .get("rounds-bound")
            .is_none_or(serde_json::Value::is_null),
        "--candidate evaluates outside the declared bound: rounds-bound must stay absent: {evidence}"
    );
    let rounds = evidence["rounds"]
        .as_array()
        .expect("evidence must carry a rounds array");
    assert_eq!(
        rounds.len(),
        1,
        "the --candidate path reports exactly one round: {evidence}"
    );
    assert_eq!(rounds[0]["round"], serde_json::json!(1));
    assert_eq!(rounds[0]["rung"], serde_json::json!("build"));
    assert_eq!(rounds[0]["converged"], serde_json::json!(false));
    assert!(
        rounds[0]["diagnostics"]
            .as_array()
            .is_some_and(|diagnostics| !diagnostics.is_empty()),
        "the single round must carry at least one diagnostic: {evidence}"
    );
}
