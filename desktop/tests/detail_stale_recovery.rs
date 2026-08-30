//! Independent integration test for 0257.3's subscription-loss/recovery
//! edge on a *selected* run's detail, as distinct from `center_disconnect.rs`
//! (which covers only the dashboard face). Drives `support::FakePeer` by
//! hand: serve a snapshot, select and load, `shutdown()` the peer (the wire
//! shape of both a center exit and a backpressure eviction), assert the link
//! reports `Down` and detail goes `Stale` and stays visibly labelled; feed a
//! delta while stale and assert nothing changes; reconnect and serve a fresh
//! snapshot, assert exactly one resync request and a return to `Following`.
//! Runs alone in its own target for the same process-global env reason
//! `center_disconnect.rs` documents.

mod support;

use std::time::{Duration, Instant};

use camino::Utf8PathBuf;
use ctx_traits_desktop::center_link::LinkUpdate;
use ctx_traits_desktop::detail::{self, FollowState, RunDetail};
use ctx_traits_desktop::run_row;
use ctx_traits_io::center::CenterDelta;

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

fn recv_down(updates: &async_channel::Receiver<LinkUpdate>) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match updates.try_recv() {
            Ok(LinkUpdate::Down(reason)) => return reason,
            Ok(_) => {}
            Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("no Down arrived before the deadline: {error}"),
        }
    }
}

#[test]
fn a_selected_runs_detail_goes_stale_on_disconnect_and_resyncs_exactly_once_on_recovery() {
    let guard = support::scratch("detail-stale-recovery");
    let root = guard.0.clone();
    // SAFETY: this test owns the whole process environment for the duration
    // of the call below (it is the only test in this target) and restores
    // nothing because the process exits with it.
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
    let seed_request = detail
        .select(row)
        .expect("first selection issues a request");
    let seed_outcome = detail::load(&seed_request);
    assert!(detail.apply(seed_request.generation, seed_outcome));
    assert!(matches!(
        detail.follow_state(),
        Some(FollowState::Following)
    ));
    assert!(detail.tree().is_some(), "the seed baseline is visible");

    connection.shutdown();
    let reason = recv_down(&updates);
    let outcome = detail.follow(&LinkUpdate::Down(reason.clone()));
    assert!(outcome.changed);
    assert!(outcome.request.is_none());
    let Some(FollowState::Stale {
        reason: stale_reason,
    }) = detail.follow_state()
    else {
        panic!("expected Stale after the disconnect");
    };
    assert_eq!(stale_reason, &reason);
    assert!(
        detail.tree().is_some(),
        "the last authoritative tree stays visible while stale"
    );

    // A delta arriving while stale must change nothing.
    let outcome = detail.follow(&LinkUpdate::Delta(CenterDelta::RowChanged {
        row: Box::new(wire_row.clone()),
    }));
    assert!(!outcome.changed, "a delta while stale must be dropped");
    assert!(outcome.request.is_none());
    assert!(matches!(
        detail.follow_state(),
        Some(FollowState::Stale { .. })
    ));

    // Reconnect and serve a fresh recovery snapshot.
    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    connection.serve_snapshot(&subscribe_id, &[wire_row]);
    let recovery_rows = recv_snapshot(&updates);

    let outcome = detail.follow(&LinkUpdate::Snapshot(recovery_rows));
    assert!(outcome.changed);
    let resync = outcome
        .request
        .expect("the recovery snapshot must issue exactly one resync request");
    assert_eq!(resync.ledger_path, ledger_path);
    assert!(matches!(
        detail.follow_state(),
        Some(FollowState::Following)
    ));

    let resync_outcome = detail::load(&resync);
    assert!(detail.apply(resync.generation, resync_outcome));
    assert!(matches!(
        detail.follow_state(),
        Some(FollowState::Following)
    ));

    connection.shutdown();
    drop(updates);
}
