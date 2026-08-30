//! Independent integration test for 0258.3: requesting a pause for a live
//! row, and resuming a paused row, both reach the center on its
//! existing-only entries, with the exact wire shape the center expects, and
//! the displayed outcome resolves only once the row's own subscription
//! delta carries the effect — never from the response's own acknowledgement
//! alone. Runs alone in its own target for the same process-global env
//! reason `interrupt_through_center.rs` documents (it calls
//! `support::install_center_env`, which owns process-global env) — and,
//! because this target carries several `#[test]` functions rather than
//! `interrupt_through_center.rs`'s one, each also holds `ENV_LOCK` for its
//! whole body: Rust runs a target's tests on separate threads by default, and
//! two tests racing `install_center_env`'s process-global mutation at once
//! is the same hazard the single-test convention exists to avoid.

mod support;

use std::sync::Mutex;
use std::time::{Duration, Instant};

static ENV_LOCK: Mutex<()> = Mutex::new(());

use camino::Utf8PathBuf;
use ctx_traits_desktop::center_link::LinkUpdate;
use ctx_traits_desktop::row_control::{self, RowControls, RowOutcome, RowStatus, RowVerb};
use ctx_traits_desktop::run_row::{RepoScope, RowState};
use ctx_traits_desktop::shell::{CenterFace, reconcile_row_controls};
use ctx_traits_io::center::{CenterDelta, ControlResult, StartResult};

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

/// Pause: sends exactly `{"kind":"control", command:"pause"}` with the
/// center-supplied `session_id`/`repo_key`; the acknowledgement alone
/// leaves the status at *requested* and does not mark the row paused; the
/// `RowChanged` delta carrying `last_drive_outcome: "paused"` is what flips
/// the projection to `RowState::Paused` and clears the pending entry.
#[test]
fn pausing_a_live_row_reaches_the_center_and_resolves_only_through_a_delta() {
    let _serial = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let guard = support::scratch("pause-through-center");
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
    let mut face = CenterFace::new(RepoScope::All);
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
    assert_eq!(row.state, RowState::Live);

    let mut controls = RowControls::default();
    let request = controls
        .request(&row, RowVerb::Pause)
        .expect("a live row is eligible for pause");
    assert_eq!(request.session_id, "session-live");
    assert_eq!(request.repo_key, "repo-a");

    let request_for_dispatch = request.clone();
    let dispatch_thread = std::thread::spawn(move || row_control::dispatch(&request_for_dispatch));

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
        wire_command, "pause",
        "the wire command must be the shared pause verb, never a GUI-specific one"
    );
    control_connection.send_control_acknowledged(&control_id);

    let outcome = dispatch_thread.join().expect("dispatch thread");
    assert!(matches!(
        outcome,
        RowOutcome::Control(ControlResult::Acknowledged)
    ));

    let settled = controls.settle(&request.ledger_path, request.generation, outcome);
    assert!(settled);
    assert!(matches!(
        controls.status(&request.ledger_path),
        Some(RowStatus::Requested {
            verb: RowVerb::Pause,
            ..
        })
    ));

    // Acknowledgement alone must not resolve anything: the row must still
    // be live in the face, and the status must not read "paused".
    assert!(
        !reconcile_row_controls(&face, &mut controls),
        "acknowledgement alone must not resolve a pending pause"
    );
    assert!(
        face.rows()
            .iter()
            .find(|row| row.ledger_path == ledger.as_str())
            .expect("row still present")
            .live,
        "the row must stay live until the center's own delta carries the effect"
    );

    let mut paused_row = live_row.clone();
    paused_row.live = false;
    paused_row.summary.last_drive_outcome = Some("paused".to_string());
    connection.send_delta(&CenterDelta::RowChanged {
        row: Box::new(paused_row),
    });
    let delta = recv_delta(&updates);
    face.apply(LinkUpdate::Delta(delta), std::time::SystemTime::now());

    assert!(
        reconcile_row_controls(&face, &mut controls),
        "reconciliation must resolve the pending pause once the delta lands"
    );
    assert!(controls.status(&request.ledger_path).is_none());
    let projected = face
        .rows()
        .iter()
        .find(|row| row.ledger_path == ledger.as_str())
        .expect("row still present");
    assert_eq!(
        projected.state,
        RowState::Paused,
        "the row must now project as paused, driven only by the delta"
    );

    connection.shutdown();
    drop(updates);
}

