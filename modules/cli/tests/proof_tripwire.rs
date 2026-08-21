//! Behavioral proofs for P479's out-of-tree mutation tripwire: a `--worktree`
//! run whose command step writes into the invocation checkout (never its own
//! worktree) must park un-landably under the default policy, warn-and-land
//! under `policy = "warn"`, and surface in `merge`, `session state`, and
//! `doctor --config` — all provider-free, no paid harness involved.

use std::fs;
use std::path::Path;
use std::process::Command;

use support::{ScratchRoot, assert_exit_code, git_init_on_branch, run_ctx, utf8};

const EXIT_RUN_NOT_COMPLETED: i32 = 3;

/// A scratch git repo carrying one reviewed, activated, provider-free
/// command-only trait (`demo`) whose command is `cmd`, with `.ctx/traits/worktrees/`
/// gitignored and no `[merge] gate` declared (the product default — a clean
/// run lands with no repository command at all). Modeled on
/// `proof_merge_on_completion.rs`'s `init_fixture_repo_inner`, reused rather
/// than re-derived.
fn init_fixture_repo(repo: &Path, home: &Path, cmd: &str, extra_config: &str) {
    fs::create_dir_all(repo.join(".ctx/traits/demo/generated")).unwrap();
    git_init_on_branch(repo, "main");
    fs::write(
        repo.join(".gitignore"),
        ".ctx/traits/worktrees/\n.ctx/traits/runtime.toml\n",
    )
    .unwrap();
    if !extra_config.is_empty() {
        fs::write(repo.join(".ctx/traits/runtime.toml"), extra_config).unwrap();
    }
    fs::write(
        repo.join(".ctx/traits/demo/generated/index.toml"),
        format!(
            r#"id = "demo"
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
output = ["slot:notified"]

[procedure.sequence.command]
argv = ["sh", "-c", "{cmd}"]
"#
        ),
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
    // `activate` mutates `trait.toml` in the invocation checkout; commit it
    // so the merge preflight's own "invocation checkout is clean" precondition
    // does not park every case here, independent of P479.
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-q", "-m", "activate"])
        .current_dir(repo)
        .status()
        .unwrap();
}

fn value_json(output: &std::process::Output) -> serde_json::Value {
    let (stdout, _) = utf8(output);
    let start = stdout
        .lines()
        .position(|line| line.trim_start().starts_with('{'));
    let json_text = match start {
        Some(index) => stdout.lines().skip(index).collect::<Vec<_>>().join("\n"),
        None => stdout.clone(),
    };
    serde_json::from_str(&json_text)
        .unwrap_or_else(|error| panic!("stdout was not a JSON envelope: {error}\n{stdout}"))
}

/// A `cmd` that writes into `<repo>/.ctx/traits/runtime.toml` — gitignored in this
/// fixture, so this is exactly the P477 incident class the sentinel leg
/// exists to cover, not the status leg. Writes a syntactically valid,
/// schema-compliant one-line TOML document (an empty `[worktree]` table),
/// since some cases here exercise commands *after* this mutation (`session
/// state`, `doctor --config`, a `warn`-policy landing) that re-resolve
/// runtime config from this same file — garbage content would fail those for
/// reasons unrelated to the tripwire.
fn write_gitignored_config_cmd(repo: &Path) -> String {
    format!(
        "printf '[worktree]' > {}/.ctx/traits/runtime.toml",
        repo.canonicalize().unwrap().display()
    )
}

/// A `cmd` that writes into a *tracked* file in the invocation checkout.
fn write_tracked_file_cmd(repo: &Path) -> String {
    format!(
        "printf x >> {}/.ctx/traits/demo/trait.toml",
        repo.canonicalize().unwrap().display()
    )
}

#[test]
fn gitignored_invocation_repo_write_parks_and_does_not_land() {
    let scratch = ScratchRoot::new("p479-gitignored-parks");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    let cmd = write_gitignored_config_cmd(&repo);
    init_fixture_repo(&repo, &scratch.home(), &cmd, "");

    let before_head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&repo)
        .output()
        .unwrap();

    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
            "--worktree",
            "--merge",
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&output, EXIT_RUN_NOT_COMPLETED);
    let envelope = value_json(&output);
    assert!(
        envelope["value"].get("merge").is_none(),
        "a tripped run must not report a landing attempt: {envelope}"
    );

    let after_head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert_eq!(
        utf8(&before_head).0,
        utf8(&after_head).0,
        "main must be unchanged by a parked-on-tripwire run"
    );

    let session_path = envelope["value"]["session-path"]
        .as_str()
        .expect("run reports its session path")
        .to_string();
    let ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&session_path).unwrap()).unwrap();
    let mutations = ledger["provenance"]["out-of-tree-mutations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(mutations.len(), 1, "exactly one finding: {ledger}");
    let finding = &mutations[0];
    assert!(
        finding["paths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path.as_str().unwrap().contains(".ctx/traits/runtime.toml")),
        "finding must name the offending path: {finding}"
    );
    assert_eq!(finding["policy"], "park");
    assert!(
        finding["frame"].as_str().unwrap().contains("Run command"),
        "finding must carry a frame label naming the frame that ran: {finding}"
    );

    let worktrees = fs::read_dir(repo.join(".ctx/traits/worktrees")).unwrap();
    assert!(
        worktrees.count() > 0,
        "a park must leave branch and worktree intact"
    );

    // Explicit later `ctx traits merge <run-id>` (default session store,
    // matching what `ctx traits run` itself just used) must refuse too. This
    // run never reached a completed drive (the tripwire stopped it before
    // the trait's own frames finished), so it is caught by merge's existing,
    // more general "not a completed drive" precondition rather than the new
    // out-of-tree-mutation-specific one — still a refusal, just via the
    // pre-existing gate; see `seeded_park_evidence_refuses_an_explicit_merge_of_an_otherwise_clean_completion`
    // for a direct exercise of the new merge-side precondition itself.
    let run_id = ledger["run-id"]
        .as_str()
        .expect("ledger carries its run-id")
        .to_string();
    let merge_output = run_ctx(
        &["traits", "merge", &run_id, "--json"],
        &repo,
        &scratch.home(),
    );
    assert_ne!(merge_output.status.code(), Some(0));
    // This run never reached a completed drive, so `merge()` refuses before
    // building any report at all (a plain `Error::Command`, still printed
    // in full on stderr by the generic error boundary) — unlike a merge
    // that DOES produce a report with a non-`merged` status, which now
    // reports via `Error::AlreadyReported` after printing that report to
    // stdout instead (see the seeded-park-evidence test below).
    let (_, merge_stderr) = utf8(&merge_output);
    assert!(
        merge_stderr.contains("out-of-tree-mutation"),
        "refusal must name the tripped outcome: {merge_stderr}"
    );
}

#[test]
fn seeded_park_evidence_refuses_an_explicit_merge_of_an_otherwise_clean_completion() {
    // Direct exercise of the merge-side precondition (draft §4.6): a run
    // whose LATEST drive completed cleanly (so the generic "not a completed
    // drive" gate does not itself refuse), but whose provenance still
    // carries an unacknowledged `policy = "park"` finding from an earlier
    // drive attempt, must still refuse to land — this is what makes a P479
    // park un-landable across invocations, not just within the one drive
    // that tripped it.
    let scratch = ScratchRoot::new("p479-seeded-park-evidence");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_fixture_repo(&repo, &scratch.home(), "true", "");

    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
            "--worktree",
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&output, 0);
    let envelope = value_json(&output);
    assert!(
        envelope["value"].get("merge").is_none(),
        "no --merge flag was passed, so no landing attempt should occur: {envelope}"
    );
    let session_path = envelope["value"]["session-path"]
        .as_str()
        .expect("run reports its session path")
        .to_string();

    let mut ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&session_path).unwrap()).unwrap();
    ledger["provenance"]["out-of-tree-mutations"] = serde_json::json!([{
        "paths": [".ctx/traits/runtime.toml"],
        "frame": "run 1 / source 1: Run command (item:command, kind:command)",
        "policy": "park",
        "detected-at-epoch": 1,
    }]);
    fs::write(
        &session_path,
        serde_json::to_string_pretty(&ledger).unwrap(),
    )
    .unwrap();

    let run_id = ledger["run-id"].as_str().unwrap().to_string();
    let merge_output = run_ctx(
        &["traits", "merge", &run_id, "--json"],
        &repo,
        &scratch.home(),
    );
    assert_ne!(merge_output.status.code(), Some(0));
    let merge_envelope = value_json(&merge_output);
    assert_eq!(merge_envelope["value"]["status"], "parked");
    assert!(
        merge_envelope["value"]["reason"]
            .as_str()
            .unwrap()
            .contains(".ctx/traits/runtime.toml"),
        "park reason must name the offending path: {merge_envelope}"
    );

    let worktrees = fs::read_dir(repo.join(".ctx/traits/worktrees")).unwrap();
    assert!(
        worktrees.count() > 0,
        "the seeded park must leave branch and worktree intact"
    );
}

