//! P498: `ctx traits context plan/clear/status` — the session context
//! ledger's dedup, staleness, host-key isolation, and rejection behavior.
//! Behavioral assertions only (exit code + parsed JSON fields), no goldens
//! or byte-frozen fixtures (P461 doctrine).

use std::fs;
use std::path::{Path, PathBuf};

use support::{ScratchRoot, git_init, require_success, run_ctx};

const TRAIT_ID: &str = "context-ledger-fixture";

fn manifest(summary: &str) -> String {
    format!(
        "id = \"{TRAIT_ID}\"\n\
schema-version = \"0.2\"\n\
version = \"0.1.0\"\n\
name = \"Context Ledger Fixture\"\n\
summary = {summary:?}\n\
\n\
[activation]\n\
\n\
[[activation.rule]]\n\
id = \"always\"\n\
reason = \"matches the fixture task\"\n\
task-keyword = \"fixture\"\n"
    )
}

fn write_fixture_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("cannot create {}: {error}", parent.display()));
    }
    fs::write(path, contents)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
}

/// A fresh Git repository with one `ready`/`verified` trait, so `context
/// plan`'s render (which reuses `prompt`'s render-trust gate) never refuses.
fn ready_repo(scratch: &ScratchRoot) -> PathBuf {
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    write_fixture_file(
        &repo.join(".ctx/traits/authored/context-ledger-fixture/trait.toml"),
        &format!(
            "[package]\nid = {TRAIT_ID:?}\nversion = \"0.1.0\"\nname = \"Context Ledger Fixture\"\nstatus = \"draft\"\n"
        ),
    );
    write_fixture_file(
        &repo.join(".ctx/traits/authored/context-ledger-fixture/generated/index.toml"),
        &manifest("P498 context-ledger fixture."),
    );
    require_success(
        "`ctx traits activate` clears the draft gate",
        &["traits", "activate", TRAIT_ID],
        &repo,
        &scratch.home(),
    );
    require_success(
        "`ctx traits trust approve` clears the unreviewed gate",
        &["traits", "trust", "approve", TRAIT_ID],
        &repo,
        &scratch.home(),
    );
    repo
}

fn edit_trait_summary(repo: &Path, summary: &str) {
    write_fixture_file(
        &repo.join(".ctx/traits/authored/context-ledger-fixture/generated/index.toml"),
        &manifest(summary),
    );
}

fn plan_json(
    repo: &Path,
    home: &Path,
    host: &str,
    host_session: &str,
    commit: bool,
) -> serde_json::Value {
    let mut args = vec![
        "traits",
        "context",
        "plan",
        "--host",
        host,
        "--host-session",
        host_session,
        "--task",
        "use the fixture trait",
        "--json",
    ];
    if commit {
        args.push("--commit");
    }
    let output = run_ctx(&args, repo, home);
    let (stdout, stderr) = support::utf8(&output);
    assert!(
        output.status.success(),
        "context plan should succeed: stdout={stdout} stderr={stderr}"
    );
    serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("plan output not JSON: {error}\n{stdout}"))
}

fn row_action<'a>(plan: &'a serde_json::Value, trait_id: &str) -> &'a str {
    plan["value"]["rows"]
        .as_array()
        .expect("rows array")
        .iter()
        .find(|row| row["trait-id"] == trait_id)
        .unwrap_or_else(|| panic!("no row for {trait_id} in {plan}"))["action"]
        .as_str()
        .expect("action is a string")
}

/// Locate every persisted context-ledger file under the scratch `HOME`
/// (`$XDG_CONFIG_HOME/ctx/context/<repo-key>/<harness>-<host-session>.json`)
/// without recomputing the repo-key hash ourselves.
fn find_ledger_files(home: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let root = home.join("ctx").join("context");
    if !root.is_dir() {
        return found;
    }
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("read_dir {dir:?}: {e}")) {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "json") {
                found.push(path);
            }
        }
    }
    found
}