/// Resume: with a fresh `CenterFace` and a fresh, empty `RowControls` — the
/// GUI-restart case, no prior in-memory state — a snapshot carrying only a
/// paused row is resume-eligible, dispatching resume sends exactly
/// `{"kind":"start", target:{type:"session"}}`, and the pending entry
/// resolves only on the `live: true` delta, never on the `Started`
/// response alone.
#[test]
fn resuming_a_paused_row_reaches_the_center_and_resolves_only_through_a_delta() {
    let _serial = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let guard = support::scratch("resume-through-center");
    let root = guard.0.clone();
    // SAFETY: same single-test-per-process-env justification as above.
    let socket = unsafe { support::install_center_env(&root) };

    let outside_any_repo = root.join("not-a-repository");
    std::fs::create_dir_all(&outside_any_repo).expect("create outside-repo scratch dir");
    std::env::set_current_dir(&outside_any_repo).expect("leave every repository");

    let repo_a_path = root.join("repo-a");
    std::fs::create_dir_all(&repo_a_path).expect("create repo-a directory");
    let ledger = Utf8PathBuf::from_path_buf(repo_a_path.join("session.json")).expect("UTF-8 path");
    support::write_session_ledger(&ledger, "session-paused", "run-paused", false);
    support::pause_session(&ledger);
    let session = ctx_traits_io::run_session::read_run_session(&ledger).expect("read fixture");
    let paused_row = support::row_from_ledger("repo-a", &ledger, &session, false);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    connection.serve_snapshot(&subscribe_id, std::slice::from_ref(&paused_row));

    let snapshot_rows = recv_snapshot(&updates);
    // A fresh face and a fresh, empty RowControls: no prior in-memory
    // state, exactly the state after a GUI restart.
    let mut face = CenterFace::new(RepoScope::All);
    face.apply(
        LinkUpdate::Snapshot(snapshot_rows),
        std::time::SystemTime::now(),
    );
    let mut controls = RowControls::default();

    let row = face
        .rows()
        .iter()
        .find(|row| row.ledger_path == ledger.as_str())
        .expect("the paused row is present after the snapshot")
        .clone();
    assert_eq!(row.state, RowState::Paused);
    assert!(row.can_resume(), "a paused row must be resume-eligible");

    let request = controls
        .request(&row, RowVerb::Resume)
        .expect("a paused row is eligible for resume");
    assert_eq!(request.session_id, "session-paused");
    assert_eq!(request.repo_key, "repo-a");

    let request_for_dispatch = request.clone();
    let dispatch_thread = std::thread::spawn(move || row_control::dispatch(&request_for_dispatch));

    let mut start_connection = peer.accept();
    let (start_id, wire_session_id, wire_repo_key) = start_connection.read_session_start_request();
    assert_eq!(
        wire_session_id, "session-paused",
        "the session id must be exactly the one the center supplied"
    );
    assert_eq!(
        wire_repo_key.as_deref(),
        Some("repo-a"),
        "the repo key must be exactly the one the center supplied"
    );
    start_connection.send_started(&start_id, "session-paused");

    let outcome = dispatch_thread.join().expect("dispatch thread");
    assert!(matches!(
        outcome,
        RowOutcome::Start(StartResult::Started { .. })
    ));

    let settled = controls.settle(&request.ledger_path, request.generation, outcome);
    assert!(settled);
    assert!(matches!(
        controls.status(&request.ledger_path),
        Some(RowStatus::Requested {
            verb: RowVerb::Resume,
            ..
        })
    ));

    // A Started response alone must not resolve anything: the row must
    // still project as non-live until the center's own delta arrives.
    assert!(
        !reconcile_row_controls(&face, &mut controls),
        "a Started response alone must not resolve a pending resume"
    );
    assert!(
        !face
            .rows()
            .iter()
            .find(|row| row.ledger_path == ledger.as_str())
            .expect("row still present")
            .live,
        "the row must stay non-live until the center's own delta carries the effect"
    );

    let mut resumed_row = paused_row.clone();
    resumed_row.live = true;
    resumed_row.summary.last_drive_outcome = None;
    connection.send_delta(&CenterDelta::RowChanged {
        row: Box::new(resumed_row),
    });
    let delta = recv_delta(&updates);
    face.apply(LinkUpdate::Delta(delta), std::time::SystemTime::now());

    assert!(
        reconcile_row_controls(&face, &mut controls),
        "reconciliation must resolve the pending resume once the live delta lands"
    );
    assert!(controls.status(&request.ledger_path).is_none());
    assert!(
        face.rows()
            .iter()
            .find(|row| row.ledger_path == ledger.as_str())
            .expect("row still present")
            .live,
        "the row must now show as live again"
    );

    connection.shutdown();
    drop(updates);
}

