//! Integration proofs for the Sessions preview run block and footer
//! (0265.10), driven against a real `support::FakePeer` center — the
//! atomicity, staleness, and honest-failure contract review-verdict-1's
//! blocker `selected-preview-not-atomic` asked for, exercised through
//! `RunDetail::preview_state` and `crate::preview`'s composers exactly as
//! `Shell::render` calls them. Runs alone in its own target for the same
//! process-global env reason `detail_stale_recovery.rs` documents.

mod support;

use std::time::{Duration, Instant};

use camino::Utf8PathBuf;
use ctx_traits_desktop::center_link::LinkUpdate;
use ctx_traits_desktop::detail::{self, DetailLoad, PreviewState, RunDetail};
use ctx_traits_desktop::preview::{
    sessions_footer, sessions_now_item, sessions_run_block, sessions_verdict_block,
};
use ctx_traits_desktop::run_row;

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

/// Serve one claimed-task request with `tag`/`data`, on a background thread,
/// while the foreground thread blocks inside `detail::load`.
fn load_serving_claimed_task(
    peer: &support::FakePeer,
    request: &detail::LoadRequest,
    tag: &str,
    data: Option<serde_json::Value>,
) -> Result<detail::DetailBaseline, String> {
    std::thread::scope(|scope| {
        let server = scope.spawn(|| {
            let mut connection = peer.accept();
            let (id, _session_id, _repo_key) = connection.read_claimed_task_request();
            connection.send_claimed_task_result(&id, tag, data);
        });
        let outcome = detail::load(request);
        server.join().expect("claimed-task server thread");
        outcome
    })
}

/// Serve a claimed-task request by dropping the connection unanswered — the
/// transport-failure branch `claimed_task` must still surface as `Err`
/// rather than blocking forever.
fn load_with_claimed_task_failure(
    peer: &support::FakePeer,
    request: &detail::LoadRequest,
) -> Result<detail::DetailBaseline, String> {
    std::thread::scope(|scope| {
        let server = scope.spawn(|| {
            let connection = peer.accept();
            connection.shutdown();
        });
        let outcome = detail::load(request);
        server.join().expect("claimed-task server thread");
        outcome
    })
}

fn accepted<'a>(state: &'a Option<PreviewState<'a>>) -> (&'a str, Option<&'a str>, bool) {
    match state {
        Some(PreviewState::Accepted {
            baseline,
            stale,
            refreshing,
            ..
        }) => (
            Box::leak(baseline.row.run_id.clone().into_boxed_str()),
            *stale,
            *refreshing,
        ),
        other => panic!("expected an accepted preview state, got {other:?}"),
    }
}

/// Runs every scenario in this file sequentially, in a single `#[test]`, so
/// each can call `install_center_env` (a process-global env mutation) in
/// turn without racing a sibling scenario — the "one test target per
/// process-global env owner" convention `detail_stale_recovery.rs`
/// documents, applied within one file since every scenario here needs a
/// live fake center.
#[test]
fn preview_run_block_atomicity_and_staleness() {
    late_result_for_a_superseded_selection_never_reaches_the_rendered_preview();
    claimed_task_transport_failure_stays_loud_without_failing_the_rest_of_the_detail();
    down_after_a_good_load_marks_the_preview_explicitly_stale();
    a_pending_resync_is_rendered_as_refreshing_not_silently_current();
    the_committed_center_row_not_a_re_derived_ledger_summary_drives_the_rendered_facts();
    an_unreadable_projected_row_renders_unreadable_even_though_the_ledger_read_succeeded();
    reopening_a_finished_run_reproduces_the_same_accepted_facts();
    unselected_row_changes_issue_no_resync_request();
    a_fingerprint_identical_live_flip_removes_the_now_item_with_no_resync();
    a_fingerprint_identical_live_resume_produces_the_now_item_with_no_resync();
    stale_and_refreshing_previews_present_neither_state_block();
    an_ended_delta_removes_the_now_item_through_the_served_detail_path();
}

