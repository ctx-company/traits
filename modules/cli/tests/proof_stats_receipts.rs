//! 0120: `ctx traits internal stats` — a scratch-store CLI proof that the aggregation
//! wired at `modules/cli/src/app/stats.rs` reproduces hand-counted numbers
//! over real and synthetic ledgers, is stable `--json` shape, and prints a
//! clean zero state on an empty store.
//!
//! One real ledger is driven end to end (a provider-free `command`-only
//! trait, mirroring `proof_landing_honesty.rs`'s clean-tree fixture); the
//! `killed`, `exhausted-unapproved`, unreadable, and refinement-rounds cases
//! are cheaper synthetic siblings written directly into the same scratch
//! session store by cloning and editing that one real ledger's JSON —
//! exercising the real `read_run_session` deserialization path without
//! driving four more processes end to end. Every fixture is generated fresh
//! by this test run, never committed content (2026-08-08 no-frozen-inventory
//! ruling).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use support::{ScratchRoot, assert_exit_code, git_init, run_ctx, utf8};

const TRAIT_MANIFEST: &str = r#"id = "demo"
schema-version = "0.4"
version = "0.1.0"
name = "Demo"
description = "A provider-free command-only trait."

[procedure]
description = "Run one deterministic command."

[[slot]]
id = "notified"
schema = "schema:text"

[[procedure.sequence]]
id = "command"
title = "Run command"
kind = "command"
cmd = "true"
output = ["slot:notified"]
"#;

fn init_fixture_repo(repo: &Path, home: &Path) {
    fs::create_dir_all(repo.join(".ctx/traits/demo/generated")).unwrap();
    git_init(repo);
    fs::write(
        repo.join(".ctx/traits/demo/generated/index.toml"),
        TRAIT_MANIFEST,
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/demo/trait.toml"),
        "[package]\nid = \"demo\"\nversion = \"0.1.0\"\nname = \"Demo\"\nstatus = \"draft\"\n",
    )
    .unwrap();
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-q", "-m", "init"])
        .current_dir(repo)
        .status()
        .unwrap();

    let fixture = ".ctx/traits/demo/generated/index.toml";
    let output = run_ctx(
        &[
            "traits",
            "internal",
            "review",
            "--file",
            fixture,
            "--approve",
        ],
        repo,
        home,
    );
    assert_exit_code(&output, 0);
    let output = run_ctx(
        &["traits", "internal", "state", "--active", "--file", fixture],
        repo,
        home,
    );
    assert_exit_code(&output, 0);
}

