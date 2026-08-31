//! Integration proof for 0265.6: loop-narration placement and the bounded
//! activity block, through the real `write_run_session` +
//! `ActivitySidecarWriter` -> `detail::load`/`follow` -> `FrameList` path —
//! the `frame_list_rows.rs`/`detail.rs` `down_marks_stale_*` pattern, real
//! files, no env mutation, no center, no clock advanced anywhere.

mod support;

use camino::Utf8PathBuf;
use ctx_traits_core::procedure::activity::{ActivityEvent, ActivityKind};
use ctx_traits_core::procedure::runtime::PathSegment;
use ctx_traits_core::procedure::session::Session;
use ctx_traits_desktop::center_link::LinkUpdate;
use ctx_traits_desktop::detail::{self, FollowState, RunDetail};
use ctx_traits_desktop::frame_list::FrameList;
use ctx_traits_desktop::{placeholders, run_row};
use ctx_traits_io::activity_sidecar::{ActivityRecord, ActivitySidecarWriter, activity_path};
use ctx_traits_io::center::CenterDelta;

/// Append a `StepSummary` and a `SessionTitle` record — the two variants
/// `ActivitySidecarWriter` has no convenience method for — bypassing the
/// writer exactly as `frame_list_rows.rs`'s `append_activity_line` does, so
/// a seeded sidecar can carry all four variants and prove only `Activity`
/// becomes a rendered line.
fn append_non_activity_records(ledger_path: &camino::Utf8Path) {
    let sidecar_path = activity_path(ledger_path);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(sidecar_path.as_std_path())
        .expect("open sidecar for append");
    for record in [
        ActivityRecord::StepSummary {
            at_epoch_ms: 1,
            key: "second-item".to_string(),
            role: "worker".to_string(),
            text: "a finished-step summary, never a line".to_string(),
        },
        ActivityRecord::SessionTitle {
            at_epoch_ms: 2,
            title: "a session title, never a line".to_string(),
        },
    ] {
        use std::io::Write as _;
        writeln!(file, "{}", serde_json::to_string(&record).unwrap()).expect("append record");
    }
}

/// Append one `Activity` record with an explicit `at_epoch_ms`, bypassing
/// `ActivitySidecarWriter::append_activity` (which stamps wall-clock time
/// via `current_epoch_ms()`) so a test can seed a record with a controlled,
/// deterministic timestamp — the `frame_list_rows.rs` `append_activity_line`
/// pattern. Required whenever a later live record's watermark ordering must
/// be asserted against the seed's own timestamp without advancing any clock.
fn append_activity_at(ledger_path: &camino::Utf8Path, at_epoch_ms: u64, event: ActivityEvent) {
    let record = ActivityRecord::Activity { at_epoch_ms, event };
    let sidecar_path = activity_path(ledger_path);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(sidecar_path.as_std_path())
        .expect("open sidecar for append");
    use std::io::Write as _;
    writeln!(file, "{}", serde_json::to_string(&record).unwrap()).expect("append activity line");
}

fn projected_row(repo_key: &str, ledger_path: &Utf8PathBuf, session: &Session) -> run_row::RunRow {
    let row = support::row_from_ledger(repo_key, ledger_path, session, true);
    run_row::project(std::slice::from_ref(&row), &run_row::RepoScope::All)
        .into_iter()
        .next()
        .expect("projected row present")
}