/// Deliberately diverge the committed center row's `elapsed_seconds` from
/// what a fresh read of the ledger session would itself compute, then prove
/// the rendered run row shows the *committed row's* number — the point of
/// carrying `LoadRequest::row`/`DetailBaseline::row` at all
/// (review-verdict-1 blocker `selected-preview-not-atomic`).
fn the_committed_center_row_not_a_re_derived_ledger_summary_drives_the_rendered_facts() {
    let guard = support::scratch("preview-run-block-row-not-ledger");
    let root = guard.0.clone();
    // SAFETY: see above.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_path =
        Utf8PathBuf::from_path_buf(root.join("repo").join("session.json")).expect("UTF-8 path");
    let session = support::write_session_ledger(&ledger_path, "session-a", "run-a", true);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let mut wire_row = support::row_from_ledger("repo", &ledger_path, &session, true);
    // The ledger's own elapsed is whatever `write_session_ledger` produced
    // (effectively 0); the wire row deliberately disagrees.
    wire_row.summary.elapsed_seconds = 91_234;
    connection.serve_snapshot(&subscribe_id, std::slice::from_ref(&wire_row));

    let snapshot_rows = recv_snapshot(&updates);
    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);
    let row = rows
        .iter()
        .find(|row| row.ledger_path == ledger_path.as_str())
        .expect("seeded row present");
    assert_eq!(
        row.elapsed_seconds, 91_234,
        "the projected row carries the wire value"
    );

    let mut detail = RunDetail::default();
    let request = detail.select(row).expect("selection issues a request");
    let outcome = load_serving_claimed_task(&peer, &request, "unclaimed", None);
    assert!(detail.apply(request.generation, outcome));

    let state = detail.preview_state();
    let block = sessions_run_block(state.as_ref());
    let run_text: String = block.rows[1]
        .value
        .iter()
        .map(|segment| segment.text.clone())
        .collect();
    let expected = ctx_traits_core::procedure::activity::compact_elapsed_text(
        std::time::Duration::from_secs(91_234),
    );
    assert!(
        run_text.contains(&expected),
        "the rendered elapsed must come from the committed row, not a re-read ledger summary: {run_text}"
    );

    drop(updates);
}

/// A projected row already flagged unreadable (`RowState::Unreadable`,
/// derived from the summary's own `parse_error`) still renders unreadable
/// once accepted, even though the ledger read that produced this
/// `DetailBaseline` succeeded (goal 8).
fn an_unreadable_projected_row_renders_unreadable_even_though_the_ledger_read_succeeded() {
    let guard = support::scratch("preview-run-block-unreadable-row");
    let root = guard.0.clone();
    // SAFETY: see above.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_path =
        Utf8PathBuf::from_path_buf(root.join("repo").join("session.json")).expect("UTF-8 path");
    let session = support::write_session_ledger(&ledger_path, "session-a", "run-a", true);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let mut wire_row = support::row_from_ledger("repo", &ledger_path, &session, true);
    wire_row.summary.parse_error = Some("trailing comma at line 4".to_string());
    connection.serve_snapshot(&subscribe_id, std::slice::from_ref(&wire_row));

    let snapshot_rows = recv_snapshot(&updates);
    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);
    let row = rows
        .iter()
        .find(|row| row.ledger_path == ledger_path.as_str())
        .expect("seeded row present");
    assert_eq!(row.state, ctx_traits_desktop::run_row::RowState::Unreadable);

    let mut detail = RunDetail::default();
    let request = detail.select(row).expect("selection issues a request");
    let outcome = load_serving_claimed_task(&peer, &request, "unclaimed", None);
    assert!(detail.apply(request.generation, outcome));

    let state = detail.preview_state();
    let block = sessions_run_block(state.as_ref());
    for row in &block.rows {
        assert_eq!(row.value.len(), 1);
        assert!(row.value[0].text.contains("trailing comma"));
        assert!(!row.value[0].text.contains("0s"));
    }
    let footer = sessions_footer(state.as_ref());
    assert!(footer.contains("trailing comma"));

    drop(updates);
}

/// Driving `detail::load` twice against the same fixture (as two fresh
/// selections would) reproduces the same accepted trait/variant, run/elapsed,
/// task/status and footer facts — nothing exists only in a first, since-
/// discarded GUI model.
fn reopening_a_finished_run_reproduces_the_same_accepted_facts() {
    let guard = support::scratch("preview-run-block-reopen");
    let root = guard.0.clone();
    // SAFETY: see above.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_path =
        Utf8PathBuf::from_path_buf(root.join("repo").join("session.json")).expect("UTF-8 path");
    let session = support::write_session_ledger(&ledger_path, "session-a", "run-a", false);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let wire_row = support::row_from_ledger("repo", &ledger_path, &session, false);
    connection.serve_snapshot(&subscribe_id, std::slice::from_ref(&wire_row));

    let snapshot_rows = recv_snapshot(&updates);
    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);
    let row = rows
        .iter()
        .find(|row| row.ledger_path == ledger_path.as_str())
        .expect("seeded row present");

    let mut first = RunDetail::default();
    let request_first = first.select(row).expect("selection issues a request");
    let outcome_first = load_serving_claimed_task(&peer, &request_first, "unclaimed", None);
    assert!(first.apply(request_first.generation, outcome_first));
    let block_first = sessions_run_block(first.preview_state().as_ref());
    let footer_first = sessions_footer(first.preview_state().as_ref());

    let mut second = RunDetail::default();
    let request_second = second.select(row).expect("selection issues a request");
    let outcome_second = load_serving_claimed_task(&peer, &request_second, "unclaimed", None);
    assert!(second.apply(request_second.generation, outcome_second));
    let block_second = sessions_run_block(second.preview_state().as_ref());
    let footer_second = sessions_footer(second.preview_state().as_ref());

    assert_eq!(block_first, block_second);
    assert_eq!(footer_first, footer_second);

    drop(updates);
}

