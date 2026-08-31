//! Integration proofs for the Sessions screen header (0265.13), driven
//! against a real `support::FakePeer` center — the same
//! `preview_run_block.rs`/`detail_stale_recovery.rs` "one `#[test]`
//! sequencing every scenario in this file" convention, since every scenario
//! here needs a live fake center and mutates process-global env.

mod support;

use std::time::{Duration, Instant};

use camino::Utf8PathBuf;
use ctx_traits_desktop::center_link::LinkUpdate;
use ctx_traits_desktop::detail::{self, RunDetail};
use ctx_traits_desktop::screen_header::sessions_header;
use ctx_traits_desktop::{run_row, screen_header};

fn recv_snapshot(
    updates: &async_channel::Receiver<LinkUpdate>,
) -> Vec<ctx_traits_io::center::CenterPublicRow> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match updates.try_recv() {
            Ok(LinkUpdate::Snapshot(rows)) => return rows,
            Ok(_) => {}
            Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("no snapshot arrived before the deadline: {error}"),
        }
    }
}

fn load_serving_claimed_task(
    peer: &support::FakePeer,
    request: &detail::LoadRequest,
    tag: &str,
    data: Option<serde_json::Value>,
) -> Result<detail::DetailBaseline, String> {
    std::thread::scope(|scope| {
        let server = scope.spawn(|| {
            let mut connection = peer.accept();
            let (id, _session_id, _repo_key) = connection.read_claimed_task_request();
            connection.send_claimed_task_result(&id, tag, data);
        });
        let outcome = detail::load(request);
        server.join().expect("claimed-task server thread");
        outcome
    })
}

fn load_with_claimed_task_failure(
    peer: &support::FakePeer,
    request: &detail::LoadRequest,
) -> Result<detail::DetailBaseline, String> {
    std::thread::scope(|scope| {
        let server = scope.spawn(|| {
            let connection = peer.accept();
            connection.shutdown();
        });
        let outcome = detail::load(request);
        server.join().expect("claimed-task server thread");
        outcome
    })
}

/// Runs every scenario in this file sequentially, in a single `#[test]`,
/// so each can call `install_center_env` (a process-global env mutation)
/// in turn without racing a sibling scenario.
#[test]
fn sessions_header_integration() {
    served_answer_drives_the_title_never_the_row_s_task_key();
    live_session_title_updates_the_summary_with_no_resync();
    absent_summary_title_renders_one_non_empty_non_error_summary();
    claimed_task_transport_failure_renders_a_loud_non_success_title();
}

/// Goal: the header title comes from the one accepted `0265.2` answer's own
/// key/title, never the row's own `task_key` — the row's `task_key` is
/// deliberately made to diverge from the served answer's key, so a
/// row-sourced key would be a visible, provable failure.
fn served_answer_drives_the_title_never_the_row_s_task_key() {
    let guard = support::scratch("sessions-header-served-answer");
    let root = guard.0.clone();
    // SAFETY: single-threaded scenario runner, no concurrent env mutation.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_path =
        Utf8PathBuf::from_path_buf(root.join("repo").join("session.json")).expect("UTF-8 path");
    let session = support::write_session_ledger(&ledger_path, "session-a", "run-a", true);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let mut wire_row = support::row_from_ledger("repo", &ledger_path, &session, true);
    wire_row.summary.task_key = Some("row-task-key-must-not-appear".to_string());
    connection.serve_snapshot(&subscribe_id, std::slice::from_ref(&wire_row));

    let snapshot_rows = recv_snapshot(&updates);
    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);
    let row = rows
        .iter()
        .find(|row| row.ledger_path == ledger_path.as_str())
        .expect("seeded row present");

    let mut detail = RunDetail::default();
    let request = detail.select(row).expect("selection issues a request");
    let served = serde_json::json!({
        "key": "0265.13",
        "title": "render the sessions header",
        "description": "",
        "stored-status": null,
        "auto-close": null,
    });
    let outcome = load_serving_claimed_task(&peer, &request, "task", Some(served));
    assert!(detail.apply(request.generation, outcome));

    let header = sessions_header(detail.preview_state().as_ref(), None);
    assert!(header.title.contains("0265.13"));
    assert!(header.title.contains("render the sessions header"));
    assert!(!header.title.contains("row-task-key-must-not-appear"));

    drop(updates);
}

