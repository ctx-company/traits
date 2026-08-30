//! Integration proof for 0265.5: the real `write_session_ledger` +
//! `write_activity_sidecar` → `run_row::project` → `select` → `detail::load`
//! → `tree` → `FrameList` path, exercising span currency and reconstruction
//! stability against real files rather than in-process fixtures. No env
//! mutation, no center — the `detail_frame_tree.rs` pattern.

mod support;

use std::io::Write as _;
use std::time::Duration;

use camino::Utf8PathBuf;
use ctx_traits_core::procedure::activity::{ActivityEvent, ActivityKind};
use ctx_traits_core::procedure::session::Session;
use ctx_traits_desktop::center_link::LinkUpdate;
use ctx_traits_desktop::detail::{self, RunDetail};
use ctx_traits_desktop::frame_list::{FrameList, RightSide};
use ctx_traits_desktop::run_row;
use ctx_traits_io::activity_sidecar::{ActivityRecord, activity_path};
use ctx_traits_io::center::CenterDelta;

fn session_with_one_frame(item_id: &str, status: &str) -> Session {
    serde_json::from_value(serde_json::json!({
        "schema-version": "0.1.0",
        "session-id": format!("{item_id}-session"),
        "run-id": format!("{item_id}-run"),
        "trait-id": "desktop-frame-list-fixture-trait",
        "current-run-index": 0,
        "status": "completed",
        "provenance": {
            "started-by": {"surface": "test", "caller": "frame-list-rows-fixture"},
            "state-source": "test",
        },
        "ledger": {
            "run-id": format!("{item_id}-run"),
            "trait-id": "desktop-frame-list-fixture-trait",
            "current-run-index": 0,
            "final-state": "completed",
            "sequence-statuses": [{
                "sequence-index": 0,
                "run-index": 0,
                "item-id": item_id,
                "title": item_id,
                "status": status,
                "reason": "",
                "position-path": [],
            }],
        },
        "state-digest": format!("sha256:frame-list-rows-{item_id}"),
    }))
    .expect("fixture session")
}

fn session_with_loop_sharing_one_frame_id(shared_item_id: &str) -> Session {
    let iteration_path = |iteration: usize| {
        serde_json::json!([
            {"kind": "procedure", "id": "the-loop", "index": 0},
            {"kind": "loop", "id": "the-loop-body", "index": iteration, "iteration": iteration},
            {"kind": "item", "id": shared_item_id, "index": 0, "iteration": iteration},
        ])
    };
    serde_json::from_value(serde_json::json!({
        "schema-version": "0.1.0",
        "session-id": "loop-shared-session",
        "run-id": "loop-shared-run",
        "trait-id": "desktop-frame-list-fixture-trait",
        "current-run-index": 0,
        "status": "completed",
        "provenance": {
            "started-by": {"surface": "test", "caller": "frame-list-rows-fixture"},
            "state-source": "test",
        },
        "ledger": {
            "run-id": "loop-shared-run",
            "trait-id": "desktop-frame-list-fixture-trait",
            "current-run-index": 0,
            "final-state": "completed",
            "sequence-statuses": [
                {
                    "sequence-index": 0,
                    "run-index": 0,
                    "item-id": "the-loop",
                    "title": "The loop",
                    "status": "pending",
                    "reason": "",
                    "position-path": [],
                },
                {
                    "sequence-index": 1,
                    "run-index": 1,
                    "item-id": shared_item_id,
                    "title": shared_item_id,
                    "status": "accepted",
                    "reason": "",
                    "position-path": iteration_path(0),
                },
                {
                    "sequence-index": 2,
                    "run-index": 2,
                    "item-id": shared_item_id,
                    "title": shared_item_id,
                    "status": "accepted",
                    "reason": "",
                    "position-path": iteration_path(1),
                },
            ],
        },
        "state-digest": "sha256:frame-list-rows-loop-shared",
    }))
    .expect("fixture session")
}