/// A `RowChanged` delta for a ledger path other than the current selection
/// must not touch the selection at all — no fingerprint move, no resync
/// request, since the request is what would cost a claimed-task round trip.
fn unselected_row_changes_issue_no_resync_request() {
    let guard = support::scratch("preview-run-block-unselected");
    let root = guard.0.clone();
    // SAFETY: see above.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_a =
        Utf8PathBuf::from_path_buf(root.join("repo").join("run-a.json")).expect("UTF-8 path");
    let ledger_b =
        Utf8PathBuf::from_path_buf(root.join("repo").join("run-b.json")).expect("UTF-8 path");
    let session_a = support::write_session_ledger(&ledger_a, "session-a", "run-a", true);
    let session_b = support::write_session_ledger(&ledger_b, "session-b", "run-b", true);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let wire_a = support::row_from_ledger("repo", &ledger_a, &session_a, true);
    let wire_b = support::row_from_ledger("repo", &ledger_b, &session_b, true);
    connection.serve_snapshot(&subscribe_id, &[wire_a, wire_b.clone()]);

    let snapshot_rows = recv_snapshot(&updates);
    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);
    let row_a = rows
        .iter()
        .find(|row| row.ledger_path == ledger_a.as_str())
        .expect("run-a present");

    let mut detail = RunDetail::default();
    let request = detail
        .select(row_a)
        .expect("selecting run-a issues a request");
    let outcome = load_serving_claimed_task(&peer, &request, "unclaimed", None);
    assert!(detail.apply(request.generation, outcome));

    let mut moved_b = wire_b;
    moved_b.modified_epoch_secs += 1;
    let outcome = detail.follow(&ctx_traits_desktop::center_link::LinkUpdate::Delta(
        ctx_traits_io::center::CenterDelta::RowChanged {
            row: Box::new(moved_b),
        },
    ));
    assert!(
        outcome.request.is_none(),
        "a delta for an unselected run must not issue a claimed-task/resync request"
    );
    assert!(
        !outcome.changed,
        "an unselected run's delta must not mark the selection changed either"
    );

    drop(updates);
}

fn late_result_for_a_superseded_selection_never_reaches_the_rendered_preview() {
    let guard = support::scratch("preview-run-block-late-a");
    let root = guard.0.clone();
    // SAFETY: this test file's sole `#[test]` runs every scenario
    // sequentially on one thread, so each scenario owns the process
    // environment for the duration of its own call below.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_a =
        Utf8PathBuf::from_path_buf(root.join("repo").join("run-a.json")).expect("UTF-8 path");
    let ledger_b =
        Utf8PathBuf::from_path_buf(root.join("repo").join("run-b.json")).expect("UTF-8 path");
    let session_a = support::write_session_ledger(&ledger_a, "session-a", "run-a", true);
    let session_b = support::write_session_ledger(&ledger_b, "session-b", "run-b", true);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let wire_a = support::row_from_ledger("repo", &ledger_a, &session_a, true);
    let wire_b = support::row_from_ledger("repo", &ledger_b, &session_b, true);
    connection.serve_snapshot(&subscribe_id, &[wire_a, wire_b]);

    let snapshot_rows = recv_snapshot(&updates);
    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);
    let row_a = rows
        .iter()
        .find(|row| row.ledger_path == ledger_a.as_str())
        .expect("run-a present")
        .clone();
    let row_b = rows
        .iter()
        .find(|row| row.ledger_path == ledger_b.as_str())
        .expect("run-b present")
        .clone();

    let mut detail = RunDetail::default();
    let request_a = detail
        .select(&row_a)
        .expect("selecting run-a issues a request");

    // Select run-b before run-a's read has landed — the generation guard,
    // not request ordering, must decide what wins.
    let request_b = detail
        .select(&row_b)
        .expect("selecting run-b issues a request");
    assert_ne!(request_a.generation, request_b.generation);

    let outcome_b = load_serving_claimed_task(&peer, &request_b, "unclaimed", None);
    assert!(detail.apply(request_b.generation, outcome_b));

    // Run-a's answer lands last. It must be silently dropped: `apply`
    // reports no change, and the rendered preview must still describe run-b.
    let outcome_a = Ok(detail::DetailBaseline {
        session: session_a.clone(),
        activity_overlay: Default::default(),
        skipped_activity_lines: 0,
        variant: Ok(None),
        claimed_task: Ok(ctx_traits_io::center::ClaimedTaskResult::Unclaimed),
        progress: Ok(ctx_traits_core::procedure::run::RunProgress::NoCountedFrames),
        row: request_a.row.clone(),
    });
    assert!(
        !detail.apply(request_a.generation, outcome_a),
        "a superseded generation's outcome must be ignored"
    );

    let state = detail.preview_state();
    let (run_id, stale, _refreshing) = accepted(&state);
    assert_eq!(run_id, "run-b");
    assert_eq!(stale, None);

    let block = sessions_run_block(state.as_ref());
    let footer = sessions_footer(state.as_ref());
    let joined: String = block.rows[1]
        .value
        .iter()
        .map(|segment| segment.text.clone())
        .collect();
    assert!(
        joined.contains("run-b"),
        "the run row must describe run-b, not the superseded run-a: {joined}"
    );
    assert!(!joined.contains("run-a"));
    assert!(footer.contains("run-b"));
    assert!(!footer.contains("run-a"));
}

