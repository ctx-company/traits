//! Independent integration test proving the rail's projection (0265.8) end
//! to end through the real wire path: `FakePeer` → `center_link` →
//! `CenterFace::rail`. Runs alone in its own target for the same
//! process-global env reason `absent_center.rs` documents.
//!
//! The process cwd is moved outside any repository first, so a cwd-derived
//! identity is structurally impossible to confuse with a pass — the rail's
//! name projection and grouping must come from the wire rows alone.

mod support;

use std::time::{Duration, Instant, SystemTime};

use ctx_traits_desktop::center_link::LinkUpdate;
use ctx_traits_desktop::frame_list::DotTone;
use ctx_traits_desktop::rail::Rail;
use ctx_traits_desktop::run_row::RepoScope;
use ctx_traits_desktop::shell::{CenterFace, CenterState};
use ctx_traits_io::center::{CenterDelta, CenterPublicRow};
use ctx_traits_io::run_summary::RunSummary;

fn row(repo_key: &str, repo_path: &str, ledger_path: &str, run_id: &str) -> CenterPublicRow {
    row_with_liveness(repo_key, repo_path, ledger_path, run_id, true)
}

fn row_with_liveness(
    repo_key: &str,
    repo_path: &str,
    ledger_path: &str,
    run_id: &str,
    live: bool,
) -> CenterPublicRow {
    CenterPublicRow {
        summary: RunSummary {
            run_id: run_id.to_string(),
            ..RunSummary::unreadable(run_id.to_string(), "fixture".to_string())
        },
        repo_key: repo_key.to_string(),
        repo_path: repo_path.to_string(),
        ledger_path: ledger_path.to_string(),
        live,
        modified_epoch_secs: 0,
    }
}

fn dot_for(rail: &Rail, repo_key: &str) -> DotTone {
    let index = (0..rail.repos().len())
        .find(|&i| rail.repos()[i].repo_key == repo_key)
        .unwrap_or_else(|| panic!("no rail row for {repo_key}"));
    rail.dot(index)
}

fn recv_snapshot(updates: &async_channel::Receiver<LinkUpdate>) -> Vec<CenterPublicRow> {
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
fn rail_projects_from_the_wire_grouping_reconciling_and_staying_stale() {
    // A cwd-derived identity must be structurally impossible: move outside
    // any repository before touching the wire path at all.
    std::env::set_current_dir("/tmp").expect("move cwd outside any repository");

    let guard = support::scratch("rail-spaces");
    let root = guard.0.clone();
    // SAFETY: this test owns the whole process environment for the duration
    // of the call below (it is the only test in this target) and restores
    // nothing because the process exits with it.
    let socket = unsafe { support::install_center_env(&root) };

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let row_a1 = row("repo-a", "/x/ctx-company/traits", "/repo-a/1.json", "run-1");
    let row_a2 = row("repo-a", "/x/ctx-company/traits", "/repo-a/2.json", "run-2");
    let row_b1 = row("repo-b", "/y/other", "/repo-b/1.json", "run-3");
    let row_c1 = row("adhoc-1234", "", "/adhoc-1234/1.json", "run-4");
    connection.serve_snapshot(&subscribe_id, &[row_a1, row_a2, row_b1, row_c1]);

    let snapshot_rows = recv_snapshot(&updates);
    let mut face = CenterFace::new(RepoScope::All);
    face.apply(LinkUpdate::Snapshot(snapshot_rows), SystemTime::now());

    let rail = face.rail(Some("repo-a"));
    let mut names: Vec<&str> = rail.repos().iter().map(|r| r.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, vec!["adhoc-1234", "ctx-company/traits", "y/other"]);
    assert_eq!(rail.space(), Some("ctx-company/traits"));

    // Land Appeared/RowChanged/Ended and prove the repository survives its
    // first row's removal, then vanishes with its last.
    connection.send_delta(&CenterDelta::Appeared {
        row: Box::new(row(
            "repo-a",
            "/x/ctx-company/traits",
            "/repo-a/3.json",
            "run-5",
        )),
    });
    let delta = recv_delta(&updates);
    face.apply(LinkUpdate::Delta(delta), SystemTime::now());
    assert_eq!(
        face.rail(Some("repo-a")).repos().len(),
        3,
        "repo-a, repo-b and adhoc-1234 all still present"
    );

    assert_eq!(
        dot_for(&face.rail(Some("repo-a")), "repo-b"),
        DotTone::Accent,
        "repo-b's only row is live"
    );
    connection.send_delta(&CenterDelta::RowChanged {
        row: Box::new(row_with_liveness(
            "repo-b",
            "/y/other",
            "/repo-b/1.json",
            "run-3",
            false,
        )),
    });
    let delta = recv_delta(&updates);
    face.apply(LinkUpdate::Delta(delta), SystemTime::now());
    assert_eq!(
        dot_for(&face.rail(Some("repo-a")), "repo-b"),
        DotTone::Idle,
        "the RowChanged delta turned repo-b's only row non-live"
    );

    connection.send_delta(&CenterDelta::Ended {
        row: Box::new(row(
            "repo-a",
            "/x/ctx-company/traits",
            "/repo-a/1.json",
            "run-1",
        )),
    });
    let delta = recv_delta(&updates);
    face.apply(LinkUpdate::Delta(delta), SystemTime::now());
    assert!(
        face.rail(Some("repo-a"))
            .repos()
            .iter()
            .any(|r| r.repo_key == "repo-a"),
        "repo-a survives its first row ending — two rows remain"
    );

    connection.send_delta(&CenterDelta::Ended {
        row: Box::new(row(
            "repo-a",
            "/x/ctx-company/traits",
            "/repo-a/2.json",
            "run-2",
        )),
    });
    let delta = recv_delta(&updates);
    face.apply(LinkUpdate::Delta(delta), SystemTime::now());
    connection.send_delta(&CenterDelta::Ended {
        row: Box::new(row(
            "repo-a",
            "/x/ctx-company/traits",
            "/repo-a/3.json",
            "run-5",
        )),
    });
    let delta = recv_delta(&updates);
    face.apply(LinkUpdate::Delta(delta), SystemTime::now());
    let rail = face.rail(Some("repo-a"));
    assert!(
        !rail.repos().iter().any(|r| r.repo_key == "repo-a"),
        "repo-a vanishes once its last row ends"
    );
    assert_eq!(
        rail.space(),
        None,
        "the footer's space line disappears together with the active row"
    );

    // Shut the peer down and prove the previously accepted rail stays
    // visible and stale.
    connection.shutdown();
    let reason = recv_down(&updates);
    face.apply(LinkUpdate::Down(reason), SystemTime::now());
    assert!(matches!(face.state(), CenterState::Stale { .. }));
    let rail = face.rail(None);
    assert!(rail.is_stale());
    assert_eq!(rail.repos().len(), 2, "repo-b and adhoc-1234 stay visible");

    // Serve a recovery snapshot and assert wholesale replacement.
    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let row_d1 = row("repo-d", "/z/fresh", "/repo-d/1.json", "run-6");
    connection.serve_snapshot(&subscribe_id, &[row_d1]);
    let fresh_rows = recv_snapshot(&updates);
    face.apply(LinkUpdate::Snapshot(fresh_rows), SystemTime::now());
    let rail = face.rail(None);
    assert!(!rail.is_stale());
    assert_eq!(rail.repos().len(), 1);
    assert_eq!(rail.repos()[0].repo_key, "repo-d");

    connection.shutdown();
    drop(updates);
}