/// Append one `Activity` record with an explicit `at_epoch_ms` and
/// `sequence`, bypassing `ActivitySidecarWriter` (which stamps wall-clock
/// time) so a test can control ordering deterministically. Still the real IO
/// boundary: the bytes on disk are exactly what `ctx_traits_io::
/// activity_sidecar::read_activity` parses back. `sequence` is carried
/// through unchanged but plays no role in the span fold — production does
/// not provide it as a durable per-frame identity (`flush_pending` persists
/// every coalesced record with `sequence: 0`), so a caller may pass `0` for
/// every record, exactly as production does.
fn append_activity_line(
    ledger_path: &camino::Utf8Path,
    frame_id: &str,
    sequence: u64,
    at_epoch_ms: u64,
) {
    let record = ActivityRecord::Activity {
        at_epoch_ms,
        event: ActivityEvent {
            sequence,
            frame_id: frame_id.to_string(),
            kind: ActivityKind::Thinking,
            text: None,
            tool: None,
            tokens: None,
            rate_limit: None,
        },
    };
    let sidecar_path = activity_path(ledger_path);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(sidecar_path.as_std_path())
        .expect("open sidecar for append");
    writeln!(file, "{}", serde_json::to_string(&record).unwrap()).expect("append activity line");
}

fn projected_row(repo_key: &str, ledger_path: &Utf8PathBuf, run_id: &str) -> run_row::RunRow {
    let wire_rows = vec![support::wire_row(repo_key, ledger_path.as_str(), run_id)];
    run_row::project(&wire_rows, &run_row::RepoScope::All)
        .into_iter()
        .next()
        .expect("projected row present")
}

#[test]
fn a_unique_frame_with_two_stamps_renders_a_compact_time() {
    let guard = support::scratch("frame-list-rows-unique-span");
    let ledger_path =
        Utf8PathBuf::from_path_buf(guard.0.join("repo-a").join("session.json")).unwrap();
    support::write_session_ledger(&ledger_path, "session", "run", false);
    // Overwrite with a fixture carrying a real frame id/status the span can
    // attach to.
    ctx_traits_io::run_session::write_run_session(
        &ledger_path,
        &session_with_one_frame("solo-item", "accepted"),
    )
    .unwrap();
    append_activity_line(&ledger_path, "solo-item", 1, 1_000);
    append_activity_line(&ledger_path, "solo-item", 2, 3_000);

    let row = projected_row("repo-a", &ledger_path, "run");
    let mut detail = RunDetail::default();
    let request = detail.select(&row).unwrap();
    assert!(detail.apply(request.generation, detail::load(&request)));
    let tree = detail.tree().unwrap();
    let list = FrameList::from_tree(&tree);

    assert_eq!(
        list.rows()[0].right,
        RightSide::Elapsed(Some(Duration::from_millis(2_000)))
    );
}

type SidecarPopulator = Box<dyn Fn(&camino::Utf8Path)>;

#[test]
fn every_omission_case_renders_no_time_with_the_tree_otherwise_intact() {
    let guard = support::scratch("frame-list-rows-omissions");

    let cases: Vec<(&str, SidecarPopulator)> = vec![
        ("no-sidecar", Box::new(|_: &camino::Utf8Path| {})),
        (
            "no-matching-record",
            Box::new(|path: &camino::Utf8Path| {
                append_activity_line(path, "some-other-frame", 1, 1_000);
            }),
        ),
        (
            "one-record",
            Box::new(|path: &camino::Utf8Path| {
                append_activity_line(path, "solo-item", 1, 1_000);
            }),
        ),
        (
            "equal-stamps",
            Box::new(|path: &camino::Utf8Path| {
                append_activity_line(path, "solo-item", 1, 1_000);
                append_activity_line(path, "solo-item", 2, 1_000);
            }),
        ),
    ];

    for (label, populate_sidecar) in cases {
        let ledger_path =
            Utf8PathBuf::from_path_buf(guard.0.join(label).join("session.json")).unwrap();
        ctx_traits_io::run_session::write_run_session(
            &ledger_path,
            &session_with_one_frame("solo-item", "accepted"),
        )
        .unwrap();
        populate_sidecar(&ledger_path);

        let row = projected_row(label, &ledger_path, "run");
        let mut detail = RunDetail::default();
        let request = detail.select(&row).unwrap();
        assert!(detail.apply(request.generation, detail::load(&request)));
        let tree = detail.tree().unwrap();
        let list = FrameList::from_tree(&tree);

        assert_eq!(
            list.rows()[0].right,
            RightSide::Elapsed(None),
            "{label} must omit the time, not fabricate one"
        );
        assert_eq!(
            list.rows()[0].title,
            "solo-item",
            "{label}: tree stays intact"
        );
    }
}

