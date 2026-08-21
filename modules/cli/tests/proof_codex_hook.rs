//! P501: `ctx traits internal hook --host codex` — the codex hook adapter rides
//! P499's handler unparameterized in substance; this proof asserts codex's
//! own event/source shapes (snake_case `hooks.json` event keys, no `fork`
//! source, the `UserPromptSubmit`/`SessionStart` wire shared with
//! claude-code) and the one genuinely new behavior the `--host` flag
//! introduces: harness namespace isolation in the ledger. Behavioral
//! assertions only (exit code + parsed JSON fields), no goldens or
//! byte-frozen fixtures (P461 doctrine, reaffirmed 2026-07-24).

use std::fs;
use std::path::Path;

use support::{
    ScratchRoot, git_init, ready_hook_fixture_trait, run_ctx_with_stdin, write_hook_fixture_trait,
};

const ALPHA_ID: &str = "hook-fixture-alpha";

fn ready_repo(scratch: &ScratchRoot) -> std::path::PathBuf {
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    write_hook_fixture_trait(
        &repo,
        ALPHA_ID,
        "Hook Fixture Alpha",
        "alpha",
        "Alpha fixture trait for the P501 codex hook proof suite.",
    );
    ready_hook_fixture_trait(&repo, &scratch.home(), ALPHA_ID);
    repo
}

fn user_prompt_submit_payload(session_id: &str, cwd: &Path, prompt: &str) -> String {
    serde_json::json!({
        "session_id": session_id,
        "transcript_path": "/tmp/does-not-exist.jsonl",
        "cwd": cwd.to_string_lossy(),
        "hook_event_name": "UserPromptSubmit",
        "permission_mode": "default",
        "turn_id": "turn-1",
        "agent_id": "agent-1",
        "agent_type": "codex",
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

fn run_hook(
    repo: &Path,
    home: &Path,
    host: &str,
    payload: &str,
) -> (i32, serde_json::Value, String) {
    let output = run_ctx_with_stdin(
        &["traits", "internal", "hook", "--host", host],
        repo,
        home,
        payload.as_bytes(),
    );
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

fn context_status(repo: &Path, home: &Path, host: &str, session_id: &str) -> serde_json::Value {
    let output = support::run_ctx(
        &[
            "traits",
            "internal",
            "context",
            "status",
            "--host",
            host,
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
fn codex_user_prompt_submit_injects_the_matching_trait() {
    let scratch = ScratchRoot::new("codex-hook-first-prompt");
    let repo = ready_repo(&scratch);
    let payload = user_prompt_submit_payload("s1", &repo, "please use the alpha fixture");

    let (code, value, _stderr) = run_hook(&repo, &scratch.home(), "codex", &payload);
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
}

#[test]
fn codex_second_identical_prompt_dedups_across_processes() {
    let scratch = ScratchRoot::new("codex-hook-dedup");
    let repo = ready_repo(&scratch);
    let payload = user_prompt_submit_payload("s1", &repo, "please use the alpha fixture");

    let (first_code, first_value, _) = run_hook(&repo, &scratch.home(), "codex", &payload);
    assert_eq!(first_code, 0);
    assert!(first_value.is_object());

    let (second_code, second_value, _) = run_hook(&repo, &scratch.home(), "codex", &payload);
    assert_eq!(second_code, 0);
    assert!(
        second_value.is_null(),
        "second identical prompt must produce no stdout: {second_value}"
    );
}

#[test]
fn codex_session_start_compact_restores_the_previously_injected_trait() {
    let scratch = ScratchRoot::new("codex-hook-compact-restore");
    let repo = ready_repo(&scratch);
    let home = scratch.home();

    let first = user_prompt_submit_payload("s1", &repo, "please use the alpha fixture");
    let (_, first_value, _) = run_hook(&repo, &home, "codex", &first);
    assert!(first_value.is_object());

    let compact = session_start_payload("s1", &repo, "compact");
    let (code, value, _) = run_hook(&repo, &home, "codex", &compact);
    assert_eq!(code, 0);
    assert_eq!(value["hookSpecificOutput"]["hookEventName"], "SessionStart");
    let context = value["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additionalContext is a string");
    assert!(
        context.contains("Alpha fixture trait"),
        "expected alpha restored after compact: {context}"
    );
}

#[test]
fn codex_session_start_resume_touches_nothing() {
    let scratch = ScratchRoot::new("codex-hook-resume");
    let repo = ready_repo(&scratch);
    let home = scratch.home();

    let first = user_prompt_submit_payload("s1", &repo, "please use the alpha fixture");
    run_hook(&repo, &home, "codex", &first);

    let resume = session_start_payload("s1", &repo, "resume");
    let (code, value, _) = run_hook(&repo, &home, "codex", &resume);
    assert_eq!(code, 0);
    assert!(value.is_null(), "resume must produce no output: {value}");

    let status = context_status(&repo, &home, "codex", "s1");
    assert_eq!(status["value"]["ledger-state"], "loaded");
    assert_eq!(
        status["value"]["entries"].as_array().unwrap().len(),
        1,
        "resume must not touch the ledger's entries"
    );
}

#[test]
fn codex_and_claude_code_never_dedup_against_each_other_under_the_same_session_id() {
    let scratch = ScratchRoot::new("codex-hook-namespace-isolation");
    let repo = ready_repo(&scratch);
    let home = scratch.home();
    let payload =
        user_prompt_submit_payload("shared-session", &repo, "please use the alpha fixture");

    let (claude_code, claude_value, _) = run_hook(&repo, &home, "claude-code", &payload);
    assert_eq!(claude_code, 0);
    assert!(
        claude_value.is_object(),
        "claude-code's first call must inject: {claude_value}"
    );

    // Same host_session, different --host: this must NOT be treated as a
    // dedup-against of the claude-code entry above (the one genuinely new
    // behavior a free-string flag would have silently broken).
    let (codex_code, codex_value, _) = run_hook(&repo, &home, "codex", &payload);
    assert_eq!(codex_code, 0);
    assert!(
        codex_value.is_object(),
        "codex must independently inject under its own host namespace: {codex_value}"
    );
}

#[test]
fn codex_settings_snippet_uses_snake_case_event_keys_and_notes_trust_hash_on_stderr() {
    let scratch = ScratchRoot::new("codex-hook-settings");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();

    let output = support::run_ctx(
        &[
            "traits",
            "internal",
            "hook",
            "--host",
            "codex",
            "--settings",
        ],
        &repo,
        &scratch.home(),
    );
    support::assert_exit_code(&output, 0);
    let (stdout, stderr) = support::utf8(&output);
    let value: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("stdout not JSON: {error}\n{stdout}"));
    assert!(value["hooks"]["session_start"].is_array());
    assert!(value["hooks"]["user_prompt_submit"].is_array());
    assert!(
        stderr.to_lowercase().contains("trust"),
        "codex settings must document the trust-hash re-arm on stderr: {stderr}"
    );
}

#[test]
fn codex_malformed_stdin_exits_zero() {
    let scratch = ScratchRoot::new("codex-hook-robustness");
    let repo = ready_repo(&scratch);
    let home = scratch.home();

    let (code, value, stderr) = run_hook(&repo, &home, "codex", "not json at all");
    assert_eq!(code, 0);
    assert!(value.is_null());
    assert!(
        !stderr.is_empty(),
        "malformed stdin should still log a diagnostic"
    );
}
