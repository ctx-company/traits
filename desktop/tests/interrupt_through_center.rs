//! Independent integration test for 0258.2: requesting an interrupt for a
//! live row from the desktop reaches the center on its existing-only
//! control entry, with the exact wire shape the center expects, and the
//! displayed outcome resolves only once the row's own subscription delta
//! carries it into a non-live state — never from the control response's
//! acknowledgement alone. Drives `support::FakePeer` by hand on two
//! connections (the snapshot/delta subscription, and a separate one for the
//! control request/response round trip — the client never pipelines two
//! requests on one connection, see `support::FakePeer`'s own doc comment).
//! Runs alone in its own target for the same process-global env reason
//! `detail_stale_recovery.rs` documents.

mod support;

use std::time::{Duration, Instant};

use camino::Utf8PathBuf;
use ctx_traits_desktop::center_link::LinkUpdate;
use ctx_traits_desktop::row_control::{RowControls, RowOutcome, RowStatus, RowVerb};
use ctx_traits_desktop::shell::{CenterFace, reconcile_row_controls};
use ctx_traits_io::center::{CenterDelta, ControlAction, ControlResult};

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

#[test]
fn interrupting_a_live_row_reaches_the_center_and_resolves_only_through_a_delta() {
    let guard = support::scratch("interrupt-through-center");
    let root = guard.0.clone();
    // SAFETY: this test owns the whole process environment for the duration
    // of the call below (it is the only test in this target) and restores
    // nothing because the process exits with it.
    let socket = unsafe { support::install_center_env(&root) };

    // Run the process outside any repository, so a cwd-derived repo_path is
    // structurally impossible to confuse with the center-supplied one.
    let outside_any_repo = root.join("not-a-repository");
    std::fs::create_dir_all(&outside_any_repo).expect("create outside-repo scratch dir");
    std::env::set_current_dir(&outside_any_repo).expect("leave every repository");

    let repo_a_path = root.join("repo-a");
    std::fs::create_dir_all(&repo_a_path).expect("create repo-a directory");
    let ledger = Utf8PathBuf::from_path_buf(repo_a_path.join("session.json")).expect("UTF-8 path");
    support::write_session_ledger(&ledger, "session-live", "run-live", true);
    let session = ctx_traits_io::run_session::read_run_session(&ledger).expect("read fixture");
    let live_row = support::row_from_ledger("repo-a", &ledger, &session, true);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    connection.serve_snapshot(&subscribe_id, std::slice::from_ref(&live_row));

    let snapshot_rows = recv_snapshot(&updates);
    let mut face = CenterFace::new(ctx_traits_desktop::run_row::RepoScope::All);
    face.apply(
        LinkUpdate::Snapshot(snapshot_rows),
        std::time::SystemTime::now(),
    );

    let row = face
        .rows()
        .iter()
        .find(|row| row.ledger_path == ledger.as_str())
        .expect("the live row is present after the snapshot")
        .clone();
    assert_eq!(row.state, ctx_traits_desktop::run_row::RowState::Live);

    // --- Ordering A: acknowledgement lands, then the row's own delta. ---
    let mut interrupts = RowControls::default();
    let request = interrupts
        .request(&row, RowVerb::Interrupt)
        .expect("a live row is eligible");
    assert_eq!(request.session_id, "session-live");
    assert_eq!(request.repo_key, "repo-a");

    let session_id = request.session_id.clone();
    let repo_key = request.repo_key.clone();
    let control_thread = std::thread::spawn(move || {
        ctx_traits_io::center::control_existing(
            &session_id,
            Some(&repo_key),
            ControlAction::Interrupt,
        )
    });

    let mut control_connection = peer.accept();
    let (control_id, wire_session_id, wire_repo_key, wire_command) =
        control_connection.read_control_request();
    assert_eq!(
        wire_session_id, "session-live",
        "the session id must be exactly the one the center supplied"
    );
    assert_eq!(
        wire_repo_key.as_deref(),
        Some("repo-a"),
        "the repo key must be exactly the one the center supplied"
    );
    assert_eq!(
        wire_command, "interrupt",
        "the wire command must be the shared interrupt verb, never a GUI-specific one"
    );
    control_connection.send_control_acknowledged(&control_id);

    let control_result = control_thread
        .join()
        .expect("control_existing thread")
        .expect("center control request");
    assert!(matches!(control_result, ControlResult::Acknowledged));

    let settled = interrupts.settle(
        &request.ledger_path,
        request.generation,
        RowOutcome::Control(control_result),
    );
    assert!(settled);
    assert!(matches!(
        interrupts.status(&request.ledger_path),
        Some(RowStatus::Requested { .. })
    ));

    // Acknowledgement alone must not resolve anything: the row must still
    // be live in the face.
    assert!(
        !reconcile_row_controls(&face, &mut interrupts),
        "acknowledgement alone must not resolve a pending interrupt"
    );
    assert!(
        face.rows()
            .iter()
            .find(|row| row.ledger_path == ledger.as_str())
            .expect("row still present")
            .live,
        "the row must stay live until the center's own delta carries the effect"
    );

    let mut stopped_row = live_row.clone();
    stopped_row.live = false;
    stopped_row.summary.last_drive_outcome = Some("interrupted".to_string());
    connection.send_delta(&CenterDelta::RowChanged {
        row: Box::new(stopped_row),
    });
    let delta = recv_delta(&updates);
    face.apply(LinkUpdate::Delta(delta), std::time::SystemTime::now());

    assert!(
        reconcile_row_controls(&face, &mut interrupts),
        "reconciliation must resolve the pending interrupt once the delta lands"
    );
    assert!(interrupts.status(&request.ledger_path).is_none());
    assert!(
        !face
            .rows()
            .iter()
            .find(|row| row.ledger_path == ledger.as_str())
            .expect("row still present")
            .live,
        "the row must now show as no longer live"
    );

    // --- Ordering B: the row's own delta wins the race and lands before
    // the control response settles. Restore a live row via a fresh delta,
    // request a second interrupt, and let the delta arrive first.
    let mut relived_row = live_row.clone();
    relived_row.ledger_path = repo_a_path
        .join("second-session.json")
        .to_str()
        .expect("utf8 path")
        .to_string();
    relived_row.summary.session_id = "session-second".to_string();
    relived_row.summary.run_id = "run-second".to_string();
    connection.send_delta(&CenterDelta::Appeared {
        row: Box::new(relived_row.clone()),
    });
    let appeared = recv_delta(&updates);
    face.apply(LinkUpdate::Delta(appeared), std::time::SystemTime::now());
    let second_row = face
        .rows()
        .iter()
        .find(|row| row.ledger_path == relived_row.ledger_path)
        .expect("the second live row is present")
        .clone();
    assert_eq!(
        second_row.state,
        ctx_traits_desktop::run_row::RowState::Live
    );

    let second_request = interrupts
        .request(&second_row, RowVerb::Interrupt)
        .expect("a live row is eligible");
    let second_session_id = second_request.session_id.clone();
    let second_repo_key = second_request.repo_key.clone();
    let second_control_thread = std::thread::spawn(move || {
        ctx_traits_io::center::control_existing(
            &second_session_id,
            Some(&second_repo_key),
            ControlAction::Interrupt,
        )
    });

    let mut second_control_connection = peer.accept();
    let (second_control_id, _sid, _rk, _cmd) = second_control_connection.read_control_request();

    let mut second_stopped_row = relived_row.clone();
    second_stopped_row.live = false;
    connection.send_delta(&CenterDelta::RowChanged {
        row: Box::new(second_stopped_row),
    });
    let second_delta = recv_delta(&updates);
    face.apply(
        LinkUpdate::Delta(second_delta),
        std::time::SystemTime::now(),
    );
    assert!(
        !reconcile_row_controls(&face, &mut interrupts),
        "reconciliation must not fire while the entry is still Requesting, not Requested"
    );

    second_control_connection.send_control_acknowledged(&second_control_id);
    let second_control_result = second_control_thread
        .join()
        .expect("second control_existing thread")
        .expect("second center control request");
    let second_settled = interrupts.settle(
        &second_request.ledger_path,
        second_request.generation,
        RowOutcome::Control(second_control_result),
    );
    assert!(second_settled);
    assert!(
        reconcile_row_controls(&face, &mut interrupts),
        "the row was already non-live when the response arrived, so reconciliation must fire at settle time"
    );
    assert!(interrupts.status(&second_request.ledger_path).is_none());

    // --- One refusal path: the center reports the driver never
    // acknowledged the request. ---
    let mut third_row = live_row.clone();
    third_row.ledger_path = repo_a_path
        .join("third-session.json")
        .to_str()
        .expect("utf8 path")
        .to_string();
    third_row.summary.session_id = "session-third".to_string();
    third_row.summary.run_id = "run-third".to_string();
    connection.send_delta(&CenterDelta::Appeared {
        row: Box::new(third_row.clone()),
    });
    let third_appeared = recv_delta(&updates);
    face.apply(
        LinkUpdate::Delta(third_appeared),
        std::time::SystemTime::now(),
    );
    let third_projected_row = face
        .rows()
        .iter()
        .find(|row| row.ledger_path == third_row.ledger_path)
        .expect("the third live row is present")
        .clone();

    let third_request = interrupts
        .request(&third_projected_row, RowVerb::Interrupt)
        .expect("a live row is eligible");
    let third_session_id = third_request.session_id.clone();
    let third_repo_key = third_request.repo_key.clone();
    let third_control_thread = std::thread::spawn(move || {
        ctx_traits_io::center::control_existing(
            &third_session_id,
            Some(&third_repo_key),
            ControlAction::Interrupt,
        )
    });
    let mut third_control_connection = peer.accept();
    let (third_control_id, _sid, _rk, _cmd) = third_control_connection.read_control_request();
    third_control_connection.send_control_result(&third_control_id, "refused", None);
    let third_control_result = third_control_thread
        .join()
        .expect("third control_existing thread")
        .expect("third center control request");
    assert!(matches!(third_control_result, ControlResult::Refused));
    let third_settled = interrupts.settle(
        &third_request.ledger_path,
        third_request.generation,
        RowOutcome::Control(third_control_result),
    );
    assert!(third_settled);
    assert!(matches!(
        interrupts.status(&third_request.ledger_path),
        Some(RowStatus::Refused(_))
    ));

    connection.shutdown();
    drop(updates);
}