#[test]
fn a_loop_shared_frame_id_omits_the_span_on_both_executions() {
    let guard = support::scratch("frame-list-rows-loop-shared");
    let ledger_path =
        Utf8PathBuf::from_path_buf(guard.0.join("repo-a").join("session.json")).unwrap();
    ctx_traits_io::run_session::write_run_session(
        &ledger_path,
        &session_with_loop_sharing_one_frame_id("shared-item"),
    )
    .unwrap();
    append_activity_line(&ledger_path, "shared-item", 1, 1_000);
    append_activity_line(&ledger_path, "shared-item", 2, 5_000);

    let row = projected_row("repo-a", &ledger_path, "run");
    let mut detail = RunDetail::default();
    let request = detail.select(&row).unwrap();
    assert!(detail.apply(request.generation, detail::load(&request)));
    let tree = detail.tree().unwrap();
    let list = FrameList::from_tree(&tree);

    let shared_rows: Vec<_> = list
        .rows()
        .iter()
        .filter(|row| row.title == "shared-item")
        .collect();
    assert_eq!(shared_rows.len(), 2, "two executions of the shared item id");
    for row in shared_rows {
        assert_eq!(
            row.right,
            RightSide::Elapsed(None),
            "an id shared by more than one execution must never guess a span"
        );
    }
}

#[test]
fn a_settled_run_reconstructed_twice_yields_identical_rows_states_and_spans() {
    let guard = support::scratch("frame-list-rows-reconstruction-stability");
    let ledger_path =
        Utf8PathBuf::from_path_buf(guard.0.join("repo-a").join("session.json")).unwrap();
    ctx_traits_io::run_session::write_run_session(
        &ledger_path,
        &session_with_one_frame("solo-item", "accepted"),
    )
    .unwrap();
    append_activity_line(&ledger_path, "solo-item", 1, 1_000);
    append_activity_line(&ledger_path, "solo-item", 2, 4_000);

    let row = projected_row("repo-a", &ledger_path, "run");

    let mut first = RunDetail::default();
    let first_request = first.select(&row).unwrap();
    assert!(first.apply(first_request.generation, detail::load(&first_request)));
    let first_list = FrameList::from_tree(&first.tree().unwrap());

    let mut second = RunDetail::default();
    let second_request = second.select(&row).unwrap();
    assert!(second.apply(second_request.generation, detail::load(&second_request)));
    let second_list = FrameList::from_tree(&second.tree().unwrap());

    assert_eq!(first_list, second_list);
}

fn projected_settled_row(
    repo_key: &str,
    ledger_path: &Utf8PathBuf,
    session: &Session,
) -> run_row::RunRow {
    let wire_rows = vec![support::row_from_ledger(
        repo_key,
        ledger_path,
        session,
        false,
    )];
    run_row::project(&wire_rows, &run_row::RepoScope::All)
        .into_iter()
        .next()
        .expect("projected row present")
}