#[test]
fn tracked_file_write_in_invocation_repo_also_parks() {
    let scratch = ScratchRoot::new("p479-tracked-parks");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    let cmd = write_tracked_file_cmd(&repo);
    init_fixture_repo(&repo, &scratch.home(), &cmd, "");

    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
            "--worktree",
            "--merge",
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&output, EXIT_RUN_NOT_COMPLETED);
    let envelope = value_json(&output);
    let session_path = envelope["value"]["session-path"]
        .as_str()
        .expect("run reports its session path")
        .to_string();
    let ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&session_path).unwrap()).unwrap();
    let mutations = ledger["provenance"]["out-of-tree-mutations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(mutations.len(), 1, "exactly one finding: {ledger}");
    assert!(
        mutations[0]["paths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path.as_str().unwrap().contains("trait.toml")),
        "finding must name the tracked path: {mutations:?}"
    );
}

#[test]
fn warn_policy_still_lands_but_carries_evidence_and_a_warning() {
    let scratch = ScratchRoot::new("p479-warn-lands");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    let cmd = write_gitignored_config_cmd(&repo);
    init_fixture_repo(
        &repo,
        &scratch.home(),
        &cmd,
        "[worktree.tripwire]\npolicy = \"warn\"\n",
    );

    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
            "--worktree",
            "--merge",
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&output, 0);
    let envelope = value_json(&output);
    assert_eq!(envelope["value"]["merge"]["status"], "merged");

    let session_path = envelope["value"]["session-path"]
        .as_str()
        .expect("run reports its session path")
        .to_string();
    let ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&session_path).unwrap()).unwrap();
    let mutations = ledger["provenance"]["out-of-tree-mutations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        mutations.len(),
        1,
        "warn policy still records evidence: {ledger}"
    );
    assert_eq!(mutations[0]["policy"], "warn");

    let (stdout, stderr) = utf8(&output);
    assert!(
        stdout.contains("out-of-tree") || stderr.contains("out-of-tree"),
        "warn policy must surface a visible warning: stdout={stdout} stderr={stderr}"
    );
}