#[test]
fn commit_then_replan_is_inject_then_skip_fresh() {
    let scratch = ScratchRoot::new("ledger-inject-then-skip");
    let repo = ready_repo(&scratch);

    let first = plan_json(&repo, &scratch.home(), "claude-code", "session-1", true);
    assert_eq!(row_action(&first, TRAIT_ID), "inject");

    let second = plan_json(&repo, &scratch.home(), "claude-code", "session-1", true);
    assert_eq!(row_action(&second, TRAIT_ID), "skip-fresh");
}

#[test]
fn clear_then_replan_injects_again() {
    let scratch = ScratchRoot::new("ledger-clear-then-inject");
    let repo = ready_repo(&scratch);

    let first = plan_json(&repo, &scratch.home(), "claude-code", "session-1", true);
    assert_eq!(row_action(&first, TRAIT_ID), "inject");

    let clear_output = run_ctx(
        &[
            "traits",
            "context",
            "clear",
            "--host",
            "claude-code",
            "--host-session",
            "session-1",
            "--reason",
            "compact",
            "--json",
        ],
        &repo,
        &scratch.home(),
    );
    assert!(
        clear_output.status.success(),
        "context clear should succeed: {:?}",
        support::utf8(&clear_output)
    );
    let (clear_stdout, _) = support::utf8(&clear_output);
    let clear_json: serde_json::Value = serde_json::from_str(&clear_stdout).unwrap();
    assert_eq!(clear_json["value"]["cleared-entry-count"], 1);

    let after_clear = plan_json(&repo, &scratch.home(), "claude-code", "session-1", true);
    assert_eq!(row_action(&after_clear, TRAIT_ID), "inject");
}

#[test]
fn editing_the_trait_between_commits_reinjects_with_digest_changed() {
    let scratch = ScratchRoot::new("ledger-digest-changed");
    let repo = ready_repo(&scratch);

    let first = plan_json(&repo, &scratch.home(), "claude-code", "session-1", true);
    assert_eq!(row_action(&first, TRAIT_ID), "inject");

    // Edit `summary`, not `name`: the render v2 behavior envelope carries no
    // `<name>` tag (rule 2 — name reaches humans through export frontmatter
    // only), so only a behavior-bearing field edit moves the ledger's digest.
    edit_trait_summary(&repo, "P498 context-ledger fixture (edited).");
    // Editing the trait changes its canonical digest, which resets trust to
    // unreviewed for the new digest (package status is unaffected — it lives
    // in `trait.toml`, not the edited generated document); re-approve so
    // only the ledger's own digest-changed staleness is under test here.
    require_success(
        "`ctx traits trust approve` re-clears the unreviewed gate after edit",
        &["traits", "trust", "approve", TRAIT_ID],
        &repo,
        &scratch.home(),
    );

    let second = plan_json(&repo, &scratch.home(), "claude-code", "session-1", true);
    assert_eq!(row_action(&second, TRAIT_ID), "reinject");
    let row = second["value"]["rows"][0].clone();
    assert_eq!(row["stale-reason"], "digest-changed");
}

#[test]
fn independent_host_sessions_and_harnesses_never_share_entries() {
    let scratch = ScratchRoot::new("ledger-host-key-isolation");
    let repo = ready_repo(&scratch);

    let session_a = plan_json(&repo, &scratch.home(), "claude-code", "session-a", true);
    assert_eq!(row_action(&session_a, TRAIT_ID), "inject");

    // A different host-session under the same harness sees no entry.
    let session_b = plan_json(&repo, &scratch.home(), "claude-code", "session-b", true);
    assert_eq!(row_action(&session_b, TRAIT_ID), "inject");

    // The same session id under a different harness also sees no entry.
    let other_harness = plan_json(&repo, &scratch.home(), "opencode", "session-a", true);
    assert_eq!(row_action(&other_harness, TRAIT_ID), "inject");

    // Re-querying the original host key still shows skip-fresh: the other
    // two commits never touched its entries.
    let session_a_again = plan_json(&repo, &scratch.home(), "claude-code", "session-a", false);
    assert_eq!(row_action(&session_a_again, TRAIT_ID), "skip-fresh");
}

