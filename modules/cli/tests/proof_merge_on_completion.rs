//! Public-path proofs for P460's completion-to-landing seam: `--merge`
//! precedence/validation, the combined report, and centralized exit codes.

use std::fs;
use std::path::Path;
use std::process::Command;

use support::{ScratchRoot, assert_exit_code, git_init_on_branch, require_success, run_ctx, utf8};

/// Centralized P460 exit statuses, mirrored here rather than imported so a
/// drift in `crate::app::error`'s constants breaks this proof loudly instead
/// of silently tracking whatever the binary happens to emit.
const EXIT_RUN_NOT_COMPLETED: i32 = 3;
const EXIT_MERGE_PARKED: i32 = 4;

/// A scratch git repo carrying one reviewed, activated, provider-free
/// command-only trait (`demo`, running `cmd`), with `.ctx/traits/worktrees/`
/// gitignored and a caller-supplied `Justfile` `test` recipe standing in for
/// this repository's own — so the declared `[merge] gate` (P477) `ctx
/// traits merge` runs inside the generated worktree stays exactly
/// `test_recipe`, never a real workspace build. Declares `[merge] gate =
/// [["just", "test"]]` in `.ctx/config.toml` so these Justfile-recipe-driven
/// proofs keep exercising the gate exactly as the pre-P477 hardcoded chain
/// did — see [`init_fixture_repo_without_gate`] for the no-declaration
/// (product-default) shape.
fn init_fixture_repo(repo: &Path, home: &Path, test_recipe: &str, cmd: &str) {
    init_fixture_repo_on_branch(repo, home, "main", test_recipe, cmd);
}

/// Same as [`init_fixture_repo`], but on `branch` instead of the fixed
/// `"main"` — used by P488 default-branch-discovery proofs.
fn init_fixture_repo_on_branch(
    repo: &Path,
    home: &Path,
    branch: &str,
    test_recipe: &str,
    cmd: &str,
) {
    init_fixture_repo_inner(repo, home, branch, cmd, |repo| {
        fs::write(repo.join("Justfile"), format!("test:\n\t{test_recipe}\n")).unwrap();
        fs::write(
            repo.join(".ctx/config.toml"),
            "[merge]\ngate = [[\"just\", \"test\"]]\n",
        )
        .unwrap();
    });
}

/// Same fixture shape as [`init_fixture_repo`], but with no `Justfile` and no
/// `[merge] gate` declaration at all — the product default. Proves the
/// empty-gate path: landing runs no repository command and still completes.
fn init_fixture_repo_without_gate(repo: &Path, home: &Path, cmd: &str) {
    init_fixture_repo_inner(repo, home, "main", cmd, |_repo| {});
}

