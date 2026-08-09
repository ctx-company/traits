//! 0151: a completed `--worktree` run's report/summary must state whether the
//! commit it made actually landed. Reuses [`support::init_fixture_repo`]-style
//! fixture construction (mirroring `proof_merge_on_completion.rs`), but with a
//! two-variant fixture trait: a commit-tail trait (three plain `command`
//! sequence items — write a file, `git add -A`, `git commit`) proving the
//! `NotMerged`/`Landed` paths, and the existing no-op `demo` (`cmd = "true"`)
//! shape proving the clean-tree silence path (Watch clause 2).

use std::fs;
use std::path::Path;
use std::process::Command;

use support::{ScratchRoot, assert_exit_code, git_init_on_branch, run_ctx, utf8};

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

/// A scratch git repo carrying one reviewed, activated, provider-free trait
/// whose procedure is exactly three plain `command` sequence items — write a
/// file, stage it, commit it (`argv: ["git", "commit", "-m", ...]`, the same
/// shape every shipped built-in commit tail uses) — so a `--worktree`
/// dispatch always leaves a [`commit_receipt`]-recognizable revision behind.
fn init_commit_fixture_repo(repo: &Path, home: &Path) {
    fs::create_dir_all(repo.join(".ctx/traits/land-demo/generated")).unwrap();
    git_init_on_branch(repo, "main");
    fs::write(repo.join(".gitignore"), ".ctx/traits/worktrees/\n").unwrap();
    fs::write(
        repo.join(".ctx/traits/land-demo/generated/index.toml"),
        r#"id = "land-demo"
schema-version = "0.2"
version = "0.1.0"
name = "Land Demo"
summary = "A provider-free trait whose procedure writes and commits a file."

[procedure]
description = "Write a file, stage it, commit it."

[[slot]]
id = "write-output"
schema = "schema:text"

[[slot]]
id = "stage-output"
schema = "schema:text"

[[slot]]
id = "commit-output"
schema = "schema:text"

[[procedure.sequence]]
id = "write"
title = "Write a change"
kind = "command"
output = ["slot:write-output"]

[procedure.sequence.command]
argv = ["touch", "note.txt"]

[[procedure.sequence]]
id = "stage"
title = "Stage the change"
kind = "command"
output = ["slot:stage-output"]

[procedure.sequence.command]
argv = ["git", "add", "-A"]

[[procedure.sequence]]
id = "commit"
title = "Commit the change"
kind = "command"
output = ["slot:commit-output"]

[procedure.sequence.command]
argv = ["git", "commit", "-m", "landing honesty fixture commit"]
"#,
    )
    .unwrap();
    fs::write(
        repo.join(".ctx/traits/land-demo/trait.toml"),
        "[package]\nid = \"land-demo\"\nversion = \"0.1.0\"\nname = \"Land Demo\"\nstatus = \"draft\"\n",
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

    let fixture = ".ctx/traits/land-demo/generated/index.toml";
    let output = run_ctx(
        &["traits", "review", "--file", fixture, "--approve"],
        repo,
        home,
    );
    assert_exit_code(&output, 0);
    let output = run_ctx(&["traits", "activate", "--file", fixture], repo, home);
    assert_exit_code(&output, 0);
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

/// Same shape as `proof_merge_on_completion.rs`'s `demo` fixture: a single
/// `cmd = "true"` command step that never touches the working tree, so a
/// completed run never has a [`commit_receipt`].
fn init_clean_tree_fixture_repo(repo: &Path, home: &Path) {
    fs::create_dir_all(repo.join(".ctx/traits/demo/generated")).unwrap();
    git_init_on_branch(repo, "main");
    fs::write(repo.join(".gitignore"), ".ctx/traits/worktrees/\n").unwrap();
    fs::write(
        repo.join(".ctx/traits/demo/generated/index.toml"),
        r#"id = "demo"
schema-version = "0.2"
version = "0.1.0"
name = "Demo"
summary = "A provider-free command-only trait."

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
"#,
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
        &["traits", "review", "--file", fixture, "--approve"],
        repo,
        home,
    );
    assert_exit_code(&output, 0);
    let output = run_ctx(&["traits", "activate", "--file", fixture], repo, home);
    assert_exit_code(&output, 0);
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

fn summary_json(ledger_path: &Path) -> serde_json::Value {
    let summary_path = Path::new(&format!("{}.summary.json", ledger_path.display())).to_path_buf();
    let text = fs::read_to_string(&summary_path).unwrap_or_else(|error| {
        panic!(
            "cannot read summary sidecar {}: {error}",
            summary_path.display()
        )
    });
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("summary sidecar was not valid JSON: {error}\n{text}"))
}

/// (a) A `--worktree` run dispatched without `--merge` that committed must
/// have its story and summary sidecar say, in one line, that it is NOT
/// merged — and name the exact `ctx traits merge <run-id>` command.
#[test]
fn worktree_run_without_merge_intent_reports_not_merged() {
    let scratch = ScratchRoot::new("p0151-not-merged");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_commit_fixture_repo(&repo, &scratch.home());

    let run_output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/land-demo/generated/index.toml",
            "--worktree",
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&run_output, 0);
    let run = value_json(&run_output);
    assert!(
        run["value"].get("merge").is_none(),
        "a run dispatched without --merge must never auto-land: {run}"
    );
    let run_id = run["value"]["session"]["run-id"]
        .as_str()
        .unwrap()
        .to_string();
    let ledger_path = Path::new(run["value"]["session-path"].as_str().unwrap()).to_path_buf();

    let summary = summary_json(&ledger_path);
    assert_eq!(
        summary["landing"], "not-merged",
        "summary sidecar must classify a completed, uncommitted-to-main worktree run as not-merged: {summary}"
    );

    let story = run_ctx(&["traits", "story", &run_id], &repo, &scratch.home());
    assert_exit_code(&story, 0);
    let (stdout, _) = utf8(&story);
    assert!(
        stdout.contains("NOT merged into main"),
        "story must state the run is not merged: {stdout}"
    );
    assert!(
        stdout.contains(&format!("ctx traits merge {run_id}")),
        "story must name the exact merge command: {stdout}"
    );
}

/// (b) A completed `--worktree` run whose command produced nothing to commit
/// (clean tree) must render no landing line at all and carry no `landing`
/// field in its summary sidecar — Watch clause 2.
#[test]
fn clean_tree_completed_run_has_no_landing_line() {
    let scratch = ScratchRoot::new("p0151-clean-tree");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_clean_tree_fixture_repo(&repo, &scratch.home());

    let run_output = run_ctx(
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
    assert_exit_code(&run_output, 0);
    let run = value_json(&run_output);
    let run_id = run["value"]["session"]["run-id"]
        .as_str()
        .unwrap()
        .to_string();
    let ledger_path = Path::new(run["value"]["session-path"].as_str().unwrap()).to_path_buf();

    let summary = summary_json(&ledger_path);
    assert!(
        summary.get("landing").is_none(),
        "a clean-tree completed run must carry no landing field: {summary}"
    );

    let story = run_ctx(&["traits", "story", &run_id], &repo, &scratch.home());
    assert_exit_code(&story, 0);
    let (stdout, _) = utf8(&story);
    assert!(
        !stdout.contains("NOT merged"),
        "a clean-tree run must never print not-merged noise: {stdout}"
    );
}

/// (c) A `--merge` run that lands reports `landing == "landed"` in its
/// summary sidecar and states the landed revision in its story disposition.
#[test]
fn merged_run_reports_landed_with_revision() {
    let scratch = ScratchRoot::new("p0151-landed");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_commit_fixture_repo(&repo, &scratch.home());

    let run_output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/land-demo/generated/index.toml",
            "--worktree",
            "--merge",
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&run_output, 0);
    let run = value_json(&run_output);
    assert_eq!(run["value"]["merge"]["status"], "merged");
    let run_id = run["value"]["session"]["run-id"]
        .as_str()
        .unwrap()
        .to_string();
    let ledger_path = Path::new(run["value"]["session-path"].as_str().unwrap()).to_path_buf();

    let summary = summary_json(&ledger_path);
    assert_eq!(
        summary["landing"], "landed",
        "summary sidecar must classify a merged run as landed: {summary}"
    );

    let story = run_ctx(&["traits", "story", &run_id], &repo, &scratch.home());
    assert_exit_code(&story, 0);
    let (stdout, _) = utf8(&story);
    assert!(
        stdout.contains("merged to main (landed "),
        "story disposition for a landed run must state the landed revision: {stdout}"
    );
}