/// P498 (c) regression: a ledger file whose persisted entry `host-key`
/// disagrees with the requesting host key must fail closed to `reinject`,
/// never `skip-fresh` — the pre-fix `reconcile` tested absence only, so a
/// *wrong* host key reconciled clean.
#[test]
fn entry_with_mismatched_stored_host_key_fails_closed_to_reinject() {
    let scratch = ScratchRoot::new("ledger-wrong-host-key-fails-closed");
    let repo = ready_repo(&scratch);

    let first = plan_json(&repo, &scratch.home(), "claude-code", "session-1", true);
    assert_eq!(row_action(&first, TRAIT_ID), "inject");

    let ledger_files = find_ledger_files(&scratch.home());
    assert_eq!(
        ledger_files.len(),
        1,
        "expected exactly one persisted ledger file: {ledger_files:?}"
    );
    let ledger_path = &ledger_files[0];
    let text = fs::read_to_string(ledger_path).unwrap();
    let tampered = text.replace("claude-code:session-1", "claude-code:some-other-session");
    assert_ne!(
        text, tampered,
        "expected the host-key string to be present verbatim"
    );
    fs::write(ledger_path, tampered).unwrap();

    let after_tamper = plan_json(&repo, &scratch.home(), "claude-code", "session-1", false);
    assert_eq!(row_action(&after_tamper, TRAIT_ID), "reinject");
    let row = after_tamper["value"]["rows"][0].clone();
    assert_eq!(row["stale-reason"], "missing-host-key");
}

#[test]
fn a_symlinked_ledger_leaf_refuses_and_writes_nothing() {
    let scratch = ScratchRoot::new("ledger-symlink-leaf-refused");
    let repo = ready_repo(&scratch);

    // First commit establishes the real ledger file path without this test
    // recomputing the repo-key hash itself.
    let first = plan_json(&repo, &scratch.home(), "claude-code", "session-1", true);
    assert_eq!(row_action(&first, TRAIT_ID), "inject");
    let ledger_files = find_ledger_files(&scratch.home());
    assert_eq!(ledger_files.len(), 1);
    let ledger_path = ledger_files[0].clone();

    fs::remove_file(&ledger_path).unwrap();
    let decoy = ledger_path.with_file_name("decoy-target.json");
    let decoy_bytes_before = "{}".to_string();
    fs::write(&decoy, &decoy_bytes_before).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&decoy, &ledger_path).unwrap();

    let output = run_ctx(
        &[
            "traits",
            "context",
            "plan",
            "--host",
            "claude-code",
            "--host-session",
            "session-1",
            "--task",
            "use the fixture trait",
            "--commit",
            "--json",
        ],
        &repo,
        &scratch.home(),
    );
    assert!(
        !output.status.success(),
        "context plan must refuse a symlinked ledger leaf"
    );
    assert!(
        fs::symlink_metadata(&ledger_path)
            .unwrap()
            .file_type()
            .is_symlink(),
        "a refused write must not replace the symlink leaf with a regular file"
    );
    let decoy_bytes_after = fs::read_to_string(&decoy).unwrap();
    assert_eq!(
        decoy_bytes_before, decoy_bytes_after,
        "a refused write must never touch the symlink target"
    );
}