fn init_fixture_repo_inner(
    repo: &Path,
    home: &Path,
    branch: &str,
    cmd: &str,
    declare_gate: impl FnOnce(&Path),
) {
    fs::create_dir_all(repo.join(".ctx/traits/demo/generated")).unwrap();
    git_init_on_branch(repo, branch);
    fs::write(repo.join(".gitignore"), ".ctx/traits/worktrees/\n").unwrap();
    declare_gate(repo);
    fs::write(
        repo.join(".ctx/traits/demo/generated/index.toml"),
        format!(
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
cmd = "{cmd}"
output = ["slot:notified"]
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
    require_success(
        "`ctx traits review --approve`",
        &["traits", "review", "--file", fixture, "--approve"],
        repo,
        home,
    );
    require_success(
        "`ctx traits activate`",
        &["traits", "activate", "--file", fixture],
        repo,
        home,
    );
    // `activate` mutates `trait.toml` in the invocation checkout; commit it
    // so `ctx traits merge`'s own "invocation checkout is clean" precondition
    // (unrelated to P460, unchanged by it) does not park every case here.
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
    // A command step's own progress line(s) can precede the pretty-printed
    // JSON envelope on stdout (pre-existing, unrelated to P460); the
    // envelope itself is the suffix starting at the first line beginning
    // with `{`, not any single line.
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

#[test]
fn merge_requires_effective_worktree_and_excludes_no_drive() {
    let scratch = ScratchRoot::new("p460-worktree-required");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&repo)
        .status()
        .unwrap();

    let output = run_ctx(
        &["traits", "run", "--merge", "--no-drive"],
        &repo,
        &scratch.home(),
    );
    assert_ne!(output.status.code(), Some(0));
    let (_, stderr) = utf8(&output);
    assert!(stderr.contains("--merge/--no-merge"), "{stderr}");

    let output = run_ctx(&["traits", "run", "--merge"], &repo, &scratch.home());
    assert_ne!(output.status.code(), Some(0));
    let (_, stderr) = utf8(&output);
    assert!(stderr.contains("effective worktree"), "{stderr}");

    let output = run_ctx(
        &["traits", "session", "start", "--merge"],
        &repo,
        &scratch.home(),
    );
    assert_ne!(output.status.code(), Some(0));
    let (_, stderr) = utf8(&output);
    assert!(stderr.contains("effective worktree"), "{stderr}");
}

#[test]
fn no_merge_intent_output_has_no_merge_section_and_exits_zero() {
    let scratch = ScratchRoot::new("p460-no-merge-compat");
    let repo = scratch.home().join("repo");
    init_fixture_repo(&repo, &scratch.home(), "true", "true");

    let output = run_ctx(
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
    assert_exit_code(&output, 0);
    let envelope = value_json(&output);
    assert!(
        envelope["value"].get("merge").is_none(),
        "no-merge-intent output must omit the merge section entirely: {envelope}"
    );
}

#[test]
fn worktree_merge_lands_automatically_and_exits_zero() {
    let scratch = ScratchRoot::new("p460-lands");
    let repo = scratch.home().join("repo");
    init_fixture_repo(&repo, &scratch.home(), "true", "true");

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
    assert!(
        !repo.join(".ctx/traits/worktrees").exists()
            || fs::read_dir(repo.join(".ctx/traits/worktrees"))
                .unwrap()
                .next()
                .is_none(),
        "a merged run must remove its worktree"
    );
    // P488 done-when 4: this fixture has no `[merge] branch` config, no
    // `origin/HEAD`, and no `init.defaultBranch` — discovery must fall back
    // to the literal "main" assumption and surface it as a named warning.
    let warnings = envelope["value"]["merge"]["warnings"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        warnings.iter().any(|warning| warning
            .as_str()
            .is_some_and(|text| text.starts_with("default branch assumed \"main\""))),
        "a fallback-resolved landing must warn about the assumption: {envelope}"
    );
}

#[test]
fn landing_gate_failure_parks_and_exits_distinct_status() {
    let scratch = ScratchRoot::new("p460-parks");
    let repo = scratch.home().join("repo");
    init_fixture_repo(&repo, &scratch.home(), "exit 1", "true");

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
    assert_exit_code(&output, EXIT_MERGE_PARKED);
    let envelope = value_json(&output);
    assert_eq!(envelope["value"]["merge"]["status"], "parked");
    let worktrees = fs::read_dir(repo.join(".ctx/traits/worktrees")).unwrap();
    assert!(
        worktrees.count() > 0,
        "a parked merge must leave its branch and worktree intact"
    );
}

#[test]
fn explicit_out_path_merge_lands_outside_default_session_store() {
    let scratch = ScratchRoot::new("p460-out-path");
    let repo = scratch.home().join("repo");
    init_fixture_repo(&repo, &scratch.home(), "true", "true");
    let ledger = scratch.home().join("elsewhere").join("session.json");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();

    let output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
            "--worktree",
            "--merge",
            "--out",
            ledger.to_str().unwrap(),
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&output, 0);
    let envelope = value_json(&output);
    assert_eq!(
        envelope["value"]["merge"]["status"], "merged",
        "a driven run with an explicit --out ledger outside the default session store must \
         still auto-land: {envelope}"
    );
    let ledger_text = fs::read_to_string(&ledger).unwrap_or_else(|error| {
        panic!(
            "exact --out ledger {} unreadable: {error}",
            ledger.display()
        )
    });
    let ledger_json: serde_json::Value = serde_json::from_str(&ledger_text).unwrap();
    assert!(
        !ledger_json["provenance"]["merge-frames"]
            .as_array()
            .unwrap()
            .is_empty(),
        "the exact --out ledger must carry the appended merge evidence: {ledger_json}"
    );
}

#[test]
fn parked_merge_is_not_retried_on_a_later_resume() {
    let scratch = ScratchRoot::new("p460-one-shot");
    let repo = scratch.home().join("repo");
    init_fixture_repo(&repo, &scratch.home(), "exit 1", "true");

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
    assert_exit_code(&output, EXIT_MERGE_PARKED);
    let envelope = value_json(&output);
    let session_path = envelope["value"]["session-path"]
        .as_str()
        .expect("parked run reports its session path")
        .to_string();
    let before = fs::read_to_string(&session_path).unwrap();
    let before_json: serde_json::Value = serde_json::from_str(&before).unwrap();
    let frames_before = before_json["provenance"]["merge-frames"]
        .as_array()
        .unwrap()
        .len();
    assert!(frames_before > 0, "the parked attempt must leave evidence");

    let before_reason = before_json["provenance"]["merge-frames"]
        .as_array()
        .unwrap()
        .last()
        .and_then(|frame| frame["reason"].as_str())
        .map(str::to_string);

    // A later `ctx traits drive --session ...` over the already-completed,
    // already-parked session must not attempt (or report) a second merge,
    // but must still exit with the same parked status as the original
    // attempt — never silently succeed.
    let resume = run_ctx(
        &[
            "traits",
            "drive",
            "--session",
            &session_path,
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&resume, EXIT_MERGE_PARKED);
    let resume_envelope = value_json(&resume);
    assert!(
        resume_envelope["value"].get("merge").is_none(),
        "a resume over an already-parked session must not report a fresh merge attempt: {resume_envelope}"
    );
    let after = fs::read_to_string(&session_path).unwrap();
    let after_json: serde_json::Value = serde_json::from_str(&after).unwrap();
    let frames_after = after_json["provenance"]["merge-frames"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(
        frames_before, frames_after,
        "no new merge frame may be appended by the resume: before={before_json} after={after_json}"
    );
    let after_reason = after_json["provenance"]["merge-frames"]
        .as_array()
        .unwrap()
        .last()
        .and_then(|frame| frame["reason"].as_str())
        .map(str::to_string);
    assert_eq!(
        before_reason, after_reason,
        "the original park reason must survive the resume unchanged"
    );
    let worktrees = fs::read_dir(repo.join(".ctx/traits/worktrees")).unwrap();
    assert!(
        worktrees.count() > 0,
        "the original park must still leave its branch and worktree intact"
    );
}

#[test]
fn no_merge_clear_waits_for_the_driver_lock_before_mutating_the_ledger() {
    let scratch = ScratchRoot::new("p460-no-merge-lock-order");
    let repo = scratch.home().join("repo");
    init_fixture_repo(&repo, &scratch.home(), "true", "false");

    // `cmd = "false"` never completes, so this run persists a merge intent
    // on an incomplete session without ever reaching automatic landing —
    // exactly the state a credits-paused-then-resumed drive would leave.
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
        .expect("incomplete run still reports its session path")
        .to_string();
    let before = fs::read_to_string(&session_path).unwrap();
    assert!(
        before.contains("\"merge-intent\""),
        "the fixture must persist a merge intent before this test holds the driver lock: {before}"
    );

    // Hold the driver lock ourselves, exactly as a concurrent live driver
    // would — `drive --no-merge`'s ledger clear must wait behind it.
    let ledger_path = camino::Utf8PathBuf::from_path_buf(std::path::PathBuf::from(&session_path))
        .expect("session path is UTF-8");
    let session_for_lock = ctx_traits_io::run_session::read_run_session(&ledger_path).unwrap();
    let guard = ctx_traits_io::run_control::try_acquire(
        &ctx_traits_io::run_liveness::LiveRunFacts {
            session_id: "held-by-test".to_string(),
            run_id: session_for_lock.run_id.as_str().to_string(),
            repo_key: "test-repo".to_string(),
            repo_path: "/test-repo".to_string(),
            ledger_path: ledger_path.clone(),
            worktree_path: None,
            branch: None,
            log_path: None,
        },
        std::sync::Arc::new(|| {}),
    )
    .unwrap()
    .expect("test process acquires the driver lock uncontended");

    let busy = run_ctx(
        &[
            "traits",
            "drive",
            "--session",
            &session_path,
            "--no-merge",
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &scratch.home(),
    );
    let (_, busy_stderr) = utf8(&busy);
    assert_ne!(
        busy.status.code(),
        Some(0),
        "a drive that cannot acquire the driver lock must not report success: {busy_stderr}"
    );
    let during_lock = fs::read_to_string(&session_path).unwrap();
    assert_eq!(
        before, during_lock,
        "a lock-losing `--no-merge` invocation must leave the ledger byte-identical"
    );

    drop(guard);

    let resumed = run_ctx(
        &[
            "traits",
            "drive",
            "--session",
            &session_path,
            "--no-merge",
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &scratch.home(),
    );
    // The fixture's `cmd = "false"` step still never completes, so the
    // drive itself does not land — only the merge intent was cleared. What
    // this asserts is the ordering contract: once the lock is free, the
    // clear proceeds and this invocation exits without the earlier
    // `EXIT_RUN_NOT_COMPLETED` merge-intent-present status, because there is
    // no longer an intent to report against.
    assert_exit_code(&resumed, 0);
    let after = fs::read_to_string(&session_path).unwrap();
    assert!(
        !after.contains("\"merge-intent\""),
        "once the lock is free, --no-merge must clear the persisted intent: {after}"
    );
}

#[test]
fn incomplete_drive_with_merge_intent_exits_distinct_status_without_merging() {
    let scratch = ScratchRoot::new("p460-not-completed");
    let repo = scratch.home().join("repo");
    init_fixture_repo(&repo, &scratch.home(), "true", "false");

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
        "a run that never completed must never attempt a merge: {envelope}"
    );
}

/// The single stable advisory text an empty (undeclared) `[merge] gate`
/// records — mirrored from `EMPTY_GATE_ADVISORY` in `app::merge` rather than
/// imported, since it is a private implementation constant; a drift between
/// the two breaks this proof loudly instead of silently tracking whatever
/// text the binary happens to emit.
const EMPTY_GATE_ADVISORY: &str =
    "no [merge] gate declared; landing without running a repository command";

/// P477: a repository with no `Justfile` and no `[merge] gate` declaration —
/// the product default — must land end to end, running no repository
/// command, recording the advisory exactly once in BOTH the `--json` merge
/// report's `warnings` array and the plain report's `warning:` line (not
/// only in persisted ledger evidence), retaining the `GatesPassed` ledger
/// frame, and removing its worktree exactly like a passing declared gate
/// would.
#[test]
fn no_gate_declared_lands_with_one_advisory_and_removes_worktree() {
    let scratch = ScratchRoot::new("p477-no-gate-json");
    let repo = scratch.home().join("repo");
    init_fixture_repo_without_gate(&repo, &scratch.home(), "true");

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
    let merge_warnings = envelope["value"]["merge"]["warnings"]
        .as_array()
        .expect("a merge with an empty gate reports a warnings array")
        .iter()
        .filter(|warning| warning.as_str() == Some(EMPTY_GATE_ADVISORY))
        .count();
    assert_eq!(
        merge_warnings, 1,
        "the JSON merge report must carry exactly one matching advisory: {envelope}"
    );
    let session_path = envelope["value"]["session-path"]
        .as_str()
        .expect("merged run reports its session path")
        .to_string();
    let ledger_text = fs::read_to_string(&session_path).unwrap();
    assert!(
        ledger_text.contains("\"status\": \"gates-passed\""),
        "an empty gate must still record the GatesPassed ledger frame: {ledger_text}"
    );
    assert!(
        !repo.join(".ctx/traits/worktrees").exists()
            || fs::read_dir(repo.join(".ctx/traits/worktrees"))
                .unwrap()
                .next()
                .is_none(),
        "a merged run must remove its worktree even with no declared gate"
    );

    // Second, independent fixture: same proof against the PLAIN (non-JSON)
    // report, whose renderer is a distinct code path (`merge::print_report`)
    // from the JSON serialization asserted above.
    let plain_scratch = ScratchRoot::new("p477-no-gate-plain");
    let plain_repo = plain_scratch.home().join("repo");
    init_fixture_repo_without_gate(&plain_repo, &plain_scratch.home(), "true");
    let plain_output = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            ".ctx/traits/demo/generated/index.toml",
            "--worktree",
            "--merge",
            "--progress",
            "none",
        ],
        &plain_repo,
        &plain_scratch.home(),
    );
    assert_exit_code(&plain_output, 0);
    let (plain_stdout, _) = utf8(&plain_output);
    let plain_advisory_lines = plain_stdout
        .lines()
        .filter(|line| line.trim() == format!("warning: {EMPTY_GATE_ADVISORY}"))
        .count();
    assert_eq!(
        plain_advisory_lines, 1,
        "the plain report must print exactly one matching advisory line: {plain_stdout}"
    );
}

/// P477: a declared `[merge] gate = [["false"]]` must park before the
/// default branch advances, retaining the run's branch/worktree and
/// recording the declared argv and captured-output path — exactly like a
/// failing `just test` did under the pre-P477 hardcoded chain.
#[test]
fn declared_false_gate_parks_and_retains_branch_and_worktree() {
    let scratch = ScratchRoot::new("p477-false-gate");
    let repo = scratch.home().join("repo");
    init_fixture_repo_inner(&repo, &scratch.home(), "main", "true", |repo| {
        fs::write(
            repo.join(".ctx/config.toml"),
            "[merge]\ngate = [[\"false\"]]\n",
        )
        .unwrap();
    });

    let main_rev_before = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap()
            .stdout,
    )
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
    assert_exit_code(&output, EXIT_MERGE_PARKED);
    let envelope = value_json(&output);
    assert_eq!(envelope["value"]["merge"]["status"], "parked");
    let main_rev_after = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(
        main_rev_before, main_rev_after,
        "a parked gate must never advance main"
    );
    let worktrees = fs::read_dir(repo.join(".ctx/traits/worktrees")).unwrap();
    assert!(
        worktrees.count() > 0,
        "a parked gate must leave its branch and worktree intact"
    );
    let session_path = envelope["value"]["session-path"]
        .as_str()
        .expect("parked run reports its session path")
        .to_string();
    let ledger_text = fs::read_to_string(&session_path).unwrap();
    assert!(
        ledger_text.contains("gate=false") && ledger_text.contains("argv="),
        "the declared argv must be recorded in the parked evidence: {ledger_text}"
    );
    assert!(
        ledger_text.contains("gate-output="),
        "the captured-output path must be recorded in the parked evidence: {ledger_text}"
    );
}

/// Clone `upstream` (a fixture repo already on `branch`) into a fresh
/// directory under `home`, so `origin/HEAD` is populated the way a real
/// clone of a `master`/`develop`/`trunk`-default repository would be — the
/// P488 done-when 1 real-world story.
fn clone_fixture(upstream: &Path, home: &Path, label: &str) -> std::path::PathBuf {
    let repo = home.join(label);
    let output = Command::new("git")
        .args([
            "clone",
            "--quiet",
            upstream.to_str().unwrap(),
            repo.to_str().unwrap(),
        ])
        .output()
        .unwrap_or_else(|error| panic!("cannot run git clone: {error}"));
    assert!(
        output.status.success(),
        "git clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    repo
}

#[test]
fn trunk_default_branch_lands_end_to_end_with_untouched_config() {
    let scratch = ScratchRoot::new("p488-trunk-e2e");
    let upstream = scratch.home().join("upstream");
    fs::create_dir_all(&upstream).unwrap();
    init_fixture_repo_on_branch(&upstream, &scratch.home(), "trunk", "true", "true");
    let repo = clone_fixture(&upstream, &scratch.home(), "repo");

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
    let branch = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(
        branch.trim(),
        "trunk",
        "the run must land on the cloned repo's actual default branch, never a literal \"main\""
    );
}

#[test]
fn merge_branch_config_override_beats_discovery_and_names_both_branches() {
    let scratch = ScratchRoot::new("p488-config-override");
    let upstream = scratch.home().join("upstream");
    fs::create_dir_all(&upstream).unwrap();
    init_fixture_repo_on_branch(&upstream, &scratch.home(), "trunk", "true", "true");
    let repo = clone_fixture(&upstream, &scratch.home(), "repo");

    fs::write(
        repo.join(".ctx/config.toml"),
        "[merge]\ngate = [[\"just\", \"test\"]]\nbranch = \"release\"\n",
    )
    .unwrap();
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(&repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-q", "-m", "declare merge.branch override"])
        .current_dir(&repo)
        .status()
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
    assert_exit_code(&output, EXIT_MERGE_PARKED);
    let envelope = value_json(&output);
    assert_eq!(envelope["value"]["merge"]["status"], "parked");
    let reason = envelope["value"]["merge"]["reason"]
        .as_str()
        .unwrap_or_default();
    assert!(
        reason.contains("release"),
        "the refusal must name the expected (configured) branch: {reason}"
    );
    assert!(
        reason.contains("trunk"),
        "the refusal must name the actual (discovered) branch: {reason}"
    );

    let doctor_stdout = require_success(
        "`ctx traits doctor --config --json`",
        &["traits", "doctor", "--config", "--json"],
        &repo,
        &scratch.home(),
    );
    let doctor_report: serde_json::Value =
        serde_json::from_str(&doctor_stdout).unwrap_or_else(|error| {
            panic!("doctor --config --json was not JSON: {error}\n{doctor_stdout}")
        });
    assert_eq!(
        doctor_report["knobs"]["merge.branch"]["value"], "release",
        "doctor must show the configured override, not a discovered value: {doctor_report}"
    );
    assert_eq!(
        doctor_report["knobs"]["merge.branch"]["winner"]["layer"], "repo",
        "doctor must attribute merge.branch to the project config layer: {doctor_report}"
    );
}

#[test]
fn init_default_branch_config_layer_is_discovered_without_origin() {
    let scratch = ScratchRoot::new("p488-init-default-branch");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init_on_branch(&repo, "trunk");
    let config_output = Command::new("git")
        .args(["config", "init.defaultBranch", "trunk"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(
        config_output.status.success(),
        "git config init.defaultBranch failed: {}",
        String::from_utf8_lossy(&config_output.stderr)
    );

    let stdout = require_success(
        "`ctx traits doctor --config --json`",
        &["traits", "doctor", "--config", "--json"],
        &repo,
        &scratch.home(),
    );
    let report: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("doctor --config --json was not JSON: {error}\n{stdout}"));
    assert_eq!(
        report["knobs"]["merge.branch"]["value"], "trunk (discovered: init.defaultBranch)",
        "doctor must name the resolved branch and its source layer when no origin is set: \
         {report}"
    );
}
