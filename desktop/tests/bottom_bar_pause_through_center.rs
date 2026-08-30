//! Independent integration test for 0265.4: the Sessions bottom bar's
//! `pause` action, bound through `0258.3`'s landed control machine, tracks
//! only the center — never an optimistic click, never a stale selection —
//! and its outcome is also observed by a second, independent center
//! subscription. Runs alone in its own target for the same process-global
//! env reason `pause_resume_through_center.rs` documents (it calls
//! `support::install_center_env`, which owns process-global env).
//!
//! Honesty caveat: the second subscription below is a genuinely independent
//! `subscribe_existing` stream over its own socket connection, but the
//! fan-out itself is scripted by `FakePeer` — the desktop lane has no real
//! center daemon. The real center's own fan-out is proven io-side and in
//! `modules/cli/tests/proof_center.rs`; this test's contribution is that the
//! bar is bound to the same delta both observers see.

mod support;

use std::time::{Duration, Instant};

use camino::Utf8PathBuf;
use ctx_traits_desktop::bottom_bar::sessions_bar;
use ctx_traits_desktop::center_link::LinkUpdate;
use ctx_traits_desktop::row_control::{RowControls, RowVerb};
use ctx_traits_desktop::run_row::{RepoScope, RowState};
use ctx_traits_desktop::shell::{CenterFace, reconcile_row_controls};
use ctx_traits_io::center::{CenterDelta, CenterEvent};

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

fn recv_delta(updates: &async_channel::Receiver<LinkUpdate>) -> CenterDelta {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match updates.try_recv() {
            Ok(LinkUpdate::Delta(delta)) => return delta,
            Ok(_) => {}
            Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("no delta arrived before the deadline: {error}"),
        }
    }
}

fn recv_subscription_delta(
    subscription: &ctx_traits_io::center::CenterSubscription,
) -> CenterDelta {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match subscription.recv_timeout(Duration::from_millis(50)) {
            Ok(CenterEvent::Delta(delta)) => return delta,
            Ok(_) => {}
            Err(_) if Instant::now() < deadline => {}
            Err(error) => panic!("no delta arrived on the second subscription: {error}"),
        }
    }
}