/// The other ordering for resume: the row's own `RowChanged { live: true }`
/// delta wins the race and lands before the `Started` response settles —
/// reconciliation must not fire on the delta alone (the entry is still
/// `Requesting`, not `Requested`), and must fire once `settle` runs.
#[test]
fn resume_delta_before_response_resolves_at_settle_time() {
    let _serial = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let guard = support::scratch("resume-delta-first");
    let root = guard.0.clone();
    // SAFETY: same single-test-per-process-env justification as above.
    let socket = unsafe { support::install_center_env(&root) };

    let outside_any_repo = root.join("not-a-repository");
    std::fs::create_dir_all(&outside_any_repo).expect("create outside-repo scratch dir");
    std::env::set_current_dir(&outside_any_repo).expect("leave every repository");

    let repo_a_path = root.join("repo-a");
    std::fs::create_dir_all(&repo_a_path).expect("create repo-a directory");
    let ledger = Utf8PathBuf::from_path_buf(repo_a_path.join("session.json")).expect("UTF-8 path");
    support::write_session_ledger(&ledger, "session-paused", "run-paused", false);
    support::pause_session(&ledger);
    let session = ctx_traits_io::run_session::read_run_session(&ledger).expect("read fixture");
    let paused_row = support::row_from_ledger("repo-a", &ledger, &session, false);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    connection.serve_snapshot(&subscribe_id, std::slice::from_ref(&paused_row));

    let snapshot_rows = recv_snapshot(&updates);
    let mut face = CenterFace::new(RepoScope::All);
    face.apply(
        LinkUpdate::Snapshot(snapshot_rows),
        std::time::SystemTime::now(),
    );
    let mut controls = RowControls::default();

    let row = face
        .rows()
        .iter()
        .find(|row| row.ledger_path == ledger.as_str())
        .expect("the paused row is present")
        .clone();
    let request = controls
        .request(&row, RowVerb::Resume)
        .expect("a paused row is eligible for resume");

    let request_for_dispatch = request.clone();
    let dispatch_thread = std::thread::spawn(move || row_control::dispatch(&request_for_dispatch));

    let mut start_connection = peer.accept();
    let (start_id, _sid, _rk) = start_connection.read_session_start_request();

    // The row's own live delta lands first, before the Started response.
    let mut resumed_row = paused_row.clone();
    resumed_row.live = true;
    resumed_row.summary.last_drive_outcome = None;
    connection.send_delta(&CenterDelta::RowChanged {
        row: Box::new(resumed_row),
    });
    let delta = recv_delta(&updates);
    face.apply(LinkUpdate::Delta(delta), std::time::SystemTime::now());
    assert!(
        !reconcile_row_controls(&face, &mut controls),
        "reconciliation must not fire while the entry is still Requesting, not Requested"
    );

    start_connection.send_started(&start_id, "session-paused");
    let outcome = dispatch_thread.join().expect("dispatch thread");
    let settled = controls.settle(&request.ledger_path, request.generation, outcome);
    assert!(settled);
    assert!(
        reconcile_row_controls(&face, &mut controls),
        "the row was already live when the response arrived, so reconciliation must fire at settle time"
    );
    assert!(controls.status(&request.ledger_path).is_none());

    connection.shutdown();
    drop(updates);
}

