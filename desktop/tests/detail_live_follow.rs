//! Independent integration proof for 0257.3's core claim: an open detail
//! view stays current as frames land, through a real in-process center (the
//! `tests/live_deltas.rs` pattern), with no polling and exactly one ledger
//! re-read per observed frame transition — plus the restart-identity half of
//! the task's "done when": a freshly constructed `RunDetail` reconstructs the
//! same tree from the ledger alone, without disturbing the ledger it read.
//! Runs alone in its own target for the same process-global env reason
//! `live_deltas.rs` documents.

mod support;

use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use camino::Utf8PathBuf;
use ctx_traits_core::procedure::activity::SessionState;
use ctx_traits_desktop::center_link::LinkUpdate;
use ctx_traits_desktop::detail::{self, RunDetail};
use ctx_traits_desktop::run_row;

fn await_socket(socket: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match UnixStream::connect(socket) {
            Ok(_) => return,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => panic!("center did not become ready at {socket:?}: {error}"),
        }
    }
}

/// Drive every arriving `LinkUpdate` through `follow`, running each returned
/// `LoadRequest` through `detail::load` exactly as `Shell::spawn_load` does,
/// until the tree has exactly `target_roots` nodes. Returns the number of
/// load requests observed during this call, so a caller can assert the
/// transition cost exactly one re-read rather than bounding the whole run's
/// total from a distance.
fn drive_until_len(
    detail: &mut RunDetail,
    updates: &async_channel::Receiver<LinkUpdate>,
    target_roots: usize,
) -> usize {
    let mut requests = 0usize;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if detail.tree().map(|tree| tree.roots.len()) == Some(target_roots) {
            return requests;
        }
        if Instant::now() >= deadline {
            panic!(
                "the tree never reached {target_roots} nodes before the deadline (currently {:?})",
                detail.tree().map(|tree| tree.roots.len())
            );
        }
        match updates.try_recv() {
            Ok(LinkUpdate::Snapshot(rows)) => {
                panic!(
                    "a second snapshot arrived; the stream must stay exclusive: {} rows",
                    rows.len()
                );
            }
            Ok(LinkUpdate::Down(message)) => panic!("center link reported down: {message}"),
            Ok(update @ LinkUpdate::Delta(_)) => {
                let outcome = detail.follow(&update);
                if let Some(request) = outcome.request {
                    requests += 1;
                    let outcome = detail::load(&request);
                    assert!(detail.apply(request.generation, outcome));
                }
            }
            Err(async_channel::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("link channel closed before the tree converged: {error}"),
        }
    }
}

/// Drive updates until the selection's projected header reports `expected`,
/// running every returned `LoadRequest` through `detail::load`. Used for the
/// terminal transition, which changes the header without necessarily
/// changing the node count.
fn drive_until_header(
    detail: &mut RunDetail,
    updates: &async_channel::Receiver<LinkUpdate>,
    expected: SessionState,
) -> usize {
    let mut requests = 0usize;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if detail
            .tree()
            .is_some_and(|tree| tree.header.run_state == expected)
        {
            return requests;
        }
        if Instant::now() >= deadline {
            panic!(
                "the header never reached {expected:?} before the deadline (currently {:?})",
                detail.tree().map(|tree| tree.header.run_state)
            );
        }
        match updates.try_recv() {
            Ok(LinkUpdate::Snapshot(rows)) => {
                panic!(
                    "a second snapshot arrived; the stream must stay exclusive: {} rows",
                    rows.len()
                );
            }
            Ok(LinkUpdate::Down(message)) => panic!("center link reported down: {message}"),
            Ok(update @ LinkUpdate::Delta(_)) => {
                let outcome = detail.follow(&update);
                if let Some(request) = outcome.request {
                    requests += 1;
                    let outcome = detail::load(&request);
                    assert!(detail.apply(request.generation, outcome));
                }
            }
            Err(async_channel::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("link channel closed before the header converged: {error}"),
        }
    }
}

