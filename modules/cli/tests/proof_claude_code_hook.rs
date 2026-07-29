//! P499: `ctx traits hook` — the claude-code hook adapter's stdin→stdout
//! wire contract, dedup-through-the-ledger behavior, the `SessionStart`
//! `compact`/`resume`/`startup` paths, the 10,000-char cap, and the
//! never-fail-the-session exit-code contract. Behavioral assertions only
//! (exit code + parsed JSON fields), no goldens or byte-frozen fixtures
//! (P461 doctrine, reaffirmed 2026-07-24).

use std::fs;
use std::path::{Path, PathBuf};

use support::{
    ScratchRoot, git_init, ready_hook_fixture_trait, run_ctx_with_stdin, write_hook_fixture_trait,
};

const ALPHA_ID: &str = "hook-fixture-alpha";
const BETA_ID: &str = "hook-fixture-beta";
const BIG_ID: &str = "hook-fixture-big";

fn write_trait(repo: &Path, id: &str, name: &str, keyword: &str, summary: &str) {
    write_hook_fixture_trait(repo, id, name, keyword, summary);
}

fn ready(repo: &Path, home: &Path, id: &str) {
    ready_hook_fixture_trait(repo, home, id);
}

/// A fresh Git repository with two small keyword-matched traits
/// (`hook-fixture-alpha` on "alpha", `hook-fixture-beta` on "beta"), both
/// `ready`/`verified` so the hook's render-trust gate never refuses.
fn ready_repo(scratch: &ScratchRoot) -> PathBuf {
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    write_trait(
        &repo,
        ALPHA_ID,
        "Hook Fixture Alpha",
        "alpha",
        "Alpha fixture trait for the P499 hook proof suite.",
    );
    write_trait(
        &repo,
        BETA_ID,
        "Hook Fixture Beta",
        "beta",
        "Beta fixture trait for the P499 hook proof suite.",
    );
    ready(&repo, &scratch.home(), ALPHA_ID);
    ready(&repo, &scratch.home(), BETA_ID);
    repo
}

fn user_prompt_submit_payload(session_id: &str, cwd: &Path, prompt: &str) -> String {
    serde_json::json!({
        "session_id": session_id,
        "transcript_path": "/tmp/does-not-exist.jsonl",
        "cwd": cwd.to_string_lossy(),
        "prompt_id": "prompt-1",
        "permission_mode": "default",
        "hook_event_name": "UserPromptSubmit",
        "prompt": prompt,
    })
    .to_string()
}

fn session_start_payload(session_id: &str, cwd: &Path, source: &str) -> String {
    serde_json::json!({
        "session_id": session_id,
        "transcript_path": "/tmp/does-not-exist.jsonl",
        "cwd": cwd.to_string_lossy(),
        "hook_event_name": "SessionStart",
        "source": source,
    })
    .to_string()
}

fn run_hook(repo: &Path, home: &Path, payload: &str) -> (i32, serde_json::Value, String) {
    let output = run_ctx_with_stdin(&["traits", "hook"], repo, home, payload.as_bytes());
    let (stdout, stderr) = support::utf8(&output);
    let code = output.status.code().expect("exit code");
    let value = if stdout.trim().is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&stdout)
            .unwrap_or_else(|error| panic!("hook stdout not JSON: {error}\n{stdout}"))
    };
    (code, value, stderr)
}

fn context_status(repo: &Path, home: &Path, session_id: &str) -> serde_json::Value {
    let output = support::run_ctx(
        &[
            "traits",
            "context",
            "status",
            "--host",
            "claude-code",
            "--host-session",
            session_id,
            "--json",
        ],
        repo,
        home,
    );
    let (stdout, _) = support::utf8(&output);
    serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("status not JSON: {error}\n{stdout}"))
}

