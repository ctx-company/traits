//! Task 0273 proofs: the basic implement canonical narrates to ctx-notify,
//! and every shipped notify script degrades to zero-exit markers under
//! notifier failure. Structural facts are read from the regenerated basic
//! canonical (counts, order, argv properties — never a whole-file golden),
//! and behavioral facts execute the exact shipped scripts extracted from
//! that canonical under a scratch-only PATH whose fake `ctx-notify` is the
//! only notifier reachable, so no test can touch a real daemon.

use std::process::Command;

use support::{ScratchRoot, repo_root};

fn basic_canonical() -> toml::Table {
    let path = repo_root().join(".ctx/traits/authored/implement/generated/basic/index.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    text.parse::<toml::Table>()
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn ids(items: &[toml::Value]) -> Vec<&str> {
    items
        .iter()
        .map(|item| {
            item.get("id")
                .and_then(toml::Value::as_str)
                .expect("sequence item id")
        })
        .collect()
}

fn index_of(ids: &[&str], wanted: &str) -> usize {
    ids.iter()
        .position(|id| *id == wanted)
        .unwrap_or_else(|| panic!("id {wanted} not found in {ids:?}"))
}

/// The embedded sh script of a command step, by id, searched across the
/// top-level procedure sequence and every named sub-sequence.
fn script_of(canonical: &toml::Table, wanted: &str) -> String {
    let mut pools: Vec<&Vec<toml::Value>> = Vec::new();
    let top = canonical
        .get("procedure")
        .and_then(|p| p.get("sequence"))
        .and_then(toml::Value::as_array)
        .expect("procedure.sequence");
    pools.push(top);
    if let Some(named) = canonical.get("sequence").and_then(toml::Value::as_table) {
        for sub in named.values() {
            if let Some(items) = sub.get("sequence").and_then(toml::Value::as_array) {
                pools.push(items);
            }
        }
    }
    for pool in pools {
        for item in pool {
            if item.get("id").and_then(toml::Value::as_str) == Some(wanted) {
                let argv = item
                    .get("command")
                    .and_then(|c| c.get("argv"))
                    .and_then(toml::Value::as_array)
                    .unwrap_or_else(|| panic!("step {wanted} has no command argv"));
                assert_eq!(argv[0].as_str(), Some("sh"), "step {wanted} argv[0]");
                assert_eq!(argv[1].as_str(), Some("-c"), "step {wanted} argv[1]");
                return argv[2].as_str().expect("script text").to_string();
            }
        }
    }
    panic!("step {wanted} not found in any sequence pool");
}

#[test]
fn basic_canonical_carries_the_narration_in_contract_order() {
    let canonical = basic_canonical();
    let top = canonical
        .get("procedure")
        .and_then(|p| p.get("sequence"))
        .and_then(toml::Value::as_array)
        .expect("procedure.sequence");
    let top_ids = ids(top);

    // begin is the first step of the run, before the session baseline.
    assert_eq!(
        top_ids[0], "notify-begin",
        "begin opens the procedure: {top_ids:?}"
    );
    let baseline = index_of(&top_ids, "capture-the-session-base");
    assert_eq!(top_ids[baseline + 1], "notify-session-base");
    let draft = index_of(&top_ids, "draft-the-implementation-plan");
    assert_eq!(top_ids[draft + 1], "notify-plan-drafted");
    // finish comes after the Maybe Commit branch so a clean tree still closes.
    let finish = index_of(&top_ids, "notify-finish");
    let maybe_commit = index_of(&top_ids, "maybe-commit");
    assert!(
        finish > maybe_commit,
        "finish after Maybe Commit: {top_ids:?}"
    );
    assert_eq!(
        finish,
        top_ids.len() - 1,
        "finish is the last step: {top_ids:?}"
    );

    // Loop body: update after implement, project+review update after review,
    // all before the owner-ruling branch.
    let body = canonical
        .get("sequence")
        .and_then(|s| s.get("reviewed-refinement-body"))
        .and_then(|s| s.get("sequence"))
        .and_then(toml::Value::as_array)
        .expect("reviewed-refinement-body.sequence");
    let body_ids = ids(body);
    let implement = index_of(&body_ids, "implement-the-task");
    assert_eq!(body_ids[implement + 1], "notify-implement-pass");
    let review = index_of(&body_ids, "review-the-implementation");
    assert_eq!(body_ids[review + 1], "notify-project-verdict");
    assert_eq!(body_ids[review + 2], "notify-review-update");
    let ruling = index_of(&body_ids, "owner-ruling");
    assert!(
        ruling > review + 2,
        "review narration precedes the ruling branch"
    );

    // Commit arm: the committed update follows the commit submit step.
    let named = canonical
        .get("sequence")
        .and_then(toml::Value::as_table)
        .expect("named sequences");
    let commit_arm = named
        .values()
        .filter_map(|sub| sub.get("sequence").and_then(toml::Value::as_array))
        .find(|items| ids(items).contains(&"commit-the-work"))
        .expect("an arm containing commit-the-work");
    let arm_ids = ids(commit_arm);
    let submit = index_of(&arm_ids, "commit-the-work");
    assert_eq!(arm_ids[submit + 1], "notify-committed");

    // Argv constraints (goal 3): begin is --json without --quiet; updates and
    // finish are --quiet; the review script sends one combined status+summary
    // update and at most one log invocation; the 200-char cap is present.
    let begin = script_of(&canonical, "notify-begin");
    assert!(begin.contains(r#""begin", "--json""#), "begin uses --json");
    assert!(!begin.contains("--quiet"), "begin never combines --quiet");
    for update_id in [
        "notify-session-base",
        "notify-plan-drafted",
        "notify-implement-pass",
        "notify-committed",
    ] {
        let script = script_of(&canonical, update_id);
        assert!(
            script.contains(r#""--quiet", "--status""#),
            "{update_id} is a quiet status update"
        );
        assert!(
            !script.contains(r#""log""#),
            "{update_id} appends no journal lines"
        );
    }
    let review_script = script_of(&canonical, "notify-review-update");
    assert!(review_script.contains(r#""--quiet", "--status", badge, "--summary", summary"#));
    assert_eq!(
        review_script.matches(r#""ctx-notify", "log""#).count(),
        1,
        "exactly one log invocation carries every open line"
    );
    assert!(
        review_script.contains("[:200]"),
        "open-point text is capped at 200 chars"
    );
    assert!(
        review_script.contains("time.monotonic() + 60.0"),
        "overall deadline present"
    );
    let finish_script = script_of(&canonical, "notify-finish");
    assert!(
        finish_script.contains(r#""finish", "--ok", "--quiet""#),
        "finish is --ok --quiet"
    );
}

#[test]
fn quick_and_complex_canonicals_stay_silent() {
    for variant in ["quick", "complex"] {
        let path = repo_root().join(format!(
            ".ctx/traits/authored/implement/generated/{variant}/index.toml"
        ));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        assert!(
            !text.contains("ctx-notify"),
            "{variant} canonical must carry no notifier invocation"
        );
    }
}

struct Fixture {
    scratch: ScratchRoot,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let scratch = ScratchRoot::new(label);
        std::fs::create_dir(scratch.path().join("bin")).expect("scratch bin");
        Self { scratch }
    }

    fn install_fake(&self, body: &str) {
        let path = self.scratch.path().join("bin/ctx-notify");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fake");
        let mut perms = std::fs::metadata(&path).expect("stat fake").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&path, perms).expect("chmod fake");
    }

    fn record_path(&self) -> std::path::PathBuf {
        self.scratch.path().join("record.log")
    }

    fn recorded(&self) -> Vec<String> {
        std::fs::read_to_string(self.record_path())
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Run one shipped script exactly as the canonical ships it: sh -c with
    /// positional argv, under a PATH of the scratch bin plus the system
    /// directories only — the real notifier's install dirs are unreachable.
    fn run(&self, script: &str, args: &[&str]) -> (String, bool) {
        let mut command = Command::new("sh");
        command.arg("-c").arg(script).arg("_").args(args);
        command.env(
            "PATH",
            format!(
                "{}:/usr/bin:/bin",
                self.scratch.path().join("bin").display()
            ),
        );
        command.env("RECORD", self.record_path());
        let output = command.output().expect("run shipped script");
        (
            String::from_utf8_lossy(&output.stdout).to_string(),
            output.status.success(),
        )
    }
}

fn script(step: &str) -> String {
    script_of(&basic_canonical(), step)
}

const RECORDING: &str = r#"printf '%s\n' "$*" >> "$RECORD""#;

#[test]
fn begin_extracts_the_activity_id_and_degrades_on_every_failure() {
    let begin = script("notify-begin");

    let good = Fixture::new("notify-begin-good");
    good.install_fake(&format!(
        "{RECORDING}\nprintf '{{\"activity_id\":\"act-42\"}}'"
    ));
    let (out, ok) = good.run(&begin, &["0273"]);
    assert!(ok);
    assert_eq!(out, "act-42");
    assert!(good.recorded()[0].contains("begin --json implement: 0273"));

    let garbage = Fixture::new("notify-begin-garbage");
    garbage.install_fake("printf 'not json at all'");
    let (out, ok) = garbage.run(&begin, &["0273"]);
    assert!(ok);
    assert_eq!(out, "unavailable", "malformed begin JSON degrades");

    let nonzero = Fixture::new("notify-begin-nonzero");
    nonzero.install_fake("printf '{\"activity_id\":\"act-42\"}'\nexit 3");
    let (out, ok) = nonzero.run(&begin, &["0273"]);
    assert!(ok);
    assert_eq!(
        out, "unavailable",
        "nonzero begin is rejected even with valid JSON"
    );

    let missing = Fixture::new("notify-begin-missing");
    let (out, ok) = missing.run(&begin, &["0273"]);
    assert!(ok);
    assert_eq!(out, "unavailable", "missing notifier binary degrades");
}

#[test]
fn begin_times_out_a_hung_notifier_and_still_exits_zero() {
    let begin = script("notify-begin");
    let hung = Fixture::new("notify-begin-hung");
    hung.install_fake("sleep 30");
    let started = std::time::Instant::now();
    let (out, ok) = hung.run(&begin, &["0273"]);
    assert!(ok);
    assert_eq!(out, "unavailable", "child timeout degrades");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(25),
        "the caught 15s child timeout bounds the wait"
    );
}

#[test]
fn update_short_circuits_marks_and_inspects_returncodes() {
    let update = script("notify-session-base");

    let unavailable = Fixture::new("notify-update-unavailable");
    let (out, ok) = unavailable.run(&update, &["unavailable", "Capture the session base"]);
    assert!(ok);
    assert_eq!(out, "skipped: no activity");
    assert!(
        unavailable.recorded().is_empty(),
        "short-circuit calls nothing"
    );

    let good = Fixture::new("notify-update-good");
    good.install_fake(RECORDING);
    let (out, ok) = good.run(&update, &["act-42", "Capture the session base"]);
    assert!(ok);
    assert_eq!(out, "ok: Capture the session base");
    assert!(good.recorded()[0].contains("update act-42 --quiet --status Capture the session base"));

    let failing = Fixture::new("notify-update-failing");
    failing.install_fake(&format!("{RECORDING}\nexit 3"));
    let (out, ok) = failing.run(&update, &["act-42", "Capture the session base"]);
    assert!(ok);
    assert_eq!(
        out, "skipped: Capture the session base",
        "nonzero exit is skipped, never ok"
    );
}

#[test]
fn review_update_sends_one_combined_update_and_one_log_call() {
    let review = script("notify-review-update");
    let points = r#"[{"steps":[{"step":"first open point","status":"open"},{"step":"already done","status":"done"},{"step":"second open point","status":"open"}]}]"#;

    let revise = Fixture::new("notify-review-revise");
    revise.install_fake(RECORDING);
    let (out, ok) = revise.run(&review, &["act-42", "revise", points]);
    assert!(ok);
    assert_eq!(out, "update:ok log:ok:2");
    let calls = revise.recorded();
    assert_eq!(
        calls.len(),
        2,
        "one combined update then one log call: {calls:?}"
    );
    assert!(
        calls[0].contains("update act-42 --quiet --status review: revise --summary 2 open points")
    );
    assert!(calls[1].contains("log act-42 first open point second open point"));

    let approved = Fixture::new("notify-review-approved");
    approved.install_fake(RECORDING);
    let (out, ok) = approved.run(&review, &["act-42", "approved", "[]"]);
    assert!(ok);
    assert_eq!(out, "update:ok", "approved sends the status only");
    let calls = approved.recorded();
    assert_eq!(calls.len(), 1, "no log call without open points: {calls:?}");
    assert!(
        calls[0].contains("--summary ") || calls[0].ends_with("--summary"),
        "approved clears the summary: {calls:?}"
    );

    let unavailable = Fixture::new("notify-review-unavailable");
    let (out, ok) = unavailable.run(&review, &["unavailable", "revise", points]);
    assert!(ok);
    assert_eq!(out, "skipped: no activity");

    let failing = Fixture::new("notify-review-failing");
    failing.install_fake(&format!("{RECORDING}\nexit 3"));
    let (out, ok) = failing.run(&review, &["act-42", "revise", points]);
    assert!(ok);
    assert_eq!(
        out, "update:skipped log:skipped:2",
        "nonzero children degrade per call"
    );
}

#[test]
fn finish_closes_ok_and_degrades_like_the_rest() {
    let finish = script("notify-finish");

    let good = Fixture::new("notify-finish-good");
    good.install_fake(RECORDING);
    let (out, ok) = good.run(&finish, &["act-42"]);
    assert!(ok);
    assert_eq!(out, "finish: ok");
    assert!(good.recorded()[0].contains("finish --ok --quiet act-42"));

    let unavailable = Fixture::new("notify-finish-unavailable");
    let (out, ok) = unavailable.run(&finish, &["unavailable"]);
    assert!(ok);
    assert_eq!(out, "skipped: no activity");

    let failing = Fixture::new("notify-finish-failing");
    failing.install_fake("exit 3");
    let (out, ok) = failing.run(&finish, &["act-42"]);
    assert!(ok);
    assert_eq!(out, "finish: skipped");
}