/// A `Down` while a resume is pending must not resolve it: the `Stale`
/// face holds no fresh evidence, and only a fresh model can distinguish
/// "row ended" from "we haven't reconnected yet".
#[test]
fn a_down_while_a_resume_is_pending_does_not_resolve_it() {
    let _serial = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let guard = support::scratch("resume-down");
    let root = guard.0.clone();
    // SAFETY: same single-test-per-process-env justification as above.
    let socket = unsafe { support::install_center_env(&root) };

    let outside_any_repo = root.join("not-a-repository");
    std::fs::create_dir_all(&outside_any_repo).expect("create outside-repo scratch dir");
    std::env::set_current_dir(&outside_any_repo).expect("leave every repository");

    let repo_a_path = root.join("repo-a");
    std::fs::create_dir_all(&repo_a_path).expect("create repo-a directory");
    let ledger = Utf8PathBuf::from_path_buf(repo_a_path.join("session.json")).expect("UTF-8 path");
    support::write_session_ledger(&ledger, "session-paused", "run-paused", false);
    support::pause_session(&ledger);
    let session = ctx_traits_io::run_session::read_run_session(&ledger).expect("read fixture");
    let paused_row = support::row_from_ledger("repo-a", &ledger, &session, false);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    connection.serve_snapshot(&subscribe_id, std::slice::from_ref(&paused_row));

    let snapshot_rows = recv_snapshot(&updates);
    let mut face = CenterFace::new(RepoScope::All);
    face.apply(
        LinkUpdate::Snapshot(snapshot_rows),
        std::time::SystemTime::now(),
    );
    let mut controls = RowControls::default();

    let row = face
        .rows()
        .iter()
        .find(|row| row.ledger_path == ledger.as_str())
        .expect("the paused row is present")
        .clone();
    let request = controls
        .request(&row, RowVerb::Resume)
        .expect("a paused row is eligible for resume");
    controls.settle(
        &request.ledger_path,
        request.generation,
        RowOutcome::Start(StartResult::Started {
            session_id: "session-paused".to_string(),
        }),
    );
    assert!(matches!(
        controls.status(&request.ledger_path),
        Some(RowStatus::Requested { .. })
    ));

    connection.shutdown();
    let down = recv_delta_or_down(&updates);
    face.apply(down, std::time::SystemTime::now());

    assert!(
        !reconcile_row_controls(&face, &mut controls),
        "a Down alone must not resolve a pending resume"
    );
    assert!(matches!(
        controls.status(&request.ledger_path),
        Some(RowStatus::Requested { .. })
    ));

    drop(updates);
}

fn recv_delta_or_down(updates: &async_channel::Receiver<LinkUpdate>) -> LinkUpdate {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match updates.try_recv() {
            Ok(update @ LinkUpdate::Down(_)) => return update,
            Ok(_) => {}
            Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("no Down update arrived before the deadline: {error}"),
        }
    }
}