fn claimed_task_transport_failure_stays_loud_without_failing_the_rest_of_the_detail() {
    let guard = support::scratch("preview-run-block-task-failure");
    let root = guard.0.clone();
    // SAFETY: see above.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_path =
        Utf8PathBuf::from_path_buf(root.join("repo").join("session.json")).expect("UTF-8 path");
    let session = support::write_session_ledger(&ledger_path, "session-a", "run-a", true);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let wire_row = support::row_from_ledger("repo", &ledger_path, &session, true);
    connection.serve_snapshot(&subscribe_id, std::slice::from_ref(&wire_row));

    let snapshot_rows = recv_snapshot(&updates);
    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);
    let row = rows
        .iter()
        .find(|row| row.ledger_path == ledger_path.as_str())
        .expect("seeded row present");

    let mut detail = RunDetail::default();
    let request = detail.select(row).expect("selection issues a request");
    let outcome = load_with_claimed_task_failure(&peer, &request);
    assert!(detail.apply(request.generation, outcome));

    assert!(
        !matches!(detail.load_state(), Some(DetailLoad::Failed(_))),
        "a center outage must not turn a readable run unreadable"
    );

    let state = detail.preview_state();
    let block = sessions_run_block(state.as_ref());
    let task_text = &block.rows[2].value[0].text;
    assert!(
        task_text.starts_with("task unavailable"),
        "the task row must render loud, got: {task_text}"
    );
    assert_eq!(
        block.rows[2].value[0].role,
        Some(ctx_traits_desktop::run_row::StateRole::Danger)
    );

    // Trait/run rows and the footer still describe the accepted run.
    let run_text: String = block.rows[1]
        .value
        .iter()
        .map(|segment| segment.text.clone())
        .collect();
    assert!(run_text.contains("run-a"));
    let footer = sessions_footer(state.as_ref());
    assert!(footer.contains("run-a"));
}

fn down_after_a_good_load_marks_the_preview_explicitly_stale() {
    let guard = support::scratch("preview-run-block-stale");
    let root = guard.0.clone();
    // SAFETY: see above.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_path =
        Utf8PathBuf::from_path_buf(root.join("repo").join("session.json")).expect("UTF-8 path");
    let session = support::write_session_ledger(&ledger_path, "session-a", "run-a", true);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let wire_row = support::row_from_ledger("repo", &ledger_path, &session, true);
    connection.serve_snapshot(&subscribe_id, std::slice::from_ref(&wire_row));

    let snapshot_rows = recv_snapshot(&updates);
    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);
    let row = rows
        .iter()
        .find(|row| row.ledger_path == ledger_path.as_str())
        .expect("seeded row present");

    let mut detail = RunDetail::default();
    let request = detail.select(row).expect("selection issues a request");
    let outcome = load_serving_claimed_task(&peer, &request, "unclaimed", None);
    assert!(detail.apply(request.generation, outcome));

    let state_before = detail.preview_state();
    let (_, stale_before, refreshing_before) = accepted(&state_before);
    assert!(
        !refreshing_before,
        "a settled, current baseline is not refreshing"
    );
    assert_eq!(stale_before, None);
    let block_before = sessions_run_block(state_before.as_ref());
    let footer_before = sessions_footer(state_before.as_ref());
    assert_eq!(block_before.heading, "run");

    connection.shutdown();
    let deadline = Instant::now() + Duration::from_secs(5);
    let reason = loop {
        match updates.try_recv() {
            Ok(LinkUpdate::Down(reason)) => break reason,
            Ok(_) => {}
            Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("no Down arrived before the deadline: {error}"),
        }
    };
    detail.follow(&LinkUpdate::Down(reason));

    let state_after = detail.preview_state();
    let (run_id, stale_after, _refreshing_after) = accepted(&state_after);
    assert_eq!(run_id, "run-a", "the last-accepted baseline stays visible");
    assert!(
        stale_after.is_some(),
        "the retained baseline must be marked explicitly stale, not presented as current"
    );

    // The stale marker must reach the rendered preview, not just the
    // internal enum (review-verdict-1 blocker `selected-preview-not-atomic`,
    // step "distinguish and visibly present refresh-in-flight and stale
    // accepted states").
    let block_after = sessions_run_block(state_after.as_ref());
    assert_ne!(
        block_after.heading, block_before.heading,
        "a stale accepted baseline must render a different heading than a current one"
    );
    let footer_after = sessions_footer(state_after.as_ref());
    assert!(
        footer_after.contains("run-a"),
        "the retained facts stay visible: {footer_after}"
    );
    assert_ne!(
        footer_after, footer_before,
        "the stale footer must differ from the current one"
    );

    drop(updates);
}

