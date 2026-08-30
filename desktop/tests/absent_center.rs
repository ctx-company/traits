//! Independent integration test proving the first-contact absent-center case
//! (0256.5): no matching center is serving, the link must never spawn one,
//! must retry on a bounded backoff without spinning, and must recover once a
//! center appears. Runs alone in its own target so the process-global env
//! mutation below has no concurrent observer within this crate.

mod support;

use std::time::{Duration, Instant, SystemTime};

use ctx_traits_desktop::center_link::LinkUpdate;
use ctx_traits_desktop::run_row::RepoScope;
use ctx_traits_desktop::shell::{CenterFace, CenterState};

#[test]
fn absent_center_reports_down_once_then_recovers_when_one_appears() {
    let guard = support::scratch("absent-center");
    let root = guard.0.clone();
    // SAFETY: this test owns the whole process environment for the duration
    // of the call below (it is the only test in this target) and restores
    // nothing because the process exits with it.
    let socket = unsafe { support::install_center_env(&root) };

    assert!(!socket.exists(), "no center must be listening yet");
    let existing_entries: Vec<_> = std::fs::read_dir(&root)
        .expect("read scratch root")
        .collect();
    assert!(
        existing_entries.is_empty(),
        "the scratch root must gain no files before a center exists"
    );

    let updates = ctx_traits_desktop::center_link::start(None);

    // Over ~1.5s of retries at the 500ms backoff, exactly one `Down` must
    // arrive — proving both the backoff and the outage de-duplication (no
    // hot loop, no repaint spam for a permanently absent center).
    let deadline = Instant::now() + Duration::from_millis(1_500);
    let mut downs = Vec::new();
    while Instant::now() < deadline {
        match updates.try_recv() {
            Ok(LinkUpdate::Down(reason)) => downs.push(reason),
            Ok(LinkUpdate::Snapshot(_)) => panic!("no center exists; a snapshot must not arrive"),
            Ok(LinkUpdate::Delta(_)) => panic!("no center exists; a delta must not arrive"),
            Err(async_channel::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("link channel closed unexpectedly: {error}"),
        }
    }
    assert_eq!(
        downs.len(),
        1,
        "exactly one Down must be reported: {downs:?}"
    );

    assert!(
        !socket.exists(),
        "the link must never spawn a center for its own subscription"
    );
    let entries_after_retries: Vec<_> = std::fs::read_dir(&root)
        .expect("read scratch root after retries")
        .collect();
    assert!(
        entries_after_retries.is_empty(),
        "the scratch root must still gain no files after real no-center retries: {entries_after_retries:?}"
    );

    let mut face = CenterFace::new(RepoScope::All);
    face.apply(LinkUpdate::Down(downs[0].clone()), SystemTime::now());
    assert!(matches!(face.state(), CenterState::Unavailable { .. }));
    assert!(face.rows().is_empty());
    assert!(
        !face.header().contains("runs"),
        "an unavailable header must not present a bare run count: {}",
        face.header()
    );

    let peer = support::FakePeer::bind(&socket);
    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let row = support::wire_row("repo", "/repo/session.json", "recovered-run");
    connection.serve_snapshot(&subscribe_id, std::slice::from_ref(&row));

    let deadline = Instant::now() + Duration::from_secs(5);
    let snapshot_rows = loop {
        match updates.try_recv() {
            Ok(LinkUpdate::Snapshot(rows)) => break rows,
            Ok(LinkUpdate::Down(_)) => {}
            Ok(LinkUpdate::Delta(_)) => panic!("no delta expected before a fresh snapshot"),
            Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("no snapshot arrived before the deadline: {error}"),
        }
    };
    assert_eq!(snapshot_rows.len(), 1);
    assert_eq!(snapshot_rows[0].summary.run_id, "recovered-run");

    face.apply(LinkUpdate::Snapshot(snapshot_rows), SystemTime::now());
    assert!(matches!(face.state(), CenterState::Connected { .. }));

    connection.shutdown();
    drop(updates);
}
