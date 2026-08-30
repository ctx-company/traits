//! Independent integration test for 0258.1: submitting a valid trait and
//! arguments from the desktop's spawn form reaches the center on its
//! existing-only entry, the row does not appear until a subscription delta
//! carries it, and a later disconnect/recovery cycle neither fabricates nor
//! duplicates it. Drives `support::FakePeer` by hand on two connections (the
//! snapshot/delta subscription, and a separate one for the start request/
//! response round trip — the client never pipelines two requests on one
//! connection, see `support::FakePeer`'s own doc comment). Runs alone in its
//! own target for the same process-global env reason `detail_stale_recovery.rs`
//! documents.

mod support;

use std::time::{Duration, Instant};

use camino::Utf8PathBuf;
use ctx_traits_desktop::center_link::LinkUpdate;
use ctx_traits_desktop::shell::{CenterFace, reconcile_spawn_status};
use ctx_traits_desktop::spawn_form::{SpawnForm, SubmitOutcome};
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
fn submitting_a_valid_spawn_reaches_the_center_and_the_row_appears_only_through_a_delta() {
    let guard = support::scratch("spawn-through-center");
    let root = guard.0.clone();
    // SAFETY: this test owns the whole process environment for the duration
    // of the call below (it is the only test in this target) and restores
    // nothing because the process exits with it.
    let socket = unsafe { support::install_center_env(&root) };

    let repo_a_path = root.join("repo-a");
    std::fs::create_dir_all(&repo_a_path).expect("create repo-a directory");
    let repo_a_ledger =
        Utf8PathBuf::from_path_buf(repo_a_path.join("session.json")).expect("UTF-8 path");
    support::write_session_ledger(&repo_a_ledger, "session-existing", "run-existing", false);
    let existing_row = support::row_from_ledger(
        "repo-a",
        &repo_a_ledger,
        &ctx_traits_io::run_session::read_run_session(&repo_a_ledger).expect("read fixture"),
        false,
    );

    // A row with an empty repo_path: the center could not resolve one, and
    // it must be excluded from the spawn picker's choices.
    let mut unresolved_row = support::wire_row("repo-unresolved", "/unresolved/session.json", "u");
    unresolved_row.repo_path = String::new();

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    connection.serve_snapshot(
        &subscribe_id,
        &[existing_row.clone(), unresolved_row.clone()],
    );

    let snapshot_rows = recv_snapshot(&updates);
    let mut face = CenterFace::new(ctx_traits_desktop::run_row::RepoScope::All);
    face.apply(
        LinkUpdate::Snapshot(snapshot_rows),
        std::time::SystemTime::now(),
    );

    let mut form = SpawnForm::default();
    form.set_repositories(face.repositories());
    let offered: Vec<_> = form
        .repositories()
        .iter()
        .map(|repo| repo.repo_key.as_str())
        .collect();
    assert_eq!(
        offered,
        vec!["repo-a"],
        "only the row with a resolvable repo_path is offered as a spawn target"
    );

    form.select_repository("repo-a".to_string());
    for line in ["fixture-trait", "--set", "answer=true"] {
        for ch in line.chars() {
            form.insert_char(ch);
        }
        form.newline();
    }

    // Run the process outside any repository, so a cwd-derived repo_path is
    // structurally impossible to confuse with the center-supplied one.
    let outside_any_repo = root.join("not-a-repository");
    std::fs::create_dir_all(&outside_any_repo).expect("create outside-repo scratch dir");
    std::env::set_current_dir(&outside_any_repo).expect("leave every repository");

    let request = form.submit().expect("a valid, repo-selected request");
    let generation = request.generation;
    let submit_thread = std::thread::spawn(move || {
        ctx_traits_io::center::start_trait_existing(
            &request.args,
            camino::Utf8Path::new(&request.repo_path),
        )
    });

    let mut start_connection = peer.accept();
    let (start_id, args, repo_path) = start_connection.read_start_request();
    assert_eq!(
        args,
        vec!["fixture-trait", "--set", "answer=true"],
        "the argv must be exactly the user's lines — no traits/run/--progress injected client-side"
    );
    // `support::row_from_ledger` derives its fixture `repo_path` as
    // `/{repo_key}` (see its doc comment); the point under test is that this
    // exact center-supplied string reaches the wire, not the process cwd
    // (`outside_any_repo`) and not `repo_a_path`.
    assert_eq!(
        repo_path, "/repo-a",
        "the repository must come from center state, never the process cwd"
    );
    start_connection.send_started(&start_id, "session-new");

    let result = submit_thread
        .join()
        .expect("start_trait_existing thread")
        .expect("center start request");
    assert!(matches!(
        result,
        ctx_traits_io::center::StartResult::Started { .. }
    ));

    let changed = form.settle(
        generation,
        SubmitOutcome::Started {
            session_id: "session-new".to_string(),
        },
    );
    assert!(changed);
    assert!(matches!(
        form.status(),
        ctx_traits_desktop::spawn_form::SpawnStatus::Requested { session_id }
        if session_id == "session-new"
    ));
    assert_eq!(
        face.rows().len(),
        2,
        "the row does not exist until a delta carries it — only the two snapshot rows are present"
    );

    let new_ledger = Utf8PathBuf::from_path_buf(repo_a_path.join("new-session.json"))
        .expect("UTF-8 new ledger path");
    support::write_session_ledger(&new_ledger, "session-new", "run-new", true);
    let new_session =
        ctx_traits_io::run_session::read_run_session(&new_ledger).expect("read new ledger");
    let new_row = support::row_from_ledger("repo-a", &new_ledger, &new_session, true);
    connection.send_delta(&CenterDelta::Appeared {
        row: Box::new(new_row.clone()),
    });

    // Ordering A: the `Started` response settles first (already reached
    // `Requested` above), then the `Appeared` delta arrives and is applied
    // to the face before reconciliation runs — the same order the main
    // update loop uses.
    let delta = recv_delta(&updates);
    face.apply(LinkUpdate::Delta(delta), std::time::SystemTime::now());
    assert!(reconcile_spawn_status(&face, &mut form));
    assert!(matches!(
        form.status(),
        ctx_traits_desktop::spawn_form::SpawnStatus::Idle
    ));
    assert_eq!(
        face.rows().len(),
        3,
        "the spawned row now appears exactly once"
    );
    let spawned = face
        .rows()
        .iter()
        .find(|row| row.session_id == "session-new")
        .expect("the spawned row is present with the center's identity");
    assert_eq!(spawned.repo_key, "repo-a");
    assert_eq!(spawned.ledger_path, new_ledger.as_str());

    // A disconnect goes Stale without losing or duplicating the spawned row,
    // and a recovery snapshot does not fabricate a second copy of it.
    connection.shutdown();
    let reason = recv_down(&updates);
    face.apply(LinkUpdate::Down(reason), std::time::SystemTime::now());
    assert!(face.is_stale());
    assert_eq!(
        face.rows().len(),
        3,
        "the spawned row stays visible, labelled stale, across the outage"
    );

    let mut recovery_connection = peer.accept();
    let recovery_subscribe_id = recovery_connection.read_subscribe_id();
    recovery_connection.serve_snapshot(
        &recovery_subscribe_id,
        &[existing_row, unresolved_row, new_row],
    );
    let recovery_rows = recv_snapshot(&updates);
    face.apply(
        LinkUpdate::Snapshot(recovery_rows),
        std::time::SystemTime::now(),
    );
    assert!(!face.is_stale());
    assert_eq!(
        face.rows().len(),
        3,
        "the recovery snapshot must not fabricate a duplicate of the spawned row"
    );

    // Ordering B: the `Appeared` delta wins the race and is applied to the
    // face before this second request's own `Started` response settles.
    // `reconcile_spawn_status` must not fire while the form is still
    // `Requesting` (the session isn't known yet), but must fire the instant
    // `settle` reaches `Requested`, since the row is already visible by
    // then.
    form.select_repository("repo-a".to_string());
    for ch in "fixture-trait-2".chars() {
        form.insert_char(ch);
    }
    let second_request = form
        .submit()
        .expect("a second valid, repo-selected request");
    let second_generation = second_request.generation;
    let second_submit_thread = std::thread::spawn(move || {
        ctx_traits_io::center::start_trait_existing(
            &second_request.args,
            camino::Utf8Path::new(&second_request.repo_path),
        )
    });

    let mut second_start_connection = peer.accept();
    let (second_start_id, _args, _repo_path) = second_start_connection.read_start_request();

    let second_ledger = Utf8PathBuf::from_path_buf(repo_a_path.join("second-session.json"))
        .expect("UTF-8 second ledger path");
    support::write_session_ledger(&second_ledger, "session-second", "run-second", true);
    let second_session =
        ctx_traits_io::run_session::read_run_session(&second_ledger).expect("read second ledger");
    let second_row = support::row_from_ledger("repo-a", &second_ledger, &second_session, true);
    recovery_connection.send_delta(&CenterDelta::Appeared {
        row: Box::new(second_row),
    });
    let second_delta = recv_delta(&updates);
    face.apply(
        LinkUpdate::Delta(second_delta),
        std::time::SystemTime::now(),
    );
    assert_eq!(
        face.rows().len(),
        4,
        "the second spawned row is visible before its own Started response settles"
    );
    assert!(
        !reconcile_spawn_status(&face, &mut form),
        "reconciliation must not fire while the form is still Requesting, not Requested"
    );

    second_start_connection.send_started(&second_start_id, "session-second");
    let second_result = second_submit_thread
        .join()
        .expect("second start_trait_existing thread")
        .expect("second center start request");
    assert!(matches!(
        second_result,
        ctx_traits_io::center::StartResult::Started { .. }
    ));

    let settled = form.settle(
        second_generation,
        SubmitOutcome::Started {
            session_id: "session-second".to_string(),
        },
    );
    assert!(
        settled,
        "settle itself reports the Requesting -> Requested change"
    );
    assert!(
        reconcile_spawn_status(&face, &mut form),
        "the session was already visible when Started arrived, so reconciliation must fire at settle time"
    );
    assert!(matches!(
        form.status(),
        ctx_traits_desktop::spawn_form::SpawnStatus::Idle
    ));

    recovery_connection.shutdown();
    drop(updates);
}