/// A fingerprint move issues a resync while the old baseline stays
/// `Loaded` (no loading flash) — `preview_state` must mark this posture
/// `refreshing` so the retained run/task facts are never painted as if the
/// resync had already confirmed them (review-verdict-1 blocker
/// `selected-preview-not-atomic`).
fn a_pending_resync_is_rendered_as_refreshing_not_silently_current() {
    let guard = support::scratch("preview-run-block-refreshing");
    let root = guard.0.clone();
    // SAFETY: see above.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_path =
        Utf8PathBuf::from_path_buf(root.join("repo").join("session.json")).expect("UTF-8 path");
    let session = support::write_session_ledger(&ledger_path, "session-a", "run-a", true);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let wire_row = support::row_from_ledger("repo", &ledger_path, &session, true);
    connection.serve_snapshot(&subscribe_id, std::slice::from_ref(&wire_row));

    let snapshot_rows = recv_snapshot(&updates);
    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);
    let row = rows
        .iter()
        .find(|row| row.ledger_path == ledger_path.as_str())
        .expect("seeded row present");

    let mut detail = RunDetail::default();
    let request = detail.select(row).expect("selection issues a request");
    let outcome = load_serving_claimed_task(&peer, &request, "unclaimed", None);
    assert!(detail.apply(request.generation, outcome));

    let mut moved = wire_row;
    moved.modified_epoch_secs += 1;
    // Preview-visible facts the resync must carry, distinguishable from the
    // pre-resync candidate: elapsed and started-at both move, not just the
    // claimed task (review-verdict-1 blocker `selected-preview-not-atomic`,
    // step "change preview-visible fields on the replacement center row").
    let pre_elapsed_seconds = moved.summary.elapsed_seconds;
    let pre_started_at_epoch = moved.summary.started_at_epoch;
    moved.summary.elapsed_seconds = pre_elapsed_seconds + 9_000;
    moved.summary.started_at_epoch = Some(pre_started_at_epoch.unwrap_or(0) + 1);
    let resync = detail
        .follow(&ctx_traits_desktop::center_link::LinkUpdate::Delta(
            ctx_traits_io::center::CenterDelta::RowChanged {
                row: Box::new(moved),
            },
        ))
        .request
        .expect("a moved fingerprint issues a resync");
    assert_eq!(resync.generation, request.generation + 1);

    // The resync is in flight; `preview_state` must report `refreshing` and
    // the rendered block/footer must not look identical to the settled
    // pre-resync current state.
    let state_settled = {
        let mut settled = RunDetail::default();
        let request = settled.select(row).unwrap();
        assert!(settled.apply(
            request.generation,
            Ok(detail::DetailBaseline {
                session: session.clone(),
                activity_overlay: Default::default(),
                skipped_activity_lines: 0,
                variant: Ok(None),
                claimed_task: Ok(ctx_traits_io::center::ClaimedTaskResult::Unclaimed),
                progress: Ok(ctx_traits_core::procedure::run::RunProgress::NoCountedFrames),
                row: request.row.clone(),
            })
        ));
        settled
    };
    let settled_preview = state_settled.preview_state();
    let refreshing_preview = detail.preview_state();
    let (_, stale, refreshing) = accepted(&refreshing_preview);
    assert_eq!(stale, None, "a resync in flight is not the stale posture");
    assert!(
        refreshing,
        "a resync in flight must be reported as refreshing"
    );

    let settled_block = sessions_run_block(settled_preview.as_ref());
    let refreshing_block = sessions_run_block(refreshing_preview.as_ref());
    assert_eq!(settled_block.heading, "run");
    assert_ne!(
        refreshing_block.heading, settled_block.heading,
        "a pending resync must not render identically to a settled current baseline"
    );

    // The retained candidate must still be the pre-resync facts: task
    // "unclaimed" at this point.
    let task_before = &refreshing_block.rows[2].value[0].text;
    assert_eq!(task_before, "no task claimed");

    // The retained candidate's row/footer must also still be the
    // pre-resync elapsed/started-at facts, not the replacement's — a test
    // that only varies the claimed-task answer cannot detect a split
    // row/footer commit (review-verdict-1 blocker
    // `selected-preview-not-atomic`).
    let elapsed_before = refreshing_block.rows[1].value[2].text.clone();
    let settled_elapsed = settled_block.rows[1].value[2].text.clone();
    assert_eq!(
        elapsed_before, settled_elapsed,
        "a pending resync must keep rendering the pre-resync elapsed value"
    );
    let expected_elapsed_before = ctx_traits_core::procedure::activity::compact_elapsed_text(
        Duration::from_secs(pre_elapsed_seconds),
    );
    assert_eq!(
        elapsed_before, expected_elapsed_before,
        "the retained pre-resync elapsed value must be the pre-resync row's own elapsed, \
         not the replacement's"
    );
    let footer_before = sessions_footer(refreshing_preview.as_ref());
    assert!(
        !footer_before.contains("started"),
        "the pre-resync candidate has no started-at fact: {footer_before}"
    );

    // Now actually serve and apply the resync, with a *distinguishable*
    // claimed-task answer, and prove the switch is atomic: row/task/footer
    // all move together only once the replacement generation is accepted.
    let resync_outcome = load_serving_claimed_task(
        &peer,
        &resync,
        "task",
        Some(serde_json::json!([
            {
                "key": "0265.10",
                "title": "the preview run block and footer",
                "description": "",
                "stored-status": "done",
                "auto-close": null,
            },
            {"type": "none-configured"},
        ])),
    );
    assert!(detail.apply(resync.generation, resync_outcome));

    let post_apply_state = detail.preview_state();
    let (_, stale_after, refreshing_after) = accepted(&post_apply_state);
    assert_eq!(
        stale_after, None,
        "an accepted resync is not the stale posture"
    );
    assert!(
        !refreshing_after,
        "refreshing must clear once the replacement generation is accepted"
    );

    let committed_state = detail.preview_state();
    let committed_block = sessions_run_block(committed_state.as_ref());
    let committed_footer = sessions_footer(committed_state.as_ref());

    let task_after = &committed_block.rows[2].value[0].text;
    assert_eq!(
        task_after, "0265.10",
        "the replacement task answer must be visible once the resync commits"
    );
    assert_ne!(
        task_after, task_before,
        "the committed facts must differ from the retained pre-resync candidate"
    );
    assert!(
        committed_footer.contains("run-a"),
        "the committed footer must reflect the accepted resync: {committed_footer}"
    );

    // The committed row/footer must reflect the replacement's own
    // elapsed/started-at facts, distinct from the retained pre-resync
    // candidate — proving row, task, and footer all moved together only
    // once the replacement generation was accepted.
    let elapsed_after = committed_block.rows[1].value[2].text.clone();
    let expected_elapsed_after = ctx_traits_core::procedure::activity::compact_elapsed_text(
        Duration::from_secs(pre_elapsed_seconds + 9_000),
    );
    assert_eq!(
        elapsed_after, expected_elapsed_after,
        "the committed elapsed value must be the replacement row's own elapsed"
    );
    assert_ne!(
        elapsed_after, elapsed_before,
        "the committed elapsed value must differ from the retained pre-resync candidate"
    );

    let expected_started_epoch = pre_started_at_epoch.unwrap_or(0) + 1;
    let expected_offset = ctx_traits_io::clock::local_utc_offset_seconds(expected_started_epoch);
    let expected_clock =
        ctx_traits_io::clock::epoch_clock_minutes(expected_started_epoch, expected_offset);
    let expected_footer = format!("run-a · started {expected_clock}");
    assert_eq!(
        committed_footer, expected_footer,
        "the committed footer must be the replacement row's own started-at fact"
    );
    assert_ne!(
        committed_footer, footer_before,
        "the committed footer must differ from the retained pre-resync footer"
    );

    drop(updates);
}