#[test]
fn a_seeded_sidecar_with_all_four_record_variants_renders_narration_and_only_activity_lines() {
    let guard = support::scratch("loop-narration-and-activity-seeded");
    let ledger_path =
        Utf8PathBuf::from_path_buf(guard.0.join("repo-a").join("session.json")).unwrap();
    let session = support::write_nested_session_ledger(&ledger_path, "session-a", "run-a");

    let mut writer = ActivitySidecarWriter::open(&ledger_path);
    writer.append_activity(ActivityEvent {
        sequence: 1,
        frame_id: "second-item".to_string(),
        kind: ActivityKind::RunningTool,
        text: Some(r#"{"raw":"tool-input-json"}"#.to_string()),
        tool: Some("edit".to_string()),
        tokens: None,
        rate_limit: None,
    });
    writer.append_narration("second-item".to_string(), "on it".to_string());
    drop(writer);
    append_non_activity_records(&ledger_path);

    let row = projected_row("repo-a", &ledger_path, &session);
    let mut detail = RunDetail::default();
    let request = detail.select(&row).expect("selection issues a request");
    assert!(detail.apply(request.generation, detail::load(&request)));
    let list = FrameList::from_tree(&detail.tree().unwrap());

    let narration = (0..list.rows().len() + 1)
        .find_map(|index| list.narration_before(index).first().copied())
        .expect("the single-iteration loop group emits a narration placement");
    assert_eq!(narration, placeholders::loop_round_narration("1"));

    let block = list
        .activity_block()
        .expect("the current row's one Activity record produces a block");
    assert_eq!(
        block.lines,
        vec!["edit".to_string()],
        "RunningTool renders its tool label, never the raw JSON text, and the \
         StepSummary/SessionTitle/Narration records contribute no line"
    );
}

#[test]
fn a_live_activity_line_delta_updates_the_block_and_a_failed_resync_keeps_the_previous_state_intact()
 {
    let guard = support::scratch("loop-narration-and-activity-live-and-failed-resync");
    let ledger_path =
        Utf8PathBuf::from_path_buf(guard.0.join("repo-a").join("session.json")).unwrap();
    let session = support::write_nested_session_ledger(&ledger_path, "session-a", "run-a");

    append_activity_at(
        &ledger_path,
        1_000,
        ActivityEvent {
            sequence: 1,
            frame_id: "second-item".to_string(),
            kind: ActivityKind::Thinking,
            text: Some("seed thought".to_string()),
            tool: None,
            tokens: None,
            rate_limit: None,
        },
    );

    let wire = support::row_from_ledger("repo-a", &ledger_path, &session, true);
    let row = run_row::project(std::slice::from_ref(&wire), &run_row::RepoScope::All)
        .into_iter()
        .next()
        .expect("projected row present");
    let mut detail = RunDetail::default();
    let request = detail.select(&row).expect("selection issues a request");
    assert!(detail.apply(request.generation, detail::load(&request)));

    // An accepted `CenterDelta::ActivityLine` updates the already-loaded
    // baseline's block immediately, with no re-read.
    let live_event = ActivityRecord::Activity {
        at_epoch_ms: 2_000,
        event: ActivityEvent {
            sequence: 2,
            frame_id: "second-item".to_string(),
            kind: ActivityKind::Thinking,
            text: Some("live thought".to_string()),
            tool: None,
            tokens: None,
            rate_limit: None,
        },
    };
    let mut wire_for_delta = wire.clone();
    wire_for_delta.ledger_path = ledger_path.to_string();
    let outcome = detail.follow(&LinkUpdate::Delta(CenterDelta::ActivityLine {
        row: Box::new(wire_for_delta),
        activity: live_event,
    }));
    assert!(outcome.changed, "the live delta must change the baseline");
    assert!(
        outcome.request.is_none(),
        "an activity delta never triggers a re-read"
    );
    let before = FrameList::from_tree(&detail.tree().unwrap());
    let block_before = before.activity_block().expect("block after the live delta");
    assert!(
        block_before
            .lines
            .iter()
            .any(|line| line.contains("live thought")),
        "the live delta's line is present"
    );
    assert!(
        block_before
            .lines
            .iter()
            .any(|line| line.contains("seed thought")),
        "the seed line is retained alongside the live one"
    );

    // Now a resync that fails must leave the previous list — narration,
    // current row and block — intact, with no leaked line.
    let mut moved = wire.clone();
    moved.ledger_path = ledger_path.to_string();
    moved.modified_epoch_secs = 999;
    let outcome = detail.follow(&LinkUpdate::Delta(CenterDelta::RowChanged {
        row: Box::new(moved),
    }));
    let resync = outcome.request.expect("fingerprint moved, resync issued");
    assert!(detail.apply(resync.generation, Err("transient read error".to_string())));

    let after = FrameList::from_tree(&detail.tree().unwrap());
    assert_eq!(
        before, after,
        "a failed resync must keep the previously accepted list intact, block and all"
    );
}

#[test]
fn a_current_frame_transition_shows_only_the_new_frames_lines_and_down_renders_the_stale_posture() {
    let guard = support::scratch("loop-narration-and-activity-transition-and-down");
    let ledger_path =
        Utf8PathBuf::from_path_buf(guard.0.join("repo-a").join("session.json")).unwrap();
    let mut session = support::write_session_ledger(&ledger_path, "session-a", "run-a", true);
    session.ledger.sequence_statuses = vec![ctx_traits_core::procedure::runtime::SequenceStatus {
        sequence_index: 0,
        run_index: 0,
        item_id: Some("first-item".to_string()),
        title: "First item".to_string(),
        status: ctx_traits_core::procedure::runtime::SequenceStatusKind::Ready,
        reason: String::new(),
        position_path: Vec::new(),
    }];
    session.active_path = vec![PathSegment {
        kind: "procedure".to_string(),
        id: Some("first-item".to_string()),
        index: 0,
        iteration: None,
        item_index: None,
    }];
    ctx_traits_io::run_session::write_run_session(&ledger_path, &session).unwrap();

    let mut writer = ActivitySidecarWriter::open(&ledger_path);
    writer.append_activity(ActivityEvent {
        sequence: 1,
        frame_id: "first-item".to_string(),
        kind: ActivityKind::Thinking,
        text: Some("working on the first frame".to_string()),
        tool: None,
        tokens: None,
        rate_limit: None,
    });
    drop(writer);

    let row = projected_row("repo-a", &ledger_path, &session);
    let mut detail = RunDetail::default();
    let request = detail.select(&row).expect("selection issues a request");
    assert!(detail.apply(request.generation, detail::load(&request)));
    let first_list = FrameList::from_tree(&detail.tree().unwrap());
    let first_block = first_list
        .activity_block()
        .expect("the first current frame's block");
    assert!(
        first_block
            .lines
            .iter()
            .any(|line| line.contains("working on the first frame"))
    );

    // The current frame transitions: a second frame lands and becomes
    // `active_path`, with its own, disjoint sidecar evidence.
    session
        .ledger
        .sequence_statuses
        .push(ctx_traits_core::procedure::runtime::SequenceStatus {
            sequence_index: 1,
            run_index: 1,
            item_id: Some("second-item".to_string()),
            title: "Second item".to_string(),
            status: ctx_traits_core::procedure::runtime::SequenceStatusKind::Ready,
            reason: String::new(),
            position_path: Vec::new(),
        });
    session.active_path = vec![PathSegment {
        kind: "procedure".to_string(),
        id: Some("second-item".to_string()),
        index: 1,
        iteration: None,
        item_index: None,
    }];
    ctx_traits_io::run_session::write_run_session(&ledger_path, &session).unwrap();
    let mut writer = ActivitySidecarWriter::open(&ledger_path);
    writer.append_activity(ActivityEvent {
        sequence: 2,
        frame_id: "second-item".to_string(),
        kind: ActivityKind::Thinking,
        text: Some("working on the second frame".to_string()),
        tool: None,
        tokens: None,
        rate_limit: None,
    });
    drop(writer);

    let mut moved = support::row_from_ledger("repo-a", &ledger_path, &session, true);
    moved.modified_epoch_secs = 999;
    let outcome = detail.follow(&LinkUpdate::Delta(CenterDelta::RowChanged {
        row: Box::new(moved),
    }));
    let resync = outcome.request.expect("fingerprint moved, resync issued");
    assert!(detail.apply(resync.generation, detail::load(&resync)));
    let second_list = FrameList::from_tree(&detail.tree().unwrap());
    let second_block = second_list
        .activity_block()
        .expect("the second current frame's block");
    assert!(
        second_block
            .lines
            .iter()
            .any(|line| line.contains("working on the second frame")),
        "the new current frame's lines are shown"
    );
    assert!(
        second_block
            .lines
            .iter()
            .all(|line| !line.contains("working on the first frame")),
        "none of the previous frame's lines survive the transition"
    );

    // `LinkUpdate::Down` renders the stale posture: the previous list stays
    // exactly as it was (no lines dropped, none fabricated), and the
    // selection reports stale rather than presenting anything as current.
    let outcome = detail.follow(&LinkUpdate::Down("subscription closed".to_string()));
    assert!(outcome.changed);
    assert!(matches!(
        detail.follow_state(),
        Some(FollowState::Stale { .. })
    ));
    let stale_list = FrameList::from_tree(&detail.tree().unwrap());
    assert_eq!(
        stale_list, second_list,
        "a Down update must not alter the tree it stops following — the stale \
         posture is reported via follow_state, never by dropping or inventing lines"
    );
}