/// Same-frame append order `1_000, 3_000, 2_000` (a non-monotonic final
/// stamp), each carrying `sequence: 0` exactly as `ActivityRecorder::
/// flush_pending` persists a coalesced record, driven live through
/// `detail::follow`'s `ActivityLine` deltas, then settled through a real
/// terminating `RowChanged` + resync: the final accepted state must render
/// the same 1-second span, and match byte-for-byte, a fresh, independently
/// selected settled load of the identical sidecar — not the 2-second span a
/// timestamp-monotonic fold would produce, and not a state still marked
/// live (review-verdict-1 blocker `live-span-reopen-divergence`).
#[test]
fn final_accepted_live_state_matches_a_fresh_reopened_load() {
    let guard = support::scratch("frame-list-rows-live-reopen-divergence");
    let ledger_path =
        Utf8PathBuf::from_path_buf(guard.0.join("repo-a").join("session.json")).unwrap();
    let session = session_with_one_frame("solo-item", "accepted");
    ctx_traits_io::run_session::write_run_session(&ledger_path, &session).unwrap();

    let wire = support::wire_row("repo-a", ledger_path.as_str(), "run");
    let live_row = run_row::project(std::slice::from_ref(&wire), &run_row::RepoScope::All)
        .into_iter()
        .next()
        .expect("projected row present");

    let mut live = RunDetail::default();
    let request = live.select(&live_row).unwrap();
    assert!(live.apply(request.generation, detail::load(&request)));
    for at_epoch_ms in [1_000, 3_000, 2_000] {
        let activity = ActivityRecord::Activity {
            at_epoch_ms,
            event: ActivityEvent {
                sequence: 0,
                frame_id: "solo-item".to_string(),
                kind: ActivityKind::Thinking,
                text: None,
                tool: None,
                tokens: None,
                rate_limit: None,
            },
        };
        live.follow(&LinkUpdate::Delta(CenterDelta::ActivityLine {
            row: Box::new(wire.clone()),
            activity,
        }));
    }

    append_activity_line(&ledger_path, "solo-item", 0, 1_000);
    append_activity_line(&ledger_path, "solo-item", 0, 3_000);
    append_activity_line(&ledger_path, "solo-item", 0, 2_000);

    // Drive the selection to settled through the real terminating
    // RowChanged + resync path, rather than comparing two independently
    // seeded overlays that are both still marked live.
    let mut settled_row = support::row_from_ledger("repo-a", &ledger_path, &session, false);
    settled_row.modified_epoch_secs = 999;
    let outcome = live.follow(&LinkUpdate::Delta(CenterDelta::RowChanged {
        row: Box::new(settled_row.clone()),
    }));
    let resync = outcome.request.expect("fingerprint moved, resync issued");
    assert!(live.apply(resync.generation, detail::load(&resync)));
    let live_list = FrameList::from_tree(&live.tree().unwrap());

    let fresh_row = projected_settled_row("repo-a", &ledger_path, &session);
    let mut fresh = RunDetail::default();
    let fresh_request = fresh.select(&fresh_row).unwrap();
    assert!(fresh.apply(fresh_request.generation, detail::load(&fresh_request)));
    let fresh_list = FrameList::from_tree(&fresh.tree().unwrap());

    assert_eq!(
        live_list.rows()[0].right,
        RightSide::Elapsed(Some(Duration::from_millis(1_000))),
        "append order 1_000 -> 2_000 (the last-folded stamp) is a 1-second span"
    );
    assert_eq!(
        live_list, fresh_list,
        "the final accepted live state must match a fresh, independently selected settled load"
    );
}

#[test]
fn a_resync_failure_leaves_the_previous_frame_list_intact_with_no_orphaned_current_row() {
    let guard = support::scratch("frame-list-rows-resync-failure");
    let ledger_path =
        Utf8PathBuf::from_path_buf(guard.0.join("repo-a").join("session.json")).unwrap();
    ctx_traits_io::run_session::write_run_session(
        &ledger_path,
        &session_with_one_frame("solo-item", "accepted"),
    )
    .unwrap();

    let row = projected_row("repo-a", &ledger_path, "run");
    let mut detail = RunDetail::default();
    let request = detail.select(&row).unwrap();
    assert!(detail.apply(request.generation, detail::load(&request)));
    let before = FrameList::from_tree(&detail.tree().unwrap());

    let mut moved = support::wire_row("repo-a", ledger_path.as_str(), "run");
    moved.modified_epoch_secs = 999;
    let outcome = detail.follow(&LinkUpdate::Delta(CenterDelta::RowChanged {
        row: Box::new(moved),
    }));
    let resync = outcome.request.expect("fingerprint moved, resync issued");
    assert!(detail.apply(resync.generation, Err("transient read error".to_string())));

    let after = FrameList::from_tree(&detail.tree().unwrap());
    assert_eq!(
        before, after,
        "a failed resync must keep the previously accepted list intact"
    );
    assert!(
        after.selected().is_none(),
        "no orphaned current row on a settled, non-live selection"
    );
}
