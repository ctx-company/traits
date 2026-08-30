//! Integration proof that detail resolution uses center-supplied repository
//! identity, never the desktop process's own cwd/HOME. Env-mutating (`HOME`
//! and the process cwd), so this stays the only `#[test]` in its target —
//! the same convention `tests/support/mod.rs` documents.

mod support;

use camino::Utf8PathBuf;
use ctx_traits_desktop::detail::RunDetail;
use ctx_traits_desktop::run_row;

#[test]
fn selection_resolves_through_center_supplied_identity_not_cwd() {
    let guard = support::scratch("detail-repository-identity");
    let root = &guard.0;

    let empty_home = root.join("empty-home");
    std::fs::create_dir_all(&empty_home).expect("create empty HOME");
    let outside_cwd = root.join("outside-any-repo");
    std::fs::create_dir_all(&outside_cwd).expect("create outside-repo cwd");

    // SAFETY: this test owns the whole process environment for its duration
    // and is the only test in this target for exactly that reason.
    unsafe {
        std::env::set_var("HOME", &empty_home);
    }
    std::env::set_current_dir(&outside_cwd).expect("chdir outside any repo");

    let ledger_a = Utf8PathBuf::from_path_buf(root.join("repo-a").join("shared-session.json"))
        .expect("UTF-8 ledger path");
    let fixture_a = support::write_session_ledger(&ledger_a, "shared-session", "shared-run", true);

    let ledger_b = Utf8PathBuf::from_path_buf(root.join("repo-b").join("shared-session.json"))
        .expect("UTF-8 ledger path");
    let fixture_b = support::write_session_ledger(&ledger_b, "shared-session", "shared-run", false);

    let mut wire_row_a = support::wire_row("repo-a", ledger_a.as_str(), "shared-run");
    wire_row_a.live = true;
    let mut wire_row_b = support::wire_row("repo-b", ledger_b.as_str(), "shared-run");
    wire_row_b.live = false;

    let rows = run_row::project(&[wire_row_a, wire_row_b], &run_row::RepoScope::All);
    let row_a = rows
        .iter()
        .find(|row| row.repo_key == "repo-a")
        .expect("repo-a row present");
    let row_b = rows
        .iter()
        .find(|row| row.repo_key == "repo-b")
        .expect("repo-b row present");

    let mut detail = RunDetail::default();
    let request_a = detail
        .select(row_a)
        .expect("repo-a selection issues a request");
    assert_eq!(
        ctx_traits_desktop::detail::load(&request_a)
            .expect("repo-a ledger reads")
            .session,
        fixture_a,
        "the same run-id/session-id in repo-a must resolve to repo-a's own ledger"
    );

    let mut detail = RunDetail::default();
    let request_b = detail
        .select(row_b)
        .expect("repo-b selection issues a request");
    assert_eq!(
        ctx_traits_desktop::detail::load(&request_b)
            .expect("repo-b ledger reads")
            .session,
        fixture_b,
        "the same run-id/session-id in repo-b must resolve to repo-b's own ledger, not repo-a's"
    );
}
