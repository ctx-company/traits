//! Public-path proofs for P460's completion-to-landing seam: `--merge`
//! precedence/validation, the combined report, and centralized exit codes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use support::{
    ScratchRoot, assert_exit_code, git_init_on_branch, require_success, run_ctx, run_ctx_with_env,
    spawn_ctx, utf8,
};

/// Centralized P460 exit statuses, mirrored here rather than imported so a
/// drift in `crate::app::error`'s constants breaks this proof loudly instead
/// of silently tracking whatever the binary happens to emit.
const EXIT_RUN_NOT_COMPLETED: i32 = 3;
const EXIT_MERGE_PARKED: i32 = 4;
const EXIT_MERGE_FAILED: i32 = 5;

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

/// Install a deliberately tiny custom merger that records every real harness
/// dispatch. The probe is separate so assertions count model-spend, not setup.
fn install_counting_merger(repo: &Path, marker: &Path) {
    let script = repo.join("counting-merger.sh");
    fs::write(
        &script,
        format!(
            // `%s` with the JSON as an ARGUMENT, never as the printf format:
            // ubuntu's /bin/sh is dash, whose printf leaves `\"` in a format
            // string literal — the receipt came out as `{{\\\"result\\\"...`,
            // invalid JSON, and every merger-dispatching proof parked red on
            // Linux while Macs (bash printf eats `\"`) stayed green.
            "#!/bin/sh\nif [ \"$1\" = \"--probe\" ]; then echo merger-fixture-1.0; exit 0; fi\nprintf 'call\\n' >> '{}'\nprintf '%s\\n' '{{\"result\":\"proceed\"}}'\n",
            marker.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::write(
        repo.join("ctx.toml"),
        format!(
            r#"schema-version = "0.2"

[harness.counter]
kind = "custom"
bin = "{}"
transports = ["cli"]
version-probe = ["--probe"]

[harness.counter.cli]
argv = []
output = "raw-json"
model-flag = "--model"
reasoning-effort-flag = "--reasoning-effort"

[agent.role.merger]
harness = "counter"
model = "fixture"
reasoning-effort = "low"

[worktree.confinement]
sandbox = false
"#,
            script.display()
        ),
    )
    .unwrap();
    Command::new("git")
        .args(["add", "ctx.toml"])
        .current_dir(repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-qm", "configure counting merger"])
        .current_dir(repo)
        .status()
        .unwrap();
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
    "no [merge] gate declared; post-run without running a repository command";

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

/// P477: a declared failing gate must park before the
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
            "[merge]\ngate = [[\"sh\", \"-c\", \"echo gate >> gate-count; exit 1\"]]\n",
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
    let worktree_root = repo.join(".ctx/traits/worktrees");
    let worktrees = fs::read_dir(&worktree_root).unwrap();
    assert!(
        worktrees.count() > 0,
        "a parked gate must leave its branch and worktree intact"
    );
    let worktree = fs::read_dir(&worktree_root)
        .unwrap()
        .next()
        .expect("parked run retains one worktree")
        .unwrap()
        .path();
    assert_eq!(
        fs::read_to_string(worktree.join("gate-count"))
            .unwrap()
            .lines()
            .count(),
        1,
        "a gate verdict must park on its first invocation instead of spending retry attempts"
    );
    let session_path = envelope["value"]["session-path"]
        .as_str()
        .expect("parked run reports its session path")
        .to_string();
    let ledger_text = fs::read_to_string(&session_path).unwrap();
    assert!(
        ledger_text.contains("gate=sh-c-") && ledger_text.contains("argv="),
        "the declared argv must be recorded in the parked evidence: {ledger_text}"
    );
    assert!(
        ledger_text.contains("gate-output="),
        "the captured-output path must be recorded in the parked evidence: {ledger_text}"
    );
}

/// A target movement after the first gate is a mechanical race: the next
/// attempt rebases against the new target and reruns the declared gate without
/// asking an owner to intervene.
#[test]
fn target_advance_retries_mechanically_and_reruns_the_gate() {
    let scratch = ScratchRoot::new("p478-mechanical-race");
    let repo = scratch.home().join("repo");
    let raced = repo.join("raced");
    let gate_count = repo.join("gate-count");
    let repo_text = repo.to_string_lossy();
    let raced_text = raced.to_string_lossy();
    let gate_count_text = gate_count.to_string_lossy();
    init_fixture_repo_inner(&repo, &scratch.home(), "main", "true", |repo| {
        fs::write(
            repo.join(".ctx/config.toml"),
            format!(
                "[merge]\nretry-attempts = 2\nretry-backoff-ms = 1\ngate = [[\"sh\", \"-c\", \"echo gate >> '{gate_count_text}'; if [ ! -f '{raced_text}' ]; then touch '{raced_text}'; echo target-race > '{repo_text}/target-race'; git -C '{repo_text}' add target-race; git -C '{repo_text}' commit -qm target-race; fi\"]]\n"
            ),
        )
        .unwrap();
    });

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
    assert_eq!(
        fs::read_to_string(&gate_count).unwrap().lines().count(),
        2,
        "each real landing attempt must rerun the declared gate"
    );
    assert!(
        repo.join("target-race").is_file(),
        "the target-side commit made during the first gate must survive landing"
    );
    let warnings = envelope["value"]["merge"]["warnings"]
        .as_array()
        .expect("retry evidence is returned as warnings");
    assert!(
        warnings.iter().any(|warning| warning
            .as_str()
            .is_some_and(|warning| warning.starts_with("merge-race-captured-target attempt=2/2"))),
        "a retry must recapture its target revision: {envelope}"
    );
}

/// A race after the paid confirmation must not buy another harness dispatch.
/// The retry's rebase conflicts with the new target, so it parks mechanically
/// rather than returning to reconciliation or spending another model call.
#[test]
fn race_retry_conflict_parks_without_a_second_merger_dispatch() {
    let scratch = ScratchRoot::new("p478-mechanical-conflict-no-merger");
    let repo = scratch.home().join("repo");
    let marker = repo.join("merger-calls");
    let raced = repo.join("raced");
    let repo_text = repo.to_string_lossy();
    let raced_text = raced.to_string_lossy();
    init_fixture_repo_inner(&repo, &scratch.home(), "main", "true", |repo| {
        fs::write(
            repo.join(".ctx/config.toml"),
            format!(
                "[merge]\nretry-attempts = 2\nretry-backoff-ms = 1\ngate = [[\"sh\", \"-c\", \"if [ ! -f '{raced_text}' ]; then touch '{raced_text}'; echo target > '{repo_text}/conflict'; git -C '{repo_text}' add conflict; git -C '{repo_text}' commit -qm target-race; fi\"]]\n"
            ),
        )
        .unwrap();
    });
    install_counting_merger(&repo, &marker);

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
    let worktree = Path::new(
        run["value"]["session"]["provenance"]["worktree"]["path"]
            .as_str()
            .unwrap(),
    );
    fs::write(worktree.join("conflict"), "run\n").unwrap();
    Command::new("git")
        .args(["add", "conflict"])
        .current_dir(worktree)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-qm", "run conflict"])
        .current_dir(worktree)
        .status()
        .unwrap();

    let output = run_ctx(
        &["traits", "merge", &run_id, "--force-merger", "--json"],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&output, EXIT_MERGE_PARKED);
    let ledger = fs::read_to_string(run["value"]["session-path"].as_str().unwrap()).unwrap();
    assert_eq!(
        fs::read_to_string(marker)
            .unwrap_or_default()
            .lines()
            .count(),
        1,
        "a retry requiring reconciliation must park without a second merger dispatch: {ledger}"
    );
    assert!(
        ledger.contains("\"stage\": \"rebase\"") && ledger.contains("\"status\": \"parked\""),
        "the mechanical retry must park at rebase rather than reenter reconciliation: {ledger}"
    );
}

/// Mirrored from `reasons::CHECKOUT_BRANCH_PROBE_FAILED` in `app::merge_story`
/// rather than imported (a private implementation constant), so a drift
/// between the two breaks this proof loudly instead of silently tracking
/// whatever text the binary happens to emit.
const CHECKOUT_BRANCH_PROBE_FAILED: &str = "failed to determine invocation checkout branch";

/// A race at the invocation checkout's branch probe (`git rev-parse
/// --abbrev-ref HEAD`, the only such call site on the merge path) must not
/// buy a merger dispatch either: every race retry is mechanical-only, so
/// when the retried rebase meets a divergent base that would need
/// reconciliation, the run parks instead of ever invoking the merger.
#[test]
#[cfg(unix)]
fn checkout_branch_probe_race_retry_parks_with_zero_merger_dispatches() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = ScratchRoot::new("p478-checkout-branch-probe-race");
    let repo = scratch.home().join("repo");
    let marker = repo.join("merger-calls");
    init_fixture_repo_inner(&repo, &scratch.home(), "main", "true", |repo| {
        fs::write(
            repo.join(".ctx/config.toml"),
            "[merge]\nretry-attempts = 2\nretry-backoff-ms = 1\ngate = [[\"true\"]]\n",
        )
        .unwrap();
    });
    install_counting_merger(&repo, &marker);

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
    let worktree = Path::new(
        run["value"]["session"]["provenance"]["worktree"]["path"]
            .as_str()
            .unwrap(),
    );
    fs::write(worktree.join("conflict"), "run\n").unwrap();
    Command::new("git")
        .args(["add", "conflict"])
        .current_dir(worktree)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-qm", "run conflict"])
        .current_dir(worktree)
        .status()
        .unwrap();

    // The divergent base, committed on `main` in the invocation checkout
    // BEFORE merge (not dirty — preflight requires a clean checkout): had
    // attempt 1 reached the merger, it would have needed reconciliation.
    fs::write(repo.join("conflict"), "main\n").unwrap();
    Command::new("git")
        .args(["add", "conflict"])
        .current_dir(&repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-qm", "main conflict"])
        .current_dir(&repo)
        .status()
        .unwrap();

    // A fail-once `git` shim: the first `rev-parse --abbrev-ref` (the
    // checkout-branch probe) fails, forcing the race; every other call
    // (including every later probe and every gate/rebase git invocation)
    // passes through to the real `git`.
    let shim_dir = scratch.home().join("shim");
    fs::create_dir_all(&shim_dir).unwrap();
    let once_marker = scratch.home().join("probe-raced-once");
    let real_git = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    assert!(!real_git.is_empty(), "cannot resolve a real `git` on PATH");
    let shim_script = shim_dir.join("git");
    fs::write(
        &shim_script,
        format!(
            "#!/bin/sh\nif [ \"$1 $2\" = \"rev-parse --abbrev-ref\" ] && [ ! -f '{once}' ]; then\n  touch '{once}'\n  echo 'shim: forcing checkout-branch-probe race' >&2\n  exit 1\nfi\nexec '{git}' \"$@\"\n",
            once = once_marker.display(),
            git = real_git,
        ),
    )
    .unwrap();
    fs::set_permissions(&shim_script, fs::Permissions::from_mode(0o755)).unwrap();

    let original_path = std::env::var("PATH").unwrap_or_default();
    let shimmed_path = format!("{}:{}", shim_dir.display(), original_path);

    let output = run_ctx_with_env(
        &["traits", "merge", &run_id, "--json"],
        &repo,
        &scratch.home(),
        &[("PATH", &shimmed_path)],
    );
    assert_exit_code(&output, EXIT_MERGE_PARKED);
    assert!(
        once_marker.is_file(),
        "the shim never intercepted the checkout-branch probe, so this proof would pass \
         vacuously without the race it exists to force"
    );
    assert_eq!(
        fs::read_to_string(&marker)
            .unwrap_or_default()
            .lines()
            .count(),
        0,
        "the merger harness must never be invoked across a checkout-branch-probe retry-then-park"
    );
    let ledger = fs::read_to_string(run["value"]["session-path"].as_str().unwrap()).unwrap();
    assert!(
        ledger.contains(&format!("merge-race-class={CHECKOUT_BRANCH_PROBE_FAILED}")),
        "the ledger must record that the checkout-branch-probe race actually fired: {ledger}"
    );
    assert!(
        ledger.contains("\"stage\": \"rebase\"") && ledger.contains("\"status\": \"parked\""),
        "the mechanical retry must park at rebase rather than reenter reconciliation: {ledger}"
    );
}

/// A fixture repo whose `[merge] gate` blocks: it touches `gate_entered`
/// (proving the holding merge process has reached the gate stage, inside the
/// lock — `modules/core/src/procedure/session.rs:500-512`) and then spins
/// until `release_gate` exists. `gate-seconds`/`retry-backoff-ms` bound the
/// pinned window and any incidental retry so a broken proof fails in minutes,
/// never hangs. Marker paths are absolute under `home` because the gate's
/// cwd is the generated worktree, not `home`.
fn init_fixture_repo_with_blocking_gate(
    repo: &Path,
    home: &Path,
    gate_entered: &Path,
    release_gate: &Path,
) {
    init_fixture_repo_inner(repo, home, "main", "true", |repo| {
        fs::write(
            repo.join(".ctx/config.toml"),
            format!(
                "[merge]\ngate = [[\"sh\", \"-c\", \"touch '{}'; until [ -f '{}' ]; do sleep 0.05; done\"]]\ngate-seconds = 60\nretry-backoff-ms = 25\n",
                gate_entered.display(),
                release_gate.display()
            ),
        )
        .unwrap();
    });
}

/// [`install_counting_merger`], plus a declared `[agent.role.merger].budget`
/// `frame-seconds`, so the derived lock-wait ceiling
/// (`merge_lock_wait_timeout_ms`, `modules/cli/src/app/merge.rs:206-214`)
/// collapses from `25 * 900s` to a few minutes — a hang bound if this proof
/// ever regresses, never a tuned expectation for the healthy path.
fn install_counting_merger_with_frame_budget(repo: &Path, marker: &Path, frame_seconds: u64) {
    install_counting_merger(repo, marker);
    let ctx_toml = repo.join("ctx.toml");
    let mut contents = fs::read_to_string(&ctx_toml).unwrap();
    contents.push_str(&format!(
        "\n[agent.role.merger.budget]\nframe-seconds = {frame_seconds}\n"
    ));
    fs::write(&ctx_toml, contents).unwrap();
    Command::new("git")
        .args(["add", "ctx.toml"])
        .current_dir(repo)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-qm", "bound merger frame budget"])
        .current_dir(repo)
        .status()
        .unwrap();
}

/// Poll `check` at a short fixed interval until it returns `true` or
/// `deadline` passes. Returns whether `check` ever succeeded.
fn poll_until(deadline: Instant, mut check: impl FnMut() -> bool) -> bool {
    loop {
        if check() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Locate the single `merge.lock` file under the scratch `home` (HOME/XDG are
/// scratch-rooted by `controlled_command`, so exactly one repo's runs root —
/// and thus exactly one `merge.lock` — exists under it).
fn find_merge_lock(home: &Path) -> PathBuf {
    fn walk(dir: &Path) -> Option<PathBuf> {
        for entry in fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = walk(&path) {
                    return Some(found);
                }
            } else if path.file_name().and_then(|name| name.to_str()) == Some("merge.lock") {
                return Some(path);
            }
        }
        None
    }
    walk(home).expect("a pinned merge holds the cross-process lock, so merge.lock must exist")
}

/// Whether `pid` currently holds an open file descriptor on `lock_path` —
/// the pre-acquisition observable (`modules/io/src/merge_lock.rs:116-131`):
/// a waiter opens `merge.lock` before its first `try_lock_exclusive`, so
/// this going true proves the waiter is inside `acquire()` and about to (or
/// already having) fail its first try while the winner is still pinned.
fn process_holds_fd(pid: u32, lock_path: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        let canonical = fs::canonicalize(lock_path).unwrap_or_else(|_| lock_path.to_path_buf());
        let Ok(entries) = fs::read_dir(format!("/proc/{pid}/fd")) else {
            return false;
        };
        entries.flatten().any(|entry| {
            fs::read_link(entry.path())
                .map(|target| target == canonical)
                .unwrap_or(false)
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let output = Command::new("lsof").arg("-t").arg(lock_path).output();
        match output {
            Ok(output) => String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line.trim().parse::<u32>() == Ok(pid)),
            Err(_) => false,
        }
    }
}

/// Independent completed runs contend for the merge lock and converge without
/// starving either run. Their commits touch distinct paths, so serializing the
/// lock holders is sufficient for both public-path landings to succeed.
///
/// Contention here is coordination-driven, not scheduling luck: merge A is
/// pinned inside the lock by a blocking `[merge] gate` (observed via a
/// marker file), merge B's queued waiter is observed holding an open fd on
/// `merge.lock` before the gate is released, and only then is the lock
/// released — so B's first `try_lock_exclusive` is guaranteed to fail
/// while A is pinned, making `queued_behind`/`wait_ms` guaranteed, not
/// lucky (`modules/io/src/merge_lock.rs:105-194`).
#[test]
fn concurrent_disjoint_runs_eventually_land() {
    let scratch = ScratchRoot::new("p478-concurrent-convergence");
    let home = scratch.home();
    let repo = home.join("repo");
    let gate_entered = home.join("gate-entered");
    let release_gate = home.join("release-gate");
    init_fixture_repo_with_blocking_gate(&repo, &home, &gate_entered, &release_gate);
    install_counting_merger_with_frame_budget(&repo, &repo.join("concurrent-merger-calls"), 5);

    let mut run_ids = Vec::new();
    let mut session_paths = Vec::new();
    for name in ["a", "b"] {
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
            &home,
        );
        assert_exit_code(&output, 0);
        let run = value_json(&output);
        let worktree = Path::new(
            run["value"]["session"]["provenance"]["worktree"]["path"]
                .as_str()
                .unwrap(),
        );
        fs::write(
            worktree.join(format!("landing-{name}")),
            format!("{name}\n"),
        )
        .unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(worktree)
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-qm", &format!("landing {name}")])
            .current_dir(worktree)
            .status()
            .unwrap();
        run_ids.push(
            run["value"]["session"]["run-id"]
                .as_str()
                .unwrap()
                .to_string(),
        );
        session_paths.push(run["value"]["session-path"].as_str().unwrap().to_string());
    }
    let run_a = run_ids[0].clone();
    let run_b = run_ids[1].clone();
    let session_path_b = session_paths[1].clone();

    // 1. Spawn merge A and wait for it to reach its blocking gate — from
    // here A provably holds the merge lock and cannot release it until this
    // test creates `release_gate`.
    let handle_a = {
        let (repo, home, run_a) = (repo.clone(), home.clone(), run_a.clone());
        thread::spawn(move || run_ctx(&["traits", "merge", &run_a, "--json"], &repo, &home))
    };
    assert!(
        poll_until(Instant::now() + Duration::from_secs(60), || gate_entered
            .is_file()),
        "merge A must reach its blocking gate (and thus hold the merge lock) within the deadline"
    );

    // 2. Deterministic contending probe: zero timing assumptions — if lock
    // serialization ever regressed, this would land B early instead of
    // passing vacuously, and the assertions below would fail loudly.
    let probe = run_ctx(
        &["traits", "merge", &run_b, "--no-wait", "--json"],
        &repo,
        &home,
    );
    assert_exit_code(&probe, EXIT_MERGE_FAILED);
    let probe_report = value_json(&probe);
    assert_eq!(probe_report["value"]["status"], "lock-unavailable");
    assert!(
        probe_report["value"]["lock-holder"]
            .as_str()
            .unwrap_or_default()
            .contains(&format!("run-id={run_a}")),
        "the probe must report A as the current lock holder: {probe_report}"
    );

    // 3. Spawn merge B and observe its queued waiter holding an open fd on
    // `merge.lock` before A releases — proof it is inside `acquire()`, about
    // to fail its first `try_lock_exclusive` while A is still pinned.
    let child_b = spawn_ctx(&["traits", "merge", &run_b, "--json"], &repo, &home);
    let pid_b = child_b.id();
    let lock_path = find_merge_lock(&home);
    assert!(
        poll_until(Instant::now() + Duration::from_secs(60), || {
            process_holds_fd(pid_b, &lock_path)
        }),
        "merge B must be observed queued behind the merge lock within the deadline"
    );

    // 4. Release: A finishes and lands; B then acquires, its gate passes
    // instantly (release_gate already exists), and it lands in turn.
    fs::write(&release_gate, "").unwrap();
    assert_exit_code(&handle_a.join().unwrap(), 0);
    let output_b = child_b
        .wait_with_output()
        .unwrap_or_else(|error| panic!("cannot wait for merge B: {error}"));
    assert_exit_code(&output_b, 0);

    assert!(repo.join("landing-a").is_file());
    assert!(repo.join("landing-b").is_file());

    // The loser's frames record the wait: this is the assertion that fails
    // whenever the second process never actually waited on the lock.
    let ledger_b = fs::read_to_string(&session_path_b).unwrap();
    let lock_frame = ledger_b
        .split_once("\"stage\": \"lock\"")
        .expect("B's ledger must record a Lock frame")
        .1;
    assert!(
        lock_frame.contains("queued-behind=pid="),
        "B's Lock frame must record who it queued behind: {lock_frame}"
    );
    assert!(
        lock_frame.contains(&format!("run-id={run_a}")),
        "B's Lock frame must record A as the holder it queued behind: {lock_frame}"
    );
    let wait_ms: u64 = lock_frame
        .split("wait-ms=")
        .nth(1)
        .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|digits| digits.parse().ok())
        .unwrap_or_else(|| {
            panic!("B's Lock frame must carry a wait-ms= evidence value: {lock_frame}")
        });
    assert!(
        wait_ms > 0,
        "B must have actually waited on the merge lock (wait-ms=0 would mean it never contended): {lock_frame}"
    );
}

/// Git's `--ff-only` check is the safety boundary for owner work: unrelated
/// staged, unstaged, and untracked files must remain untouched while landing.
#[test]
fn disjoint_dirty_checkout_survives_a_successful_landing() {
    let scratch = ScratchRoot::new("p478-disjoint-dirty");
    let repo = scratch.home().join("repo");
    init_fixture_repo(&repo, &scratch.home(), "true", "true");
    fs::write(repo.join("owner-staged"), "staged owner work\n").unwrap();
    Command::new("git")
        .args(["add", "owner-staged"])
        .current_dir(&repo)
        .status()
        .unwrap();
    fs::write(repo.join("owner-unstaged"), "unstaged owner work\n").unwrap();
    fs::write(repo.join("owner-untracked"), "untracked owner work\n").unwrap();

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
    assert_eq!(value_json(&output)["value"]["merge"]["status"], "merged");
    assert_eq!(
        fs::read_to_string(repo.join("owner-staged")).unwrap(),
        "staged owner work\n"
    );
    assert_eq!(
        fs::read_to_string(repo.join("owner-unstaged")).unwrap(),
        "unstaged owner work\n"
    );
    assert_eq!(
        fs::read_to_string(repo.join("owner-untracked")).unwrap(),
        "untracked owner work\n"
    );
}

/// Persistent target movement consumes the configured bound, then writes one
/// terminal park with evidence for every real attempt rather than parking an
/// intermediate race.
#[test]
fn exhausted_target_races_record_all_attempts_in_one_terminal_park() {
    let scratch = ScratchRoot::new("p478-exhausted-races");
    let repo = scratch.home().join("repo");
    let repo_text = repo.to_string_lossy();
    init_fixture_repo_inner(&repo, &scratch.home(), "main", "true", |repo| {
        fs::write(
            repo.join(".ctx/config.toml"),
            format!(
                "[merge]\ngate = [[\"sh\", \"-c\", \"git -C '{repo_text}' commit --allow-empty -qm target-race\"]]\n"
            ),
        )
        .unwrap();
    });

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
        .expect("parked merge reports its session path");
    let ledger = fs::read_to_string(session_path).unwrap();
    let terminal = ledger
        .rsplit_once("\"stage\": \"landing\"")
        .expect("the final parked landing frame is present")
        .1;
    assert!(
        terminal.contains("merge-race-exhausted attempts=5/5"),
        "{terminal}"
    );
    assert_eq!(
        terminal
            .matches("merge-race-captured-target attempt=")
            .count(),
        5,
        "{terminal}"
    );
    assert_eq!(
        terminal
            .matches("merge-race-observed-target attempt=")
            .count(),
        5,
        "{terminal}"
    );
    assert_eq!(
        ledger.matches("\"status\": \"parked\"").count(),
        1,
        "{ledger}"
    );
}

/// `--no-wait` is intentionally an operator escape hatch, not a smaller retry
/// budget: a retryable target race parks after its one real landing attempt.
#[test]
fn no_wait_stops_after_one_retryable_attempt_without_exhaustion() {
    let scratch = ScratchRoot::new("p478-no-wait");
    let repo = scratch.home().join("repo");
    let repo_text = repo.to_string_lossy();
    let gate_count = repo.join("gate-count");
    let gate_count_text = gate_count.to_string_lossy();
    init_fixture_repo_inner(&repo, &scratch.home(), "main", "true", |repo| {
        fs::write(
            repo.join(".ctx/config.toml"),
            format!(
                "[merge]\nretry-attempts = 5\nretry-backoff-ms = 1\ngate = [[\"sh\", \"-c\", \"echo gate >> '{gate_count_text}'; git -C '{repo_text}' commit --allow-empty -qm target-race\"]]\n"
            ),
        )
        .unwrap();
    });

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
    let run_id = value_json(&run_output)["value"]["session"]["run-id"]
        .as_str()
        .expect("completed run reports its run id")
        .to_string();
    let output = run_ctx(
        &["traits", "merge", &run_id, "--no-wait", "--json"],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&output, EXIT_MERGE_PARKED);
    assert_eq!(
        fs::read_to_string(&gate_count).unwrap().lines().count(),
        1,
        "--no-wait must not make a second landing attempt"
    );
    let session_path = value_json(&run_output)["value"]["session-path"]
        .as_str()
        .expect("completed run reports its session path")
        .to_string();
    let ledger = fs::read_to_string(session_path).unwrap();
    assert!(
        !ledger.contains("merge-race-retry") && !ledger.contains("merge-race-exhausted"),
        "--no-wait must neither back off nor claim exhaustion: {ledger}"
    );
}

/// `git merge --ff-only` is the final atomic owner-work safety boundary. It
/// must refuse an overlapping checkout path regardless of Git's index state,
/// leaving the owner's bytes alone rather than stashing or touching them.
#[test]
fn overlapping_dirty_checkout_work_is_refused_without_mutation() {
    for mode in ["staged", "unstaged", "untracked"] {
        let scratch = ScratchRoot::new(&format!("p478-overlap-{mode}"));
        let repo = scratch.home().join("repo");
        init_fixture_repo(&repo, &scratch.home(), "true", "true");
        if mode != "untracked" {
            fs::write(repo.join("owner-overlap"), "base\n").unwrap();
            Command::new("git")
                .args(["add", "owner-overlap"])
                .current_dir(&repo)
                .status()
                .unwrap();
            Command::new("git")
                .args(["commit", "-qm", "owner base"])
                .current_dir(&repo)
                .status()
                .unwrap();
        }
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
            .expect("completed run reports its run id")
            .to_string();
        let worktree = Path::new(
            run["value"]["session"]["provenance"]["worktree"]["path"]
                .as_str()
                .expect("completed worktree run reports its checkout"),
        );
        fs::write(worktree.join("owner-overlap"), "landing change\n").unwrap();
        Command::new("git")
            .args(["add", "owner-overlap"])
            .current_dir(worktree)
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-qm", "landing overlap"])
            .current_dir(worktree)
            .status()
            .unwrap();
        let owner_contents = format!("owner {mode} work\n");
        fs::write(repo.join("owner-overlap"), &owner_contents).unwrap();
        if mode == "staged" {
            Command::new("git")
                .args(["add", "owner-overlap"])
                .current_dir(&repo)
                .status()
                .unwrap();
        }
        let output = run_ctx(
            &["traits", "merge", &run_id, "--json"],
            &repo,
            &scratch.home(),
        );
        assert_exit_code(&output, EXIT_MERGE_PARKED);
        assert_eq!(
            fs::read_to_string(repo.join("owner-overlap")).unwrap(),
            owner_contents,
            "{mode} owner work must survive an overlapping fast-forward refusal"
        );
    }
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