#[test]
fn clean_worktree_run_lands_exactly_as_it_does_without_the_tripwire() {
    let scratch = ScratchRoot::new("p479-clean-no-regression");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_fixture_repo(&repo, &scratch.home(), "true", "");

    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
            "--worktree",
            "--merge",
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&output, 0);
    let envelope = value_json(&output);
    assert_eq!(envelope["value"]["merge"]["status"], "merged");
    let session_path = envelope["value"]["session-path"]
        .as_str()
        .expect("run reports its session path")
        .to_string();
    let ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&session_path).unwrap()).unwrap();
    assert!(
        ledger["provenance"]["out-of-tree-mutations"]
            .as_array()
            .map(|array| array.is_empty())
            .unwrap_or(true),
        "a clean run must carry no findings: {ledger}"
    );
}

#[test]
fn session_state_and_doctor_config_surface_the_tripwire() {
    let scratch = ScratchRoot::new("p479-surfaces");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    let cmd = write_gitignored_config_cmd(&repo);
    init_fixture_repo(&repo, &scratch.home(), &cmd, "");

    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
            "--worktree",
            "--merge",
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&output, EXIT_RUN_NOT_COMPLETED);
    let envelope = value_json(&output);
    let session_path = envelope["value"]["session-path"]
        .as_str()
        .expect("run reports its session path")
        .to_string();

    let state_output = run_ctx(
        &[
            "traits",
            "internal",
            "session",
            "state",
            "--session",
            &session_path,
        ],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&state_output, 0);
    let (stdout, _) = utf8(&state_output);
    assert!(
        stdout.contains("out-of-tree-mutations"),
        "session state must render the tripwire block: {stdout}"
    );

    let doctor_output = run_ctx(&["traits", "doctor", "--config"], &repo, &scratch.home());
    assert_exit_code(&doctor_output, 0);
    let (doctor_stdout, _) = utf8(&doctor_output);
    assert!(
        doctor_stdout.contains("worktree.tripwire.policy")
            && doctor_stdout.contains("worktree.tripwire.sentinel"),
        "doctor --config must show both tripwire knobs: {doctor_stdout}"
    );
}