#[test]
fn first_user_prompt_submit_injects_the_matching_trait() {
    let scratch = ScratchRoot::new("hook-first-prompt");
    let repo = ready_repo(&scratch);
    let payload = user_prompt_submit_payload("s1", &repo, "please use the alpha fixture");

    let (code, value, _stderr) = run_hook(&repo, &scratch.home(), &payload);
    assert_eq!(code, 0);
    assert_eq!(
        value["hookSpecificOutput"]["hookEventName"],
        "UserPromptSubmit"
    );
    let context = value["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additionalContext is a string");
    assert!(
        context.contains("Alpha fixture trait"),
        "expected alpha's rendered text in additionalContext: {context}"
    );
    assert!(
        !context.contains("Beta fixture trait"),
        "beta was not matched and must not appear: {context}"
    );
}

#[test]
fn second_identical_prompt_dedups_across_processes() {
    let scratch = ScratchRoot::new("hook-dedup");
    let repo = ready_repo(&scratch);
    let payload = user_prompt_submit_payload("s1", &repo, "please use the alpha fixture");

    let (first_code, first_value, _) = run_hook(&repo, &scratch.home(), &payload);
    assert_eq!(first_code, 0);
    assert!(first_value.is_object());

    let (second_code, second_value, _) = run_hook(&repo, &scratch.home(), &payload);
    assert_eq!(second_code, 0);
    assert!(
        second_value.is_null(),
        "second identical prompt must produce no stdout: {second_value}"
    );
}

#[test]
fn a_second_matching_trait_only_shows_its_own_text() {
    let scratch = ScratchRoot::new("hook-second-trait");
    let repo = ready_repo(&scratch);
    let home = scratch.home();

    let first = user_prompt_submit_payload("s1", &repo, "please use the alpha fixture");
    run_hook(&repo, &home, &first);

    let second = user_prompt_submit_payload("s1", &repo, "now use the beta fixture too");
    let (code, value, _) = run_hook(&repo, &home, &second);
    assert_eq!(code, 0);
    let context = value["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additionalContext is a string");
    assert!(
        context.contains("Beta fixture trait"),
        "expected beta's rendered text: {context}"
    );
    assert!(
        !context.contains("Alpha fixture trait"),
        "alpha is already fresh and must not be re-emitted: {context}"
    );
}

#[test]
fn session_start_compact_restores_the_previously_injected_trait() {
    let scratch = ScratchRoot::new("hook-compact-restore");
    let repo = ready_repo(&scratch);
    let home = scratch.home();

    let first = user_prompt_submit_payload("s1", &repo, "please use the alpha fixture");
    let (_, first_value, _) = run_hook(&repo, &home, &first);
    assert!(first_value.is_object());

    let compact = session_start_payload("s1", &repo, "compact");
    let (code, value, _) = run_hook(&repo, &home, &compact);
    assert_eq!(code, 0);
    assert_eq!(value["hookSpecificOutput"]["hookEventName"], "SessionStart");
    let context = value["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additionalContext is a string");
    assert!(
        context.contains("Alpha fixture trait"),
        "expected alpha restored after compact: {context}"
    );

    // Re-committed as fresh: the very next identical prompt is a skip.
    let (after_code, after_value, _) = run_hook(&repo, &home, &first);
    assert_eq!(after_code, 0);
    assert!(
        after_value.is_null(),
        "alpha should be fresh again immediately after the compact restore: {after_value}"
    );
}

#[test]
fn session_start_resume_touches_nothing() {
    let scratch = ScratchRoot::new("hook-resume");
    let repo = ready_repo(&scratch);
    let home = scratch.home();

    let first = user_prompt_submit_payload("s1", &repo, "please use the alpha fixture");
    run_hook(&repo, &home, &first);

    let resume = session_start_payload("s1", &repo, "resume");
    let (code, value, _) = run_hook(&repo, &home, &resume);
    assert_eq!(code, 0);
    assert!(value.is_null(), "resume must produce no output: {value}");

    let status = context_status(&repo, &home, "s1");
    assert_eq!(status["value"]["ledger-state"], "loaded");
    assert_eq!(
        status["value"]["entries"].as_array().unwrap().len(),
        1,
        "resume must not touch the ledger's entries"
    );
}