fn value_json(output: &std::process::Output) -> serde_json::Value {
    let (stdout, stderr) = utf8(output);
    let start = stdout
        .lines()
        .position(|line| line.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON envelope in stdout:\n{stdout}\nstderr:\n{stderr}"));
    let json_text = stdout.lines().skip(start).collect::<Vec<_>>().join("\n");
    serde_json::from_str(&json_text)
        .unwrap_or_else(|error| panic!("stdout was not a JSON envelope: {error}\n{stdout}"))
}

fn stats_json(repo: &Path, home: &Path, extra: &[&str]) -> serde_json::Value {
    let mut args = vec!["traits", "internal", "stats", "--json"];
    args.extend_from_slice(extra);
    let output = run_ctx(&args, repo, home);
    assert_exit_code(&output, 0);
    value_json(&output)
}

/// Write `name.json` into `runs_dir`, seeded from `base`'s already-parsed
/// JSON and mutated by `edit`.
fn write_fixture_ledger(
    runs_dir: &Path,
    name: &str,
    base: &serde_json::Value,
    edit: impl FnOnce(&mut serde_json::Value),
) {
    let mut fixture = base.clone();
    edit(&mut fixture);
    fs::write(
        runs_dir.join(format!("{name}.json")),
        serde_json::to_string(&fixture).unwrap(),
    )
    .unwrap();
}

#[test]
fn stats_reproduces_hand_counted_numbers_and_the_empty_store_is_a_clean_zero() {
    let scratch = ScratchRoot::new("stats-receipts");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_fixture_repo(&repo, &scratch.home());

    // Empty store: the clean zero state, before any run exists.
    let empty = stats_json(&repo, &scratch.home(), &[]);
    assert_eq!(empty["schema-version"], serde_json::json!("2"));
    assert_eq!(empty["total-runs"], serde_json::json!(0));
    assert_eq!(empty["unreadable-runs"], serde_json::json!(0));
    assert_eq!(empty["matched-runs"], serde_json::json!(0));
    assert_eq!(empty["outcomes"]["completed"], serde_json::json!(0));
    assert_eq!(empty["outcomes"]["killed"], serde_json::json!(0));
    assert_eq!(
        empty["outcomes"]["exhausted-unapproved"],
        serde_json::json!(0)
    );
    assert_eq!(
        empty["refinement-rounds"]["average-rounds"],
        serde_json::Value::Null
    );
    assert_eq!(empty["traits"], serde_json::json!([]));

    // Drive one real ledger to completion.
    let run_output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&run_output, 0);
    let run = value_json(&run_output);
    let ledger_path = PathBuf::from(run["value"]["session-path"].as_str().expect("session-path"));
    let runs_dir = ledger_path.parent().expect("runs dir").to_path_buf();
    let base: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&ledger_path).unwrap()).unwrap();
    assert_eq!(
        base["status"],
        serde_json::json!("completed"),
        "the driven fixture must actually complete: {base}"
    );

    // One real completed run: hand-counted single-run state.
    let after_one = stats_json(&repo, &scratch.home(), &[]);
    assert_eq!(after_one["total-runs"], serde_json::json!(1));
    assert_eq!(after_one["unreadable-runs"], serde_json::json!(0));
    assert_eq!(after_one["outcomes"]["completed"], serde_json::json!(1));
    assert_eq!(
        after_one["refinement-rounds"]["completed-runs-missing"],
        serde_json::json!(1),
        "the driven fixture writes no `-verdict` slot, so its round coverage is missing, not zero"
    );
    let traits = after_one["traits"].as_array().expect("traits array");
    assert_eq!(traits.len(), 1);
    assert_eq!(traits[0]["trait-id"], serde_json::json!("demo"));
    assert_eq!(traits[0]["runs"], serde_json::json!(1));

    // Killed: status not completed/blocked, typed drive-outcome-kind killed.
    write_fixture_ledger(&runs_dir, "session-killed-fixture", &base, |fixture| {
        fixture["session-id"] = serde_json::json!("session-killed-fixture");
        fixture["run-id"] = serde_json::json!("run-killed-fixture");
        fixture["status"] = serde_json::json!("awaiting-agent-output");
        fixture["last-drive-outcome"]["outcome"] = serde_json::json!("killed");
        fixture["last-drive-outcome"]["recorded-at-epoch"] = serde_json::json!(200);
    });

    // Exhausted-unapproved: terminal Status::Blocked with the loop runtime's
    // own max-iterations-exhausted stop reason.
    write_fixture_ledger(&runs_dir, "session-exhausted-fixture", &base, |fixture| {
        fixture["session-id"] = serde_json::json!("session-exhausted-fixture");
        fixture["run-id"] = serde_json::json!("run-exhausted-fixture");
        fixture["status"] = serde_json::json!("blocked");
        fixture["stop-reason"] = serde_json::json!({"reason": "max-iterations-exhausted"});
        fixture["last-drive-outcome"]["outcome"] = serde_json::json!("blocked");
        fixture["last-drive-outcome"]["recorded-at-epoch"] = serde_json::json!(300);
    });

    // Refinement rounds: a completed run with three `-verdict`-suffixed
    // accepted-slot revisions.
    write_fixture_ledger(&runs_dir, "session-rounds-fixture", &base, |fixture| {
        fixture["session-id"] = serde_json::json!("session-rounds-fixture");
        fixture["run-id"] = serde_json::json!("run-rounds-fixture");
        fixture["status"] = serde_json::json!("completed");
        fixture["last-drive-outcome"]["recorded-at-epoch"] = serde_json::json!(400);
        let base_revision = fixture["slot-revisions"][0].clone();
        let revisions = fixture["slot-revisions"].as_array_mut().expect("array");
        for order in 1..=3 {
            let mut revision = base_revision.clone();
            revision["slot-ref"] = serde_json::json!("slot:review-verdict");
            revision["acceptance-order"] = serde_json::json!(order);
            revisions.push(revision);
        }
    });

    // Unreadable: not valid JSON at all.
    fs::write(
        runs_dir.join("session-corrupt-fixture.json"),
        "{not valid json",
    )
    .unwrap();

    let report = stats_json(&repo, &scratch.home(), &[]);
    assert_eq!(report["total-runs"], serde_json::json!(5));
    assert_eq!(report["unreadable-runs"], serde_json::json!(1));
    assert_eq!(report["trait-matched-runs"], serde_json::json!(4));
    assert_eq!(report["matched-runs"], serde_json::json!(4));
    assert_eq!(report["outcomes"]["completed"], serde_json::json!(2));
    assert_eq!(
        report["outcomes"]["exhausted-unapproved"],
        serde_json::json!(1)
    );
    assert_eq!(report["outcomes"]["killed"], serde_json::json!(1));
    assert_eq!(report["outcomes"]["blocked"], serde_json::json!(0));
    assert_eq!(
        report["outcomes"]["no-outcome-recorded"],
        serde_json::json!(0)
    );
    assert_eq!(report["outcomes"]["other"], serde_json::json!(0));
    assert_eq!(
        report["refinement-rounds"]["average-rounds"],
        serde_json::json!(3.0),
        "only the rounds fixture carries verdict-slot evidence, so the completed-run average is exactly its 3 revisions: {report}"
    );
    assert_eq!(
        report["refinement-rounds"]["completed-runs-observed"],
        serde_json::json!(1)
    );
    assert_eq!(
        report["refinement-rounds"]["completed-runs-missing"],
        serde_json::json!(1),
        "the real driven run is completed but has no verdict-slot evidence"
    );

    // `--since` excludes the killed fixture (recorded-at-epoch 200, well
    // before this cutoff) but keeps the real run (a genuine current-time
    // timestamp) plus the exhausted/rounds fixtures (300, 400).
    let since = stats_json(&repo, &scratch.home(), &["--since", "300"]);
    assert_eq!(since["matched-runs"], serde_json::json!(3));
    assert_eq!(since["outcomes"]["completed"], serde_json::json!(2));
    assert_eq!(
        since["outcomes"]["exhausted-unapproved"],
        serde_json::json!(1)
    );
    assert_eq!(since["outcomes"]["killed"], serde_json::json!(0));

    // `--trait` matches every fixture (all cloned from the same `demo`
    // ledger) and an unknown trait id matches none.
    let matched_trait = stats_json(&repo, &scratch.home(), &["--trait", "demo"]);
    assert_eq!(matched_trait["trait-matched-runs"], serde_json::json!(4));
    let unmatched_trait = stats_json(&repo, &scratch.home(), &["--trait", "no-such-trait"]);
    assert_eq!(unmatched_trait["trait-matched-runs"], serde_json::json!(0));
    assert_eq!(unmatched_trait["traits"], serde_json::json!([]));
}