/// Goal 5: a live `SessionTitle` sidecar line immediately followed by the
/// center's own `RowChanged` for the rewritten title updates the header's
/// summary, and `FollowOutcome.request` stays `None` — no ledger re-read.
fn live_session_title_updates_the_summary_with_no_resync() {
    let guard = support::scratch("sessions-header-live-title");
    let root = guard.0.clone();
    // SAFETY: see above.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_path =
        Utf8PathBuf::from_path_buf(root.join("repo").join("session.json")).expect("UTF-8 path");
    let session = support::write_session_ledger(&ledger_path, "session-a", "run-a", true);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let wire_row = support::row_from_ledger("repo", &ledger_path, &session, true);
    connection.serve_snapshot(&subscribe_id, std::slice::from_ref(&wire_row));

    let snapshot_rows = recv_snapshot(&updates);
    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);
    let row = rows
        .iter()
        .find(|row| row.ledger_path == ledger_path.as_str())
        .expect("seeded row present");

    let mut detail = RunDetail::default();
    let request = detail.select(row).expect("selection issues a request");
    let outcome = load_serving_claimed_task(&peer, &request, "unclaimed", None);
    assert!(detail.apply(request.generation, outcome));

    let before = sessions_header(detail.preview_state().as_ref(), None);
    assert_eq!(before.summary, "no run description yet");

    let mut retitled = support::row_from_ledger("repo", &ledger_path, &session, true);
    retitled.summary.title = Some("a live title".to_string());
    let outcome = detail.follow(&LinkUpdate::Delta(
        ctx_traits_io::center::CenterDelta::RowChanged {
            row: Box::new(retitled),
        },
    ));
    assert!(
        outcome.request.is_none(),
        "a title-only rewrite must not force a ledger re-read"
    );
    assert!(outcome.changed);

    let after = sessions_header(detail.preview_state().as_ref(), None);
    assert_eq!(after.summary, "a live title");

    drop(updates);
}

/// Goal: an absent raw session title renders one non-empty, non-error
/// summary — never a blank string, never a fabricated error.
fn absent_summary_title_renders_one_non_empty_non_error_summary() {
    let guard = support::scratch("sessions-header-absent-title");
    let root = guard.0.clone();
    // SAFETY: see above.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_path =
        Utf8PathBuf::from_path_buf(root.join("repo").join("session.json")).expect("UTF-8 path");
    let session = support::write_session_ledger(&ledger_path, "session-a", "run-a", true);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let wire_row = support::row_from_ledger("repo", &ledger_path, &session, true);
    connection.serve_snapshot(&subscribe_id, std::slice::from_ref(&wire_row));

    let snapshot_rows = recv_snapshot(&updates);
    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);
    let row = rows
        .iter()
        .find(|row| row.ledger_path == ledger_path.as_str())
        .expect("seeded row present");
    assert!(row.session_title.is_none());

    let mut detail = RunDetail::default();
    let request = detail.select(row).expect("selection issues a request");
    let outcome = load_serving_claimed_task(&peer, &request, "unclaimed", None);
    assert!(detail.apply(request.generation, outcome));

    let header = sessions_header(detail.preview_state().as_ref(), None);
    assert!(!header.summary.is_empty());
    assert_eq!(header.summary, "no run description yet");

    drop(updates);
}

/// Goal: the claimed-task transport failing (connection dropped unanswered)
/// still renders a loud, non-success title through the same wording
/// `preview.rs`'s `task_row` uses — never blank, never a panic.
fn claimed_task_transport_failure_renders_a_loud_non_success_title() {
    let guard = support::scratch("sessions-header-transport-failure");
    let root = guard.0.clone();
    // SAFETY: see above.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_path =
        Utf8PathBuf::from_path_buf(root.join("repo").join("session.json")).expect("UTF-8 path");
    let session = support::write_session_ledger(&ledger_path, "session-a", "run-a", true);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let wire_row = support::row_from_ledger("repo", &ledger_path, &session, true);
    connection.serve_snapshot(&subscribe_id, std::slice::from_ref(&wire_row));

    let snapshot_rows = recv_snapshot(&updates);
    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);
    let row = rows
        .iter()
        .find(|row| row.ledger_path == ledger_path.as_str())
        .expect("seeded row present");

    let mut detail = RunDetail::default();
    let request = detail.select(row).expect("selection issues a request");
    let outcome = load_with_claimed_task_failure(&peer, &request);
    assert!(detail.apply(request.generation, outcome));

    let header = screen_header::sessions_header(detail.preview_state().as_ref(), None);
    assert_eq!(header.title, "task unavailable: center error");

    drop(updates);
}