#[test]
fn session_start_startup_clears_the_ledger_and_injects_nothing() {
    let scratch = ScratchRoot::new("hook-startup");
    let repo = ready_repo(&scratch);
    let home = scratch.home();

    let first = user_prompt_submit_payload("s1", &repo, "please use the alpha fixture");
    run_hook(&repo, &home, &first);

    let startup = session_start_payload("s1", &repo, "startup");
    let (code, value, _) = run_hook(&repo, &home, &startup);
    assert_eq!(code, 0);
    assert!(
        value.is_null(),
        "startup with no task text injects nothing: {value}"
    );

    let status = context_status(&repo, &home, "s1");
    assert_eq!(status["value"]["ledger-state"], "missing");
    assert_eq!(status["value"]["last-cleared-reason"], "startup");
}

#[test]
fn oversized_trait_is_omitted_under_the_cap_and_never_committed() {
    let scratch = ScratchRoot::new("hook-cap");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    // Long, but not a contiguous base64-looking run: `sanitize_model_text`
    // collapses any >80-char run of base64 characters, so plain repetition
    // of one letter would get sanitized away before the cap logic ever sees
    // it. Spaces every few words keep it under that sanitizer's radar.
    let big_summary = "this is filler text for the P499 cap proof suite. ".repeat(240);
    write_trait(&repo, BIG_ID, "Hook Fixture Big", "big", &big_summary);
    ready(&repo, &scratch.home(), BIG_ID);

    let payload = user_prompt_submit_payload("s1", &repo, "please use the big fixture");
    let (code, value, stderr) = run_hook(&repo, &scratch.home(), &payload);
    assert_eq!(code, 0);
    assert!(
        value.is_null(),
        "an over-cap trait alone must leave nothing to emit: {value}"
    );
    assert!(
        stderr.contains(BIG_ID) && stderr.contains("cap"),
        "omission must be named on stderr: {stderr}"
    );

    // Never committed: the ledger stays empty, and a second identical
    // prompt still names the trait as omitted rather than silently no
    // longer mentioning it (which is what would happen if it had wrongly
    // been marked fresh despite never being emitted).
    let status = context_status(&repo, &scratch.home(), "s1");
    assert_eq!(status["value"]["ledger-state"], "missing");

    let (second_code, second_value, second_stderr) = run_hook(&repo, &scratch.home(), &payload);
    assert_eq!(second_code, 0);
    assert!(second_value.is_null());
    assert!(
        second_stderr.contains(BIG_ID) && second_stderr.contains("cap"),
        "an uncommitted over-cap trait must still be reported (proves no-commit): {second_stderr}"
    );
}

#[test]
fn malformed_stdin_and_unknown_event_and_traitless_cwd_all_exit_zero_with_empty_stdout() {
    let scratch = ScratchRoot::new("hook-robustness");
    let repo = ready_repo(&scratch);
    let home = scratch.home();

    let (code, value, stderr) = run_hook(&repo, &home, "not json at all");
    assert_eq!(code, 0);
    assert!(value.is_null());
    assert!(
        !stderr.is_empty(),
        "malformed stdin should still log a diagnostic"
    );

    let unknown = serde_json::json!({
        "session_id": "s1",
        "cwd": repo.to_string_lossy(),
        "hook_event_name": "SomeFutureEvent",
    })
    .to_string();
    let (code, value, _) = run_hook(&repo, &home, &unknown);
    assert_eq!(code, 0);
    assert!(value.is_null());

    let empty_repo = scratch.home().join("empty-repo");
    fs::create_dir_all(&empty_repo).unwrap();
    git_init(&empty_repo);
    let payload = user_prompt_submit_payload("s2", &empty_repo, "nothing matches here");
    let (code, value, _) = run_hook(&empty_repo, &home, &payload);
    assert_eq!(code, 0);
    assert!(value.is_null());
}

#[test]
fn settings_snippet_is_json_and_names_both_events() {
    let scratch = ScratchRoot::new("hook-settings");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();

    let output = support::run_ctx(&["traits", "hook", "--settings"], &repo, &scratch.home());
    support::assert_exit_code(&output, 0);
    let (stdout, _) = support::utf8(&output);
    let value: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|error| panic!("not JSON: {error}\n{stdout}"));
    assert!(value["hooks"]["UserPromptSubmit"].is_array());
    assert!(value["hooks"]["SessionStart"].is_array());
}