#[test]
fn bar_state_and_bar_detail_track_only_the_center_across_a_full_pause() {
    let guard = support::scratch("bottom-bar-pause-through-center");
    let root = guard.0.clone();
    // SAFETY: this test owns the whole process environment for the duration
    // of the call below (it is the only test in this target) and restores
    // nothing because the process exits with it.
    let socket = unsafe { support::install_center_env(&root) };

    let outside_any_repo = root.join("not-a-repository");
    std::fs::create_dir_all(&outside_any_repo).expect("create outside-repo scratch dir");
    std::env::set_current_dir(&outside_any_repo).expect("leave every repository");

    let repo_a_path = root.join("repo-a");
    std::fs::create_dir_all(&repo_a_path).expect("create repo-a directory");
    let ledger_a =
        Utf8PathBuf::from_path_buf(repo_a_path.join("session.json")).expect("UTF-8 path");
    support::write_session_ledger(&ledger_a, "session-live", "run-live", true);
    let session_a = ctx_traits_io::run_session::read_run_session(&ledger_a).expect("read fixture");
    let row_a = support::row_from_ledger("repo-a", &ledger_a, &session_a, true);

    let repo_b_path = root.join("repo-b");
    std::fs::create_dir_all(&repo_b_path).expect("create repo-b directory");
    let ledger_b =
        Utf8PathBuf::from_path_buf(repo_b_path.join("session.json")).expect("UTF-8 path");
    support::write_session_ledger(&ledger_b, "session-live-b", "run-live-b", true);
    let session_b = ctx_traits_io::run_session::read_run_session(&ledger_b).expect("read fixture");
    let row_b = support::row_from_ledger("repo-b", &ledger_b, &session_b, true);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    connection.serve_snapshot(&subscribe_id, &[row_a.clone(), row_b.clone()]);

    let snapshot_rows = recv_snapshot(&updates);
    let mut face = CenterFace::new(RepoScope::All);
    face.apply(
        LinkUpdate::Snapshot(snapshot_rows),
        std::time::SystemTime::now(),
    );

    // A second, independent center subscription: its own connection, its
    // own subscribe id, its own reader — never the desktop's own link.
    // `subscribe_existing` blocks on the handshake response, so it must run
    // on its own thread while this thread accepts and completes the hello.
    let subscribe_thread = std::thread::spawn(|| ctx_traits_io::center::subscribe_existing(None));
    let mut second_connection = peer.accept();
    let second_subscription = subscribe_thread
        .join()
        .expect("subscribe thread")
        .expect("second subscription connects");
    let second_subscribe_id = second_connection.read_subscribe_id();
    second_connection.serve_snapshot(&second_subscribe_id, &[row_a.clone(), row_b.clone()]);
    // Drain the second subscription's own snapshot events before the delta.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match second_subscription.recv_timeout(Duration::from_millis(50)) {
            Ok(CenterEvent::SnapshotEnd) => break,
            Ok(_) => {}
            Err(_) if Instant::now() < deadline => {}
            Err(error) => panic!("second subscription's snapshot never completed: {error}"),
        }
    }

    let mut controls = RowControls::default();

    let find_row = |face: &CenterFace, ledger: &Utf8PathBuf| {
        face.rows()
            .iter()
            .find(|row| row.ledger_path == ledger.as_str())
            .expect("row present")
            .clone()
    };

    // Baseline: no control status, bar word is "running", no detail.
    let row = find_row(&face, &ledger_a);
    assert_eq!(row.state, RowState::Live);
    let baseline_bar = sessions_bar(&row, controls.status(&row.ledger_path));
    assert_eq!(baseline_bar.state.word, "running");
    assert_eq!(baseline_bar.detail_text(), None);

    // Press: exactly one request reaches the wire; a second press while
    // in flight issues nothing more.
    let request = controls
        .request(&row, RowVerb::Pause)
        .expect("a live row is eligible for pause");
    assert_eq!(request.session_id, "session-live");
    assert_eq!(request.repo_key, "repo-a");
    assert!(
        controls.request(&row, RowVerb::Pause).is_none(),
        "a second press while one is in flight must issue nothing more"
    );

    let request_for_dispatch = request.clone();
    let dispatch_thread = std::thread::spawn(move || {
        ctx_traits_desktop::row_control::dispatch(&request_for_dispatch)
    });

    let mut control_connection = peer.accept();
    let (control_id, wire_session_id, wire_repo_key, wire_command) =
        control_connection.read_control_request();
    assert_eq!(
        (
            wire_session_id.as_str(),
            wire_repo_key.as_deref(),
            wire_command.as_str()
        ),
        ("session-live", Some("repo-a"), "pause")
    );

    // After press, before response: bar word unchanged, detail requesting.
    let row = find_row(&face, &ledger_a);
    let requesting_bar = sessions_bar(&row, controls.status(&row.ledger_path));
    assert_eq!(requesting_bar.state.word, "running");
    assert_eq!(
        requesting_bar.detail_text().as_deref(),
        Some("· requesting…")
    );

    control_connection.send_control_acknowledged(&control_id);
    let outcome = dispatch_thread.join().expect("dispatch thread");
    let settled = controls.settle(&request.ledger_path, request.generation, outcome);
    assert!(settled);

    // Acknowledged, before the delta: bar word still "running", detail
    // reads the requested wording, never "paused".
    assert!(!reconcile_row_controls(&face, &mut controls));
    let row = find_row(&face, &ledger_a);
    let acknowledged_bar = sessions_bar(&row, controls.status(&row.ledger_path));
    assert_eq!(acknowledged_bar.state.word, "running");
    let detail = acknowledged_bar.detail_text().expect("a requested detail");
    assert_eq!(
        detail,
        "· pause requested (session-live) — waiting for the center"
    );
    assert!(!detail.contains("paused"));

    // Selection scoping: row B's bar carries no control-status detail while
    // row A's is still pending.
    let row_b_projected = find_row(&face, &ledger_b);
    let row_b_bar = sessions_bar(
        &row_b_projected,
        controls.status(&row_b_projected.ledger_path),
    );
    assert_eq!(row_b_bar.detail_text(), None);

    // Settled delta, sent on both connections: the desktop reducer applies
    // it, and the second, independent subscriber observes the same delta.
    let mut paused_row = row_a.clone();
    paused_row.live = false;
    paused_row.summary.last_drive_outcome = Some("paused".to_string());
    connection.send_delta(&CenterDelta::RowChanged {
        row: Box::new(paused_row.clone()),
    });
    second_connection.send_delta(&CenterDelta::RowChanged {
        row: Box::new(paused_row),
    });

    let delta = recv_delta(&updates);
    face.apply(LinkUpdate::Delta(delta), std::time::SystemTime::now());
    assert!(reconcile_row_controls(&face, &mut controls));
    assert!(controls.status(ledger_a.as_str()).is_none());

    let row = find_row(&face, &ledger_a);
    let paused_bar = sessions_bar(&row, controls.status(&row.ledger_path));
    assert_eq!(paused_bar.state.word, "paused");
    assert_eq!(paused_bar.detail_text(), None);

    let second_delta = recv_subscription_delta(&second_subscription);
    match second_delta {
        CenterDelta::RowChanged { row } => {
            assert_eq!(row.summary.last_drive_outcome.as_deref(), Some("paused"));
        }
        other => panic!("expected a RowChanged delta on the second subscription, got {other:?}"),
    }

    connection.shutdown();
    second_connection.shutdown();
    drop(updates);
    drop(second_subscription);
}