/// A `RowChanged` delta whose fingerprint (`modified_epoch_secs` +
/// title-cleared summary) is unchanged from the just-loaded baseline, only
/// `live` flips true-to-false, must remove the now item through the real
/// `follow -> preview_state -> sessions_now_item` chain, and must not issue a
/// resync `LoadRequest` — the connected regression review-verdict-1's
/// blocker `state-blocks-ignore-preview-freshness` asked for, proving
/// `Selection.live` (not the frozen `baseline.row.live`) drives the
/// composer.
fn a_fingerprint_identical_live_flip_removes_the_now_item_with_no_resync() {
    let guard = support::scratch("preview-run-block-live-flip");
    let root = guard.0.clone();
    // SAFETY: see above.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_path =
        Utf8PathBuf::from_path_buf(root.join("repo").join("session.json")).expect("UTF-8 path");
    let session = support::write_session_ledger(&ledger_path, "session-a", "run-a", true);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let wire_row = support::row_from_ledger("repo", &ledger_path, &session, true);
    connection.serve_snapshot(&subscribe_id, std::slice::from_ref(&wire_row));

    let snapshot_rows = recv_snapshot(&updates);
    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);
    let row = rows
        .iter()
        .find(|row| row.ledger_path == ledger_path.as_str())
        .expect("seeded row present");

    let mut detail = RunDetail::default();
    let request = detail.select(row).expect("selection issues a request");
    let outcome = load_serving_claimed_task(&peer, &request, "unclaimed", None);
    assert!(detail.apply(request.generation, outcome));

    let before = detail.preview_state();
    assert!(
        sessions_now_item(before.as_ref()).is_some(),
        "a live, readable, settled selection must render a now item"
    );

    let mut flipped = wire_row;
    flipped.live = false;
    let outcome = detail.follow(&ctx_traits_desktop::center_link::LinkUpdate::Delta(
        ctx_traits_io::center::CenterDelta::RowChanged {
            row: Box::new(flipped),
        },
    ));
    assert!(
        outcome.request.is_none(),
        "a live flip alone must cost no re-read"
    );

    let after = detail.preview_state();
    assert!(
        sessions_now_item(after.as_ref()).is_none(),
        "the now item must disappear on a live-to-finished transition, even a \
         fingerprint-identical one that never triggers a resync"
    );

    drop(updates);
}

