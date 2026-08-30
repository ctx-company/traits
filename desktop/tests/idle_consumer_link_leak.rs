//! Independent integration test proving the idle-consumer regression fix
//! (0256.5, review round 1): dropping the UI receiver while the subscription
//! is connected but idle — no event in flight, the peer just silent — must
//! still end the link thread and drop the subscription within a bounded
//! interval, rather than blocking forever in a blocking `recv()`. Runs alone
//! in its own target for the same process-global env reason
//! `absent_center.rs` documents.

mod support;

use std::time::{Duration, Instant};

use ctx_traits_desktop::center_link::LinkUpdate;

#[test]
fn dropping_the_receiver_while_idle_ends_the_subscription_promptly() {
    let guard = support::scratch("idle-consumer-link-leak");
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

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match updates.try_recv() {
            Ok(LinkUpdate::Snapshot(_)) => break,
            Ok(_) => {}
            Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("no snapshot arrived before the deadline: {error}"),
        }
    }

    // The subscription is now genuinely idle: the snapshot has been
    // delivered and the peer sends nothing further. Dropping the receiver
    // here, with no event in flight to wake a blocking `recv()`, is exactly
    // the case a proactive-only `tx.is_closed()` check (checked only before
    // subscribing) would miss.
    drop(updates);

    connection.wait_for_client_eof(Duration::from_secs(2));
}