#[test]
fn an_open_detail_view_grows_as_frames_land_with_bounded_re_reads_and_survives_a_restart() {
    let guard = support::scratch("detail-live-follow");
    let root = guard.0.clone();
    let socket = root.join("center.sock");

    let ledger_path =
        Utf8PathBuf::from_path_buf(root.join("repo").join("session.json")).expect("UTF-8 path");
    support::write_session_ledger(&ledger_path, "session-a", "run-a", true);
    // `write_session_ledger` writes no frames; land the first one up front so
    // the seed baseline already has one node, exactly like a run selected
    // mid-flight.
    support::append_landed_frame(&ledger_path, "frame-one", "Frame one");

    // SAFETY: this test owns the whole process environment for the duration
    // of the call below (it is the only test in this target) and restores
    // nothing because the process exits with it.
    unsafe {
        std::env::set_var("CTX_CENTER_SOCKET", &socket);
        std::env::set_var("CTX_CENTER_SPAWN_LOCK", root.join("center.lock"));
        std::env::set_var("CTX_CENTER_RUNS_ROOT", &root);
        std::env::set_var("CTX_CENTER_INDEX", root.join("index.sqlite3"));
        std::env::set_var("CTX_CENTER_LIVENESS_ROOT", root.join("liveness"));
        std::env::set_var("CTX_CENTER_SCAN_MS", "20");
        std::env::set_var("CTX_CENTER_IDLE_MS", "30000");
        std::env::set_var("HOME", &root);
    }

    std::thread::spawn(|| {
        let _ = ctx_traits_io::center::run_server();
    });
    await_socket(&socket);

    let updates = ctx_traits_desktop::center_link::start(None);
    let deadline = Instant::now() + Duration::from_secs(10);
    let snapshot_rows = loop {
        match updates.try_recv() {
            Ok(LinkUpdate::Snapshot(rows)) => break rows,
            Ok(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(_) => panic!("snapshot never arrived before the deadline"),
            Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("no snapshot arrived before the deadline: {error}"),
        }
    };

    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);
    let row = rows
        .iter()
        .find(|row| row.ledger_path == ledger_path.as_str())
        .expect("seeded row present in the first snapshot");

    let mut detail = RunDetail::default();
    let seed_request = detail
        .select(row)
        .expect("first selection issues a request");
    let mut request_count = 1usize;
    let seed_outcome = detail::load(&seed_request);
    assert!(detail.apply(seed_request.generation, seed_outcome));
    assert_eq!(
        detail.tree().unwrap().roots.len(),
        1,
        "the seed baseline's one frame"
    );

    // The center's own wire fingerprint (`modified_epoch_secs`) is
    // 1-second resolution (documented in `detail.rs`'s `Fingerprint`), so
    // each write is spaced past a full second or the center itself would
    // fold two same-second writes into one observed change.
    std::thread::sleep(Duration::from_millis(1100));
    support::append_landed_frame(&ledger_path, "frame-two", "Frame two");
    let requests_for_frame_two = drive_until_len(&mut detail, &updates, 2);
    assert_eq!(
        requests_for_frame_two, 1,
        "landing frame-two must cost exactly one re-read, not one per delta"
    );
    request_count += requests_for_frame_two;

    std::thread::sleep(Duration::from_millis(1100));
    support::append_landed_frame(&ledger_path, "frame-three", "Frame three");
    let requests_for_frame_three = drive_until_len(&mut detail, &updates, 3);
    assert_eq!(
        requests_for_frame_three, 1,
        "landing frame-three must cost exactly one re-read, not one per delta"
    );
    request_count += requests_for_frame_three;

    // Representative sidecar evidence, appended before the terminal ledger
    // write — activity is recorded during a frame, the ledger is written at
    // the frame boundary.
    support::append_narration(&ledger_path, "frame-three", "wrapping up");
    std::thread::sleep(Duration::from_millis(1100));
    support::complete_session(&ledger_path);
    let requests_for_terminal = drive_until_header(&mut detail, &updates, SessionState::Completed);
    assert_eq!(
        requests_for_terminal, 1,
        "the terminal transition must cost exactly one re-read"
    );
    request_count += requests_for_terminal;

    assert_eq!(
        request_count, 4,
        "1 seed + 1 per observed frame transition + 1 terminal transition, never one per delta"
    );

    let final_tree = detail.tree().expect("the converged, terminal tree");
    assert_eq!(final_tree.roots.len(), 3, "no frame was lost or duplicated");
    assert_eq!(
        final_tree.roots[2].narration.as_deref(),
        Some("wrapping up"),
        "the appended narration must attach to the frame it named"
    );

    let terminal_bytes = std::fs::read(ledger_path.as_std_path()).expect("read terminal ledger");

    // Restart identity: a fresh `RunDetail` reconstructs the same tree from
    // the ledger alone, with no live stream in the loop, and observing it
    // must not have mutated the ledger it read.
    let mut restarted = RunDetail::default();
    let restart_request = restarted
        .select(row)
        .expect("selection after restart issues a request");
    let restart_outcome = detail::load(&restart_request);
    assert!(restarted.apply(restart_request.generation, restart_outcome));
    let restarted_tree = restarted.tree().expect("the restarted tree");

    assert_eq!(
        restarted_tree, final_tree,
        "the restarted tree must equal the live-followed, converged tree exactly"
    );
    assert_eq!(
        std::fs::read(ledger_path.as_std_path()).expect("read ledger after restart read"),
        terminal_bytes,
        "reconstructing detail from the ledger must not change it"
    );

    drop(updates);
}