/// A refused/exited outcome for either verb renders a distinct, non-success
/// message rather than resolving the entry as if it had succeeded.
#[test]
fn refused_and_exited_outcomes_render_distinct_non_success_messages() {
    let _serial = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let guard = support::scratch("pause-resume-refusals");
    let root = guard.0.clone();
    // SAFETY: same single-test-per-process-env justification as above.
    let socket = unsafe { support::install_center_env(&root) };

    let outside_any_repo = root.join("not-a-repository");
    std::fs::create_dir_all(&outside_any_repo).expect("create outside-repo scratch dir");
    std::env::set_current_dir(&outside_any_repo).expect("leave every repository");

    let repo_a_path = root.join("repo-a");
    std::fs::create_dir_all(&repo_a_path).expect("create repo-a directory");

    let live_ledger =
        Utf8PathBuf::from_path_buf(repo_a_path.join("live-session.json")).expect("UTF-8 path");
    support::write_session_ledger(&live_ledger, "session-live", "run-live", true);
    let live_session =
        ctx_traits_io::run_session::read_run_session(&live_ledger).expect("read fixture");
    let live_row = support::row_from_ledger("repo-a", &live_ledger, &live_session, true);

    let paused_ledger =
        Utf8PathBuf::from_path_buf(repo_a_path.join("paused-session.json")).expect("UTF-8 path");
    support::write_session_ledger(&paused_ledger, "session-paused", "run-paused", false);
    support::pause_session(&paused_ledger);
    let paused_session =
        ctx_traits_io::run_session::read_run_session(&paused_ledger).expect("read fixture");
    let paused_row = support::row_from_ledger("repo-a", &paused_ledger, &paused_session, false);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    connection.serve_snapshot(&subscribe_id, &[live_row, paused_row]);

    let snapshot_rows = recv_snapshot(&updates);
    let mut face = CenterFace::new(RepoScope::All);
    face.apply(
        LinkUpdate::Snapshot(snapshot_rows),
        std::time::SystemTime::now(),
    );
    let mut controls = RowControls::default();

    // Pause refusal.
    let live = face
        .rows()
        .iter()
        .find(|row| row.ledger_path == live_ledger.as_str())
        .expect("live row present")
        .clone();
    let pause_request = controls
        .request(&live, RowVerb::Pause)
        .expect("a live row is eligible for pause");
    let pause_request_for_dispatch = pause_request.clone();
    let pause_thread =
        std::thread::spawn(move || row_control::dispatch(&pause_request_for_dispatch));
    let mut pause_connection = peer.accept();
    let (pause_id, _sid, _rk, _cmd) = pause_connection.read_control_request();
    pause_connection.send_control_result(&pause_id, "refused", None);
    let pause_outcome = pause_thread.join().expect("pause dispatch thread");
    controls.settle(
        &pause_request.ledger_path,
        pause_request.generation,
        pause_outcome,
    );
    let pause_status = controls
        .status(&pause_request.ledger_path)
        .expect("pause entry present");
    let RowStatus::Refused(pause_message) = pause_status else {
        panic!("expected Refused, got {pause_status:?}");
    };
    let pause_message = pause_message.clone();
    assert!(!pause_message.is_empty());

    // Resume exited.
    let paused = face
        .rows()
        .iter()
        .find(|row| row.ledger_path == paused_ledger.as_str())
        .expect("paused row present")
        .clone();
    let resume_request = controls
        .request(&paused, RowVerb::Resume)
        .expect("a paused row is eligible for resume");
    let resume_request_for_dispatch = resume_request.clone();
    let resume_thread =
        std::thread::spawn(move || row_control::dispatch(&resume_request_for_dispatch));
    let mut resume_connection = peer.accept();
    let (resume_id, _sid, _rk) = resume_connection.read_session_start_request();
    resume_connection.send_start_exited(&resume_id, Some(1), "boom");
    let resume_outcome = resume_thread.join().expect("resume dispatch thread");
    controls.settle(
        &resume_request.ledger_path,
        resume_request.generation,
        resume_outcome,
    );
    let resume_status = controls
        .status(&resume_request.ledger_path)
        .expect("resume entry present");
    let RowStatus::Refused(resume_message) = resume_status else {
        panic!("expected Refused, got {resume_status:?}");
    };
    assert!(resume_message.contains("boom"));
    let resume_message = resume_message.clone();
    assert_ne!(
        pause_message, resume_message,
        "the two refusal messages must be distinct, never a generic shared string"
    );

    connection.shutdown();
    drop(updates);
}