#[test]
fn rejects_a_path_traversal_host_session_and_an_empty_host() {
    let scratch = ScratchRoot::new("ledger-rejects-bad-host-key");
    let repo = ready_repo(&scratch);

    let traversal = run_ctx(
        &[
            "traits",
            "context",
            "status",
            "--host",
            "claude-code",
            "--host-session",
            "../escape",
            "--json",
        ],
        &repo,
        &scratch.home(),
    );
    assert!(
        !traversal.status.success(),
        "a path-traversal host-session must be refused"
    );
    let (_, stderr) = support::utf8(&traversal);
    assert!(
        stderr.contains("letters, digits"),
        "refusal should name the charset rule: {stderr}"
    );

    let empty = run_ctx(
        &[
            "traits",
            "context",
            "status",
            "--host",
            "",
            "--host-session",
            "session-1",
            "--json",
        ],
        &repo,
        &scratch.home(),
    );
    assert!(
        !empty.status.success(),
        "an empty harness id must be refused"
    );
}

#[test]
fn status_reports_missing_then_loaded_then_missing_after_clear() {
    let scratch = ScratchRoot::new("ledger-status-lifecycle");
    let repo = ready_repo(&scratch);

    let status_args = [
        "traits",
        "context",
        "status",
        "--host",
        "claude-code",
        "--host-session",
        "session-1",
        "--json",
    ];

    let before = run_ctx(&status_args, &repo, &scratch.home());
    assert!(before.status.success());
    let (before_stdout, _) = support::utf8(&before);
    let before_json: serde_json::Value = serde_json::from_str(&before_stdout).unwrap();
    assert_eq!(before_json["value"]["ledger-state"], "missing");

    plan_json(&repo, &scratch.home(), "claude-code", "session-1", true);

    let loaded = run_ctx(&status_args, &repo, &scratch.home());
    assert!(loaded.status.success());
    let (loaded_stdout, _) = support::utf8(&loaded);
    let loaded_json: serde_json::Value = serde_json::from_str(&loaded_stdout).unwrap();
    assert_eq!(loaded_json["value"]["ledger-state"], "loaded");
    assert_eq!(loaded_json["value"]["entries"].as_array().unwrap().len(), 1);

    require_success(
        "`ctx traits context clear` empties the ledger",
        &[
            "traits",
            "context",
            "clear",
            "--host",
            "claude-code",
            "--host-session",
            "session-1",
            "--reason",
            "clear",
        ],
        &repo,
        &scratch.home(),
    );

    let after_clear = run_ctx(&status_args, &repo, &scratch.home());
    assert!(after_clear.status.success());
    let (after_clear_stdout, _) = support::utf8(&after_clear);
    let after_clear_json: serde_json::Value = serde_json::from_str(&after_clear_stdout).unwrap();
    assert_eq!(after_clear_json["value"]["ledger-state"], "missing");
    assert_eq!(after_clear_json["value"]["last-cleared-reason"], "clear");
}

#[test]
fn resolve_json_carries_source_digest_and_stays_a_bare_object() {
    let scratch = ScratchRoot::new("ledger-resolve-source-digest");
    let repo = ready_repo(&scratch);

    let output = run_ctx(
        &[
            "traits",
            "resolve",
            "--task",
            "use the fixture trait",
            "--json",
        ],
        &repo,
        &scratch.home(),
    );
    assert!(
        output.status.success(),
        "resolve should succeed: {:?}",
        support::utf8(&output)
    );
    let (stdout, _) = support::utf8(&output);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    // Bare object: `resolve --json` must never gain an envelope shape.
    assert!(
        value.get("ok").is_none(),
        "resolve --json must stay a bare object: {stdout}"
    );
    assert!(
        value.get("schema-version").is_none(),
        "resolve --json must stay a bare object: {stdout}"
    );

    let selected = value["selected"].as_array().expect("selected array");
    let fixture = selected
        .iter()
        .find(|c| c["trait-id"] == TRAIT_ID)
        .unwrap_or_else(|| panic!("fixture trait not selected: {stdout}"));
    assert!(
        fixture["source-digest"]
            .as_str()
            .is_some_and(|d| d.starts_with("sha256:")),
        "resolve --json candidate must carry source-digest: {fixture}"
    );
    assert!(
        fixture.get("model-view-digest").is_none(),
        "resolve never renders, so model-view-digest must stay absent: {fixture}"
    );
}
