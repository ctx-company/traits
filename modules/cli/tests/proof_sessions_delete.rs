//! 0252.5: `ctx traits sessions delete --failed` end-to-end. Drives the real
//! `ctx` binary against a seeded run-session store — exact `Status::Failed`
//! selection (decision 1), an unreadable ledger never selected (decision 2),
//! a held driver lock counted as a live skip (decision 3), an honest
//! `deleted` count, and a bare invocation refusing without a selector.

use std::fs;
use std::path::Path;

use support::{ScratchRoot, active_repo_key, controlled_command, ctx_bin, git_init, run_ctx};

fn store_root(home: &Path, active_key: &str) -> std::path::PathBuf {
    home.join("ctx")
        .join("traits")
        .join("runs")
        .join(active_key)
}

fn write_ledger(store: &Path, session_id: &str, status: &str) {
    let ledger = store.join(format!("{session_id}.json"));
    // `Status` and `FinalState` are distinct, narrower vocabularies — every
    // in-progress `Status` (e.g. `awaiting-agent-output`) maps to
    // `FinalState::Running`, matching `proof_center.rs`'s own fixture.
    let final_state = match status {
        "completed" => "completed",
        "failed" => "failed",
        "rejected" => "rejected",
        _ => "running",
    };
    let session = serde_json::from_value(serde_json::json!({
        "schema-version": "0.1.0",
        "session-id": session_id,
        "run-id": format!("run-{session_id}"),
        "trait-id": "sessions-delete-fixture",
        "current-run-index": 0,
        "status": status,
        "provenance": {
            "started-by": {"surface": "test", "caller": "proof-sessions-delete"},
            "state-source": "test",
            "started-at-epoch": 1000,
        },
        "ledger": {
            "run-id": format!("run-{session_id}"),
            "trait-id": "sessions-delete-fixture",
            "current-run-index": 0,
            "final-state": final_state,
        },
        "state-digest": "sha256:sessions-delete-fixture",
    }))
    .expect("fixture session");
    let ledger = camino::Utf8PathBuf::from_path_buf(ledger).expect("UTF-8 fixture ledger path");
    ctx_traits_io::run_session::write_run_session(&ledger, &session).expect("write fixture");
}

#[test]
fn sessions_delete_failed_selects_exact_failed_status_only() {
    let scratch = ScratchRoot::new("p0252-5-sessions-delete-selection");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    let home = scratch.home();
    let active_key = active_repo_key(&repo, &home);
    let store = store_root(&home, &active_key);
    fs::create_dir_all(&store).unwrap();

    write_ledger(&store, "failed-one", "failed");
    write_ledger(&store, "completed-one", "completed");
    write_ledger(&store, "awaiting-one", "awaiting-agent-output");
    write_ledger(&store, "rejected-one", "rejected");
    fs::write(store.join("corrupt.json"), "not json").unwrap();

    let output = run_ctx(&["traits", "sessions", "delete", "--failed"], &repo, &home);
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("deleted: 1 session(s)"), "stdout: {stdout}");
    assert!(
        !stdout.contains("skipped (live)"),
        "no live sessions in this store: {stdout}"
    );

    assert!(!store.join("failed-one.json").exists());
    assert!(store.join("completed-one.json").exists());
    assert!(store.join("awaiting-one.json").exists());
    assert!(store.join("rejected-one.json").exists());
    assert!(store.join("corrupt.json").exists());
}

#[test]
fn sessions_delete_failed_skips_a_held_driver_lock() {
    let scratch = ScratchRoot::new("p0252-5-sessions-delete-held-lock");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    let home = scratch.home();
    let active_key = active_repo_key(&repo, &home);
    let store = store_root(&home, &active_key);
    fs::create_dir_all(&store).unwrap();

    write_ledger(&store, "failed-held", "failed");
    let ledger = camino::Utf8PathBuf::from_path_buf(store.join("failed-held.json"))
        .expect("UTF-8 ledger path");
    let lock_path = ctx_traits_io::run_control::driver_lock_path(&ledger);
    let lock =
        ctx_traits_io::file_lock::open_lock_file_no_follow(&lock_path).expect("open driver lock");
    ctx_traits_io::file_lock::lock_exclusive_blocking(&lock).expect("hold driver lock");

    let output = run_ctx(&["traits", "sessions", "delete", "--failed"], &repo, &home);
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("deleted: 0 session(s)"), "stdout: {stdout}");
    assert!(
        stdout.contains("skipped (live): 1 session(s)"),
        "stdout: {stdout}"
    );
    assert!(
        store.join("failed-held.json").exists(),
        "a held driver lock must never be deleted"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed-held"),
        "the skipped session must be named on stderr: {stderr}"
    );

    drop(lock);
}

#[test]
fn sessions_delete_failed_against_an_empty_store_is_a_clean_pass() {
    let scratch = ScratchRoot::new("p0252-5-sessions-delete-empty");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    let home = scratch.home();

    let output = run_ctx(&["traits", "sessions", "delete", "--failed"], &repo, &home);
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("deleted: 0 session(s)"), "stdout: {stdout}");
    assert!(
        !stdout.contains("skipped (live)"),
        "an empty store must never grow a skipped-live row: {stdout}"
    );
    let body_lines = stdout
        .lines()
        .filter(|line| {
            line.trim_start().starts_with("deleted") || line.trim_start().starts_with("skipped")
        })
        .count();
    assert!(
        body_lines <= 1,
        "panel must carry at most two rows: {stdout}"
    );
}

#[test]
fn bare_sessions_delete_refuses_without_a_selector() {
    let scratch = ScratchRoot::new("p0252-5-sessions-delete-no-selector");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    let home = scratch.home();

    let mut command =
        controlled_command(&ctx_bin(), &["traits", "sessions", "delete"], &repo, &home);
    let output = command.output().expect("run ctx");
    assert!(
        !output.status.success(),
        "a bare `sessions delete` must exit nonzero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--failed") || stderr.contains("failed"),
        "stderr must name the missing selector: {stderr}"
    );
}