/// The mirror of the live-to-finished proof above: a `RowChanged` delta whose
/// fingerprint is unchanged from the just-loaded baseline, only `live` flips
/// false-to-true, must produce a now item with the shared `RowState::Live`
/// presentation (`running` / `StateRole::Accent`) through the real
/// `follow -> preview_state -> sessions_now_item` chain, and must not issue a
/// resync `LoadRequest` — review-verdict-1 blocker
/// `live-resume-marker-uses-frozen-row-state`'s required regression.
fn a_fingerprint_identical_live_resume_produces_the_now_item_with_no_resync() {
    let guard = support::scratch("preview-run-block-live-resume");
    let root = guard.0.clone();
    // SAFETY: see above.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_path =
        Utf8PathBuf::from_path_buf(root.join("repo").join("session.json")).expect("UTF-8 path");
    let session = support::write_session_ledger(&ledger_path, "session-a", "run-a", false);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let wire_row = support::row_from_ledger("repo", &ledger_path, &session, false);
    connection.serve_snapshot(&subscribe_id, std::slice::from_ref(&wire_row));

    let snapshot_rows = recv_snapshot(&updates);
    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);
    let row = rows
        .iter()
        .find(|row| row.ledger_path == ledger_path.as_str())
        .expect("seeded row present");

    let mut detail = RunDetail::default();
    let request = detail.select(row).expect("selection issues a request");
    let outcome = load_serving_claimed_task(&peer, &request, "unclaimed", None);
    assert!(detail.apply(request.generation, outcome));

    let before = detail.preview_state();
    assert!(
        sessions_now_item(before.as_ref()).is_none(),
        "a non-live, readable, settled selection must render no now item"
    );

    let mut flipped = wire_row;
    flipped.live = true;
    let outcome = detail.follow(&ctx_traits_desktop::center_link::LinkUpdate::Delta(
        ctx_traits_io::center::CenterDelta::RowChanged {
            row: Box::new(flipped),
        },
    ));
    assert!(
        outcome.request.is_none(),
        "a live resume alone must cost no re-read"
    );

    let after = detail.preview_state();
    let now_item = sessions_now_item(after.as_ref())
        .expect("the now item must appear on a finished-to-live transition");
    assert_eq!(
        now_item.state_word, "running",
        "the resumed now item must carry the shared live-row presentation word"
    );
    assert_eq!(
        now_item.state_role,
        run_row::StateRole::Accent,
        "the resumed now item must carry the shared live-row presentation role"
    );

    drop(updates);
}

