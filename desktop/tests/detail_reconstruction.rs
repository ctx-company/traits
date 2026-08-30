//! Integration proof for the task's "done when" clause: reconstruction
//! fixtures cover a live and a finished session baseline, and the same
//! baseline reappears after a simulated process restart. No env mutation —
//! this walks the real row -> selection -> load path against on-disk
//! ledgers, without a center.

mod support;

use camino::Utf8PathBuf;
use ctx_traits_desktop::detail::RunDetail;
use ctx_traits_desktop::run_row;

#[test]
fn live_and_finished_baselines_reconstruct_and_survive_a_restart() {
    let guard = support::scratch("detail-reconstruction");
    let root = &guard.0;

    let live_ledger = Utf8PathBuf::from_path_buf(root.join("repo-a").join("live-session.json"))
        .expect("UTF-8 ledger path");
    let live_fixture =
        support::write_session_ledger(&live_ledger, "live-session", "live-run", true);

    let finished_ledger =
        Utf8PathBuf::from_path_buf(root.join("repo-b").join("finished-session.json"))
            .expect("UTF-8 ledger path");
    let finished_fixture =
        support::write_session_ledger(&finished_ledger, "finished-session", "finished-run", false);

    let live_bytes_before = std::fs::read(&live_ledger).expect("read live ledger bytes");
    let finished_bytes_before =
        std::fs::read(&finished_ledger).expect("read finished ledger bytes");

    let wire_rows = vec![
        wire_row("repo-a", live_ledger.as_str(), "live-run", true),
        wire_row("repo-b", finished_ledger.as_str(), "finished-run", false),
    ];
    let rows = run_row::project(&wire_rows, &run_row::RepoScope::All);
    let live_row = rows
        .iter()
        .find(|row| row.ledger_path == live_ledger.as_str())
        .expect("live row present");
    let finished_row = rows
        .iter()
        .find(|row| row.ledger_path == finished_ledger.as_str())
        .expect("finished row present");

    let mut detail = RunDetail::default();
    let live_request = detail
        .select(live_row)
        .expect("live selection issues a request");
    let live_outcome = ctx_traits_desktop::detail::load(&live_request);
    assert_eq!(
        live_outcome.as_ref().expect("live ledger reads").session,
        live_fixture,
        "the reconstructed live session must equal the written fixture"
    );

    let mut detail = RunDetail::default();
    let finished_request = detail
        .select(finished_row)
        .expect("finished selection issues a request");
    let finished_outcome = ctx_traits_desktop::detail::load(&finished_request);
    assert_eq!(
        finished_outcome
            .as_ref()
            .expect("finished ledger reads")
            .session,
        finished_fixture,
        "the reconstructed finished session must equal the written fixture"
    );

    // Restart safety: drop both `RunDetail`s above and reconstruct fresh
    // ones from the same rows, exactly as a reopened process would.
    let mut restarted_live_detail = RunDetail::default();
    let restarted_live_request = restarted_live_detail
        .select(live_row)
        .expect("live selection issues a request after restart");
    assert_eq!(
        ctx_traits_desktop::detail::load(&restarted_live_request)
            .expect("live ledger reads")
            .session,
        live_fixture,
        "the live baseline must reappear identically after a simulated restart"
    );

    let mut restarted_finished_detail = RunDetail::default();
    let restarted_finished_request = restarted_finished_detail
        .select(finished_row)
        .expect("finished selection issues a request after restart");
    assert_eq!(
        ctx_traits_desktop::detail::load(&restarted_finished_request)
            .expect("finished ledger reads")
            .session,
        finished_fixture,
        "the finished baseline must reappear identically after a simulated restart"
    );

    assert_eq!(
        std::fs::read(&live_ledger).expect("read live ledger bytes again"),
        live_bytes_before,
        "loading a ledger must not modify it"
    );
    assert_eq!(
        std::fs::read(&finished_ledger).expect("read finished ledger bytes again"),
        finished_bytes_before,
        "loading a ledger must not modify it"
    );
}

fn wire_row(
    repo_key: &str,
    ledger_path: &str,
    run_id: &str,
    live: bool,
) -> ctx_traits_io::center::CenterPublicRow {
    let mut row = support::wire_row(repo_key, ledger_path, run_id);
    row.live = live;
    row
}
