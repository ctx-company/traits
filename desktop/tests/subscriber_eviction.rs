//! Independent integration test proving the third degraded-lifecycle case
//! (0256.5): server-side backpressure eviction of a subscriber whose bounded
//! queue overflowed. `modules/io/src/center.rs:3344` defines eviction to end
//! in `shutdown(Both)` with no close frame — the same wire shape as any
//! other stream end — so the client cannot and must not distinguish it from
//! a center exit; both recover through the same reconnect-and-resnapshot
//! path. The io-side proof that eviction *does* shut the socket down is
//! `subscription_snapshot_precedes_later_deltas_and_backpressure_disconnects`
//! in `modules/io/src/center.rs`; this test supplies the client-side half.
//!
//! This is deliberately not a full-stack eviction reproduction: the link
//! drains the socket into an unbounded channel and so cannot be evicted for
//! its own slowness (see `desktop/README.md`). It instead reproduces the
//! wire shape eviction is defined to produce. Runs alone in its own target
//! for the same process-global env reason `absent_center.rs` documents.

mod support;

use std::time::{Duration, Instant, SystemTime};

use ctx_traits_desktop::center_link::LinkUpdate;
use ctx_traits_desktop::run_row::RepoScope;
use ctx_traits_desktop::shell::{CenterFace, CenterState};
use ctx_traits_io::center::CenterDelta;

// `SUBSCRIBER_QUEUE` in `modules/io/src/center.rs` is private; 64 is its
// current value. The exact count only needs to be "queue-scale", not exact.
const SUBSCRIBER_QUEUE_SCALE: usize = 64;

#[test]
fn a_backlog_then_shutdown_is_reported_as_a_lost_subscription_and_recovers() {
    let guard = support::scratch("subscriber-eviction");
    let root = guard.0.clone();
    // SAFETY: this test owns the whole process environment for the duration
    // of the call below (it is the only test in this target) and restores
    // nothing because the process exits with it.
    let socket = unsafe { support::install_center_env(&root) };

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let row = support::wire_row("repo", "/repo/session.json", "run-a");
    connection.serve_snapshot(&subscribe_id, std::slice::from_ref(&row));
    for _ in 0..SUBSCRIBER_QUEUE_SCALE {
        connection.send_delta(&CenterDelta::RowChanged {
            row: Box::new(support::wire_row("repo", "/repo/session.json", "run-a")),
        });
    }
    // No close frame: exactly the wire shape a server-side backpressure
    // eviction produces.
    connection.shutdown();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut face = CenterFace::new(RepoScope::All);
    let mut saw_snapshot = false;
    let reason = loop {
        match updates.try_recv() {
            Ok(LinkUpdate::Snapshot(rows)) => {
                saw_snapshot = true;
                face.apply(LinkUpdate::Snapshot(rows), SystemTime::now());
            }
            Ok(update @ LinkUpdate::Delta(_)) => face.apply(update, SystemTime::now()),
            Ok(LinkUpdate::Down(reason)) => break reason,
            Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("the loss was never reported before the deadline: {error}"),
        }
    };
    assert!(saw_snapshot, "the initial snapshot must have arrived first");
    face.apply(LinkUpdate::Down(reason), SystemTime::now());
    assert!(
        matches!(face.state(), CenterState::Stale { .. }),
        "a backlog followed by a bare shutdown must present as a lost, recoverable subscription"
    );

    // The next subscription's snapshot is what restores state, exactly as
    // for any other stream end.
    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let recovered_row = support::wire_row("repo", "/repo/session.json", "run-a");
    connection.serve_snapshot(&subscribe_id, std::slice::from_ref(&recovered_row));

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match updates.try_recv() {
            Ok(LinkUpdate::Snapshot(rows)) => {
                face.apply(LinkUpdate::Snapshot(rows), SystemTime::now());
                break;
            }
            Ok(other) => face.apply(other, SystemTime::now()),
            Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("no recovery snapshot arrived before the deadline: {error}"),
        }
    }
    assert!(matches!(face.state(), CenterState::Connected { .. }));

    connection.shutdown();
    drop(updates);
}