/// Neither state block is presented as current while the accepted preview is
/// stale (center lost) or refreshing (a resync in flight) — proved through
/// the same served `preview_state` a stale/refreshing selection actually
/// reaches, not a hand-built `PreviewState` (review-verdict-1 blocker
/// `state-blocks-ignore-preview-freshness`). The seeded run carries real
/// verdict evidence so `sessions_verdict_block`, not just `sessions_now_item`,
/// is proved under the same stale/refreshing/`Ended` transitions.
fn stale_and_refreshing_previews_present_neither_state_block() {
    let guard = support::scratch("preview-run-block-live-freshness");
    let root = guard.0.clone();
    // SAFETY: see above.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_path =
        Utf8PathBuf::from_path_buf(root.join("repo").join("session.json")).expect("UTF-8 path");
    let session = support::write_session_ledger_with_verdict(
        &ledger_path,
        "session-a",
        "run-a",
        true,
        "approved",
        None,
        None,
    );

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let wire_row = support::row_from_ledger("repo", &ledger_path, &session, true);
    connection.serve_snapshot(&subscribe_id, std::slice::from_ref(&wire_row));

    let snapshot_rows = recv_snapshot(&updates);
    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);
    let row = rows
        .iter()
        .find(|row| row.ledger_path == ledger_path.as_str())
        .expect("seeded row present");

    let mut detail = RunDetail::default();
    let request = detail.select(row).expect("selection issues a request");
    let outcome = load_serving_claimed_task(&peer, &request, "unclaimed", None);
    assert!(detail.apply(request.generation, outcome));
    assert!(sessions_now_item(detail.preview_state().as_ref()).is_some());
    assert!(sessions_verdict_block(detail.preview_state().as_ref()).is_some());

    // Move the fingerprint so `follow` starts a resync, leaving this
    // selection `refreshing` while the old baseline is still `Loaded`.
    let mut moved = wire_row.clone();
    moved.modified_epoch_secs += 1;
    let resync = detail
        .follow(&ctx_traits_desktop::center_link::LinkUpdate::Delta(
            ctx_traits_io::center::CenterDelta::RowChanged {
                row: Box::new(moved),
            },
        ))
        .request
        .expect("a moved fingerprint issues a resync");

    let refreshing_state = detail.preview_state();
    assert!(
        sessions_now_item(refreshing_state.as_ref()).is_none(),
        "a refreshing preview must not present a now item as current"
    );
    assert!(
        sessions_verdict_block(refreshing_state.as_ref()).is_none(),
        "a refreshing preview must not present a verdict block as current"
    );

    let resync_outcome = load_serving_claimed_task(&peer, &resync, "unclaimed", None);
    assert!(detail.apply(resync.generation, resync_outcome));
    assert!(sessions_now_item(detail.preview_state().as_ref()).is_some());
    assert!(sessions_verdict_block(detail.preview_state().as_ref()).is_some());

    // Lose the center: the accepted baseline stays but must be marked stale.
    connection.shutdown();
    let deadline = Instant::now() + Duration::from_secs(5);
    let reason = loop {
        match updates.try_recv() {
            Ok(LinkUpdate::Down(reason)) => break reason,
            Ok(_) => {}
            Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("no Down arrived before the deadline: {error}"),
        }
    };
    detail.follow(&LinkUpdate::Down(reason));
    let stale_state = detail.preview_state();
    assert!(
        sessions_now_item(stale_state.as_ref()).is_none(),
        "a stale preview must not present a now item as current"
    );
    assert!(
        sessions_verdict_block(stale_state.as_ref()).is_none(),
        "a stale preview must not present a verdict block as current"
    );

    drop(updates);
}

/// An `Ended` center delta on the selected run must reach the served
/// `preview_state` the same way `RowChanged` does — the now item disappears
/// once the ended row's liveness commits — proving the third `CenterDelta`
/// variant is not a blind spot next to `RowChanged`/`Down` (review-verdict-1
/// blocker `state-blocks-ignore-preview-freshness`).
fn an_ended_delta_removes_the_now_item_through_the_served_detail_path() {
    let guard = support::scratch("preview-run-block-ended");
    let root = guard.0.clone();
    // SAFETY: see above.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_path =
        Utf8PathBuf::from_path_buf(root.join("repo").join("session.json")).expect("UTF-8 path");
    let session = support::write_session_ledger(&ledger_path, "session-a", "run-a", true);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let wire_row = support::row_from_ledger("repo", &ledger_path, &session, true);
    connection.serve_snapshot(&subscribe_id, std::slice::from_ref(&wire_row));

    let snapshot_rows = recv_snapshot(&updates);
    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);
    let row = rows
        .iter()
        .find(|row| row.ledger_path == ledger_path.as_str())
        .expect("seeded row present");

    let mut detail = RunDetail::default();
    let request = detail.select(row).expect("selection issues a request");
    let outcome = load_serving_claimed_task(&peer, &request, "unclaimed", None);
    assert!(detail.apply(request.generation, outcome));
    assert!(sessions_now_item(detail.preview_state().as_ref()).is_some());

    let mut ended = wire_row;
    ended.live = false;
    let follow_outcome = detail.follow(&ctx_traits_desktop::center_link::LinkUpdate::Delta(
        ctx_traits_io::center::CenterDelta::Ended {
            row: Box::new(ended),
        },
    ));
    assert!(
        follow_outcome.request.is_none(),
        "a fingerprint-identical Ended delta must cost no re-read, same as RowChanged"
    );
    assert!(
        sessions_now_item(detail.preview_state().as_ref()).is_none(),
        "the now item must disappear once the Ended delta commits"
    );

    drop(updates);
}
