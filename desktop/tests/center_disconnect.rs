//! Independent integration test proving the mid-stream disconnect case
//! (0256.5): a subscribed center goes away, the disconnected view stays
//! visible and labelled stale rather than fatal, a delta while stale changes
//! nothing, and a fresh snapshot on reconnect wholesale-replaces the stale
//! state rather than merging into it. Runs alone in its own target for the
//! same process-global env reason `absent_center.rs` documents.

mod support;

use std::time::{Duration, Instant, SystemTime};

use ctx_traits_desktop::center_link::LinkUpdate;
use ctx_traits_desktop::run_row::RepoScope;
use ctx_traits_desktop::shell::{CenterFace, CenterState};
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
fn mid_stream_disconnect_shows_stale_then_recovers_from_a_fresh_snapshot() {
    let guard = support::scratch("center-disconnect");
    let root = guard.0.clone();
    // SAFETY: this test owns the whole process environment for the duration
    // of the call below (it is the only test in this target) and restores
    // nothing because the process exits with it.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_root = root.join("repository");
    std::fs::create_dir_all(&ledger_root).expect("create ledger dir");
    let ledger_path = ledger_root.join("session.json");
    std::fs::write(&ledger_path, "seed").expect("seed ledger file");
    let seeded_contents = std::fs::read(&ledger_path).expect("read seeded ledger");

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let row_a = support::wire_row("repo", "/repo/a.json", "run-a");
    let row_b = support::wire_row("repo", "/repo/b.json", "run-b");
    connection.serve_snapshot(&subscribe_id, &[row_a, row_b]);
    connection.send_delta(&CenterDelta::RowChanged {
        row: Box::new(support::wire_row("repo", "/repo/b.json", "run-b")),
    });

    let snapshot_rows = recv_snapshot(&updates);
    assert_eq!(snapshot_rows.len(), 2);

    let mut face = CenterFace::new(RepoScope::All);
    face.apply(LinkUpdate::Snapshot(snapshot_rows), SystemTime::now());
    assert!(matches!(face.state(), CenterState::Connected { .. }));

    // Drain the one delta the peer already served before disconnecting.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match updates.try_recv() {
            Ok(LinkUpdate::Delta(delta)) => {
                face.apply(LinkUpdate::Delta(delta), SystemTime::now());
                break;
            }
            Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(LinkUpdate::Snapshot(_)) => {
                panic!("expected the pre-shutdown delta, got a snapshot")
            }
            Ok(LinkUpdate::Down(reason)) => {
                panic!("expected the pre-shutdown delta, got Down({reason})")
            }
            Ok(LinkUpdate::Board { .. }) => {}
            Err(error) => panic!("link channel ended before the delta arrived: {error}"),
        }
    }

    connection.shutdown();

    let reason = recv_down(&updates);
    face.apply(LinkUpdate::Down(reason.clone()), SystemTime::now());
    let CenterState::Stale { dashboard, .. } = face.state() else {
        panic!("expected Stale after a mid-stream disconnect");
    };
    assert_eq!(dashboard.len(), 2, "both rows must still be shown, stale");
    let header = face.header();
    assert!(
        header.contains("as of") && header.contains(&reason),
        "stale header must carry the as-of/reason wording: {header}"
    );
    assert!(
        !header.trim_start().starts_with(char::is_numeric),
        "stale rows must never be presented as a plain current run count: {header}"
    );

    // A delta injected while Stale must change nothing.
    face.apply(
        LinkUpdate::Delta(CenterDelta::Ended {
            row: Box::new(support::wire_row("repo", "/repo/a.json", "run-a")),
        }),
        SystemTime::now(),
    );
    let CenterState::Stale { dashboard, .. } = face.state() else {
        panic!("expected Stale to persist across an ignored delta");
    };
    assert_eq!(dashboard.len(), 2, "a delta while Stale must be dropped");

    // The peer accepts the reconnect and serves a fresh snapshot {B, C}.
    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let row_b_again = support::wire_row("repo", "/repo/b.json", "run-b");
    let row_c = support::wire_row("repo", "/repo/c.json", "run-c");
    connection.serve_snapshot(&subscribe_id, &[row_b_again, row_c]);

    let fresh_rows = recv_snapshot(&updates);
    face.apply(LinkUpdate::Snapshot(fresh_rows), SystemTime::now());
    let CenterState::Connected { dashboard } = face.state() else {
        panic!("expected Connected after a fresh snapshot");
    };
    let mut ledger_paths: Vec<_> = dashboard
        .rows()
        .iter()
        .map(|row| row.run_id.clone())
        .collect();
    ledger_paths.sort();
    assert_eq!(
        ledger_paths,
        vec!["run-b".to_string(), "run-c".to_string()],
        "A must be gone, B present exactly once, C present"
    );

    assert_eq!(
        std::fs::read(&ledger_path).expect("re-read seeded ledger"),
        seeded_contents,
        "the face must never touch running work under the runs root"
    );

    connection.shutdown();
    drop(updates);
}
