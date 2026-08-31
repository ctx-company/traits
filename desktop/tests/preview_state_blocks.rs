//! Integration proofs for the Sessions preview `in progress`/`verdict`
//! blocks (0265.11), driven against a real `support::FakePeer` center —
//! review-verdict-1's blocker `required-rendered-preview-evidence-missing`
//! asked for representative proof of the selected-detail, verdict-round, and
//! now-item behavior classes through `RunDetail::preview_state` and
//! `crate::preview`'s composers exactly as `Shell::render` calls them. Runs
//! alone in its own target for the same process-global env reason
//! `detail_stale_recovery.rs` documents.

mod support;

use std::time::{Duration, Instant};

use camino::Utf8PathBuf;
use ctx_traits_desktop::center_link::LinkUpdate;
use ctx_traits_desktop::detail::{self, DetailLoad, PreviewState, RunDetail};
use ctx_traits_desktop::preview::{
    sessions_now_item, sessions_slots_block, sessions_verdict_block,
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

#[test]
fn preview_state_blocks_selected_detail_and_verdict_round() {
    a_verdict_written_to_an_already_selected_run_reconciles_through_the_served_detail_path();
    out_of_order_selection_never_mixes_one_selections_title_with_anothers_verdict();
    a_rejected_candidate_leaves_the_previously_accepted_blocks_whole();
    an_implement_shaped_run_renders_a_verdict_block_and_a_plain_run_renders_none();
    no_read_fires_for_an_unselected_row_change();
    a_finished_run_reopened_after_restart_reproduces_the_same_blocks();
    a_non_verdict_slot_written_to_an_already_selected_run_reconciles_through_the_served_detail_path(
    );
    no_slots_block_without_evidence_and_a_block_with_it();
    a_selection_change_to_a_run_without_slot_evidence_leaves_no_rows_behind();
}

/// D1 — a verdict written to an already-selected run reconciles through the
/// served detail path with no second request kind and no timer: `follow`
/// yields a resync `LoadRequest` off the moved `RunSummary.verdict_rounds`
/// fingerprint field, `apply` commits it, and the verdict block appears.
fn a_verdict_written_to_an_already_selected_run_reconciles_through_the_served_detail_path() {
    let guard = support::scratch("preview-state-blocks-d1");
    let root = guard.0.clone();
    // SAFETY: see module doc / detail_stale_recovery.rs.
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
    assert!(
        sessions_verdict_block(detail.preview_state().as_ref()).is_none(),
        "no verdict evidence yet: no block"
    );

    // Append a verdict revision to the scratch ledger, then move the wire
    // row's `RunSummary` (goal 8's fix moves `verdict_rounds`) and deliver
    // it as a `RowChanged`.
    let verdict_session = support::write_session_ledger_with_verdict(
        &ledger_path,
        "session-a",
        "run-a",
        true,
        "approved",
        None,
        None,
    );
    let moved_row = support::row_from_ledger("repo", &ledger_path, &verdict_session, true);
    assert_ne!(
        moved_row.summary.verdict_rounds, wire_row.summary.verdict_rounds,
        "the verdict write must move the fingerprint-bearing summary field"
    );

    let follow_outcome = detail.follow(&LinkUpdate::Delta(
        ctx_traits_io::center::CenterDelta::RowChanged {
            row: Box::new(moved_row),
        },
    ));
    let resync = follow_outcome
        .request
        .expect("a moved verdict-rounds fingerprint issues exactly one resync request");

    let resync_outcome = load_serving_claimed_task(&peer, &resync, "unclaimed", None);
    assert!(detail.apply(resync.generation, resync_outcome));

    let block = sessions_verdict_block(detail.preview_state().as_ref())
        .expect("the resync-committed baseline carries the verdict block");
    assert_eq!(block.heading, "verdict \u{b7} round 1");
    assert_eq!(block.status_row.value[0].text, "approved");

    drop(updates);
}

/// D2 — out-of-order A-then-B selection with A's outcome arriving last: B's
/// now item and verdict block are intact, and no combination of A's facts
/// with B's ever renders.
fn out_of_order_selection_never_mixes_one_selections_title_with_anothers_verdict() {
    let guard = support::scratch("preview-state-blocks-d2");
    let root = guard.0.clone();
    // SAFETY: see module doc.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_a =
        Utf8PathBuf::from_path_buf(root.join("repo").join("a.json")).expect("UTF-8 path");
    let ledger_b =
        Utf8PathBuf::from_path_buf(root.join("repo").join("b.json")).expect("UTF-8 path");
    let session_a = support::write_session_ledger_with_verdict(
        &ledger_a,
        "session-a",
        "run-a",
        true,
        "approved",
        Some("reviewing plan a"),
        None,
    );
    let session_b = support::write_session_ledger_with_verdict(
        &ledger_b,
        "session-b",
        "run-b",
        true,
        "revise",
        Some("reviewing plan b"),
        Some("blocker-b"),
    );

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let row_a = support::row_from_ledger("repo", &ledger_a, &session_a, true);
    let row_b = support::row_from_ledger("repo", &ledger_b, &session_b, true);
    connection.serve_snapshot(&subscribe_id, &[row_a, row_b]);

    let snapshot_rows = recv_snapshot(&updates);
    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);
    let selected_a = rows
        .iter()
        .find(|row| row.ledger_path == ledger_a.as_str())
        .expect("row a present");
    let selected_b = rows
        .iter()
        .find(|row| row.ledger_path == ledger_b.as_str())
        .expect("row b present");

    let mut detail = RunDetail::default();
    let request_a = detail.select(selected_a).expect("selects a");
    let request_b = detail.select(selected_b).expect("selects b");

    // Serve B's claimed-task request first, then A's — A's outcome arrives
    // last but must be dropped as superseded rather than committed.
    let outcome_b = load_serving_claimed_task(&peer, &request_b, "unclaimed", None);
    let outcome_a = load_serving_claimed_task(&peer, &request_a, "unclaimed", None);

    assert!(!detail.apply(request_a.generation, outcome_a));
    assert!(detail.apply(request_b.generation, outcome_b));

    let state = detail.preview_state();
    assert!(
        matches!(state, Some(PreviewState::Accepted { .. })),
        "the surviving selection must be accepted"
    );
    let now = sessions_now_item(state.as_ref()).expect("B's now item must render");
    assert_eq!(
        now.title, "reviewing plan b",
        "the surviving now item must carry B's own title, never A's"
    );
    let verdict =
        sessions_verdict_block(state.as_ref()).expect("B's own verdict evidence must render");
    assert_eq!(verdict.status_row.value[0].text, "revise");
    assert_eq!(
        verdict.blocker_lines,
        vec!["blocker-b".to_string()],
        "B's own blocker must render, never A's (which has none)"
    );

    drop(updates);
}

/// D3 — a rejected (stale-generation) candidate leaves the previously
/// accepted blocks whole.
fn a_rejected_candidate_leaves_the_previously_accepted_blocks_whole() {
    let guard = support::scratch("preview-state-blocks-d3");
    let root = guard.0.clone();
    // SAFETY: see module doc.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_path =
        Utf8PathBuf::from_path_buf(root.join("repo").join("session.json")).expect("UTF-8 path");
    let session = support::write_session_ledger_with_verdict(
        &ledger_path,
        "session-a",
        "run-a",
        true,
        "approved",
        Some("accepted-candidate-title"),
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
    let first_request = detail.select(row).expect("first selection");
    let first_outcome = load_serving_claimed_task(&peer, &first_request, "unclaimed", None);
    assert!(detail.apply(first_request.generation, first_outcome));
    let accepted_before = sessions_verdict_block(detail.preview_state().as_ref())
        .expect("verdict block present before the stale reject");
    assert_eq!(accepted_before.status_row.value[0].text, "approved");
    assert!(accepted_before.blocker_lines.is_empty());

    // Move the ledger to a full selected-detail tuple the reader would
    // notice on every axis — title, status and blockers all differ from the
    // accepted candidate — so a rejected candidate that leaked any single
    // field would fail this assertion, not just a status-only mismatch.
    support::write_session_ledger_with_verdict(
        &ledger_path,
        "session-a",
        "run-a",
        true,
        "revise",
        Some("rejected-candidate-title"),
        Some("rejected-candidate-blocker"),
    );

    // A stale generation number must be rejected outright, even though the
    // load it carries would read the moved (now-differing) ledger content.
    let stale_outcome = load_serving_claimed_task(&peer, &first_request, "unclaimed", None);
    assert!(
        !detail.apply(first_request.generation.wrapping_sub(1), stale_outcome),
        "an older generation must never overwrite the accepted candidate"
    );

    let accepted_after = sessions_verdict_block(detail.preview_state().as_ref())
        .expect("verdict block untouched by the rejected candidate");
    assert_eq!(
        accepted_before, accepted_after,
        "the rejected candidate's differing content (title, status, blockers) must never appear"
    );
    assert_eq!(accepted_after.status_row.value[0].text, "approved");
    assert!(accepted_after.blocker_lines.is_empty());

    let now_after = sessions_now_item(detail.preview_state().as_ref())
        .expect("now item untouched by the rejected candidate");
    assert_eq!(now_after.title, "accepted-candidate-title");

    drop(updates);
}

/// D9 — an implement-shaped run with a numbered `review-verdict-1` revision
/// renders a verdict block with a round; a run with no verdict evidence at
/// all renders no block.
fn an_implement_shaped_run_renders_a_verdict_block_and_a_plain_run_renders_none() {
    let guard = support::scratch("preview-state-blocks-d9");
    let root = guard.0.clone();
    // SAFETY: see module doc.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_verdict =
        Utf8PathBuf::from_path_buf(root.join("repo").join("verdict.json")).expect("UTF-8 path");
    let ledger_plain =
        Utf8PathBuf::from_path_buf(root.join("repo").join("plain.json")).expect("UTF-8 path");
    let session_verdict = support::write_session_ledger_with_verdict(
        &ledger_verdict,
        "session-verdict",
        "run-verdict",
        false,
        "approved",
        None,
        None,
    );
    let session_plain =
        support::write_session_ledger(&ledger_plain, "session-plain", "run-plain", false);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let row_verdict = support::row_from_ledger("repo", &ledger_verdict, &session_verdict, false);
    let row_plain = support::row_from_ledger("repo", &ledger_plain, &session_plain, false);
    connection.serve_snapshot(&subscribe_id, &[row_verdict, row_plain]);

    let snapshot_rows = recv_snapshot(&updates);
    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);

    let selected_verdict = rows
        .iter()
        .find(|row| row.ledger_path == ledger_verdict.as_str())
        .expect("verdict row present");
    let mut detail = RunDetail::default();
    let request = detail
        .select(selected_verdict)
        .expect("selects verdict run");
    let outcome = load_serving_claimed_task(&peer, &request, "unclaimed", None);
    assert!(detail.apply(request.generation, outcome));
    let block = sessions_verdict_block(detail.preview_state().as_ref())
        .expect("a numbered review-verdict revision must render a verdict block");
    assert_eq!(block.heading, "verdict \u{b7} round 1");

    let selected_plain = rows
        .iter()
        .find(|row| row.ledger_path == ledger_plain.as_str())
        .expect("plain row present");
    let request = detail.select(selected_plain).expect("selects plain run");
    let outcome = load_serving_claimed_task(&peer, &request, "unclaimed", None);
    assert!(detail.apply(request.generation, outcome));
    assert!(
        sessions_verdict_block(detail.preview_state().as_ref()).is_none(),
        "a run with no recognised verdict evidence renders no verdict block"
    );

    drop(updates);
}

/// D7 — a `RowChanged` for a row that is not the current selection fires no
/// re-read.
fn no_read_fires_for_an_unselected_row_change() {
    let guard = support::scratch("preview-state-blocks-d7");
    let root = guard.0.clone();
    // SAFETY: see module doc.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_a =
        Utf8PathBuf::from_path_buf(root.join("repo").join("a.json")).expect("UTF-8 path");
    let ledger_b =
        Utf8PathBuf::from_path_buf(root.join("repo").join("b.json")).expect("UTF-8 path");
    let session_a = support::write_session_ledger(&ledger_a, "session-a", "run-a", true);
    let session_b = support::write_session_ledger(&ledger_b, "session-b", "run-b", true);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let row_a = support::row_from_ledger("repo", &ledger_a, &session_a, true);
    let row_b = support::row_from_ledger("repo", &ledger_b, &session_b, true);
    connection.serve_snapshot(&subscribe_id, &[row_a, row_b.clone()]);

    let snapshot_rows = recv_snapshot(&updates);
    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);
    let selected_a = rows
        .iter()
        .find(|row| row.ledger_path == ledger_a.as_str())
        .expect("row a present");

    let mut detail = RunDetail::default();
    let request = detail.select(selected_a).expect("selects a");
    let outcome = load_serving_claimed_task(&peer, &request, "unclaimed", None);
    assert!(detail.apply(request.generation, outcome));

    let mut changed_b = row_b;
    changed_b.modified_epoch_secs += 1;
    let follow_outcome = detail.follow(&LinkUpdate::Delta(
        ctx_traits_io::center::CenterDelta::RowChanged {
            row: Box::new(changed_b),
        },
    ));
    assert!(
        follow_outcome.request.is_none(),
        "a delta for an unselected row must never issue a read"
    );
    assert!(matches!(
        detail.load_state(),
        Some(DetailLoad::Loaded { .. })
    ));

    drop(updates);
}

/// D8 — a finished run reopened after the harness restarts (a fresh
/// `RunDetail`, same durable ledger) reproduces the same title, marker,
/// heading round, status and blocker lines from durable evidence alone.
fn a_finished_run_reopened_after_restart_reproduces_the_same_blocks() {
    let guard = support::scratch("preview-state-blocks-d8");
    let root = guard.0.clone();
    // SAFETY: see module doc.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_path =
        Utf8PathBuf::from_path_buf(root.join("repo").join("session.json")).expect("UTF-8 path");
    let session = support::write_session_ledger_with_verdict(
        &ledger_path,
        "session-a",
        "run-a",
        false,
        "revise",
        Some("finalize the report"),
        Some("restart-blocker"),
    );

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

    let mut first_detail = RunDetail::default();
    let request = first_detail.select(row).expect("first-session selection");
    let outcome = load_serving_claimed_task(&peer, &request, "unclaimed", None);
    assert!(first_detail.apply(request.generation, outcome));
    let before = sessions_verdict_block(first_detail.preview_state().as_ref())
        .expect("verdict block present before the restart");
    assert_eq!(before.blocker_lines, vec!["restart-blocker".to_string()]);
    assert_eq!(before.status_row.value[0].text, "revise");

    // Simulate the harness restarting: a fresh `RunDetail` selecting the
    // same durable row, re-reading the same on-disk ledger.
    let mut second_detail = RunDetail::default();
    let request = second_detail
        .select(row)
        .expect("second-session (post-restart) selection");
    let outcome = load_serving_claimed_task(&peer, &request, "unclaimed", None);
    assert!(second_detail.apply(request.generation, outcome));
    let after = sessions_verdict_block(second_detail.preview_state().as_ref())
        .expect("verdict block present after the restart");

    assert_eq!(
        before, after,
        "durable evidence must reproduce the same block"
    );

    drop(updates);
}

/// Goal 8's central claim, widened past the verdict slot D1 already covers:
/// an arbitrary **non-verdict** slot written to an already-selected run
/// becomes visible through the one served detail path `0265.11` landed —
/// no second request kind, no timer.
fn a_non_verdict_slot_written_to_an_already_selected_run_reconciles_through_the_served_detail_path()
{
    let guard = support::scratch("preview-state-blocks-slots-d1");
    let root = guard.0.clone();
    // SAFETY: see module doc.
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
    assert!(
        sessions_slots_block(detail.preview_state().as_ref()).is_none(),
        "no slot evidence yet: no block"
    );

    let scratch_session = support::write_session_ledger_with_slot(
        &ledger_path,
        "session-a",
        "run-a",
        true,
        "scratch-note",
        serde_json::json!("arbitrary evidence"),
    );
    let mut moved_row = support::row_from_ledger("repo", &ledger_path, &scratch_session, true);
    // A non-verdict slot write never moves `RunSummary.verdict_rounds`, so
    // the fingerprint here rests entirely on `modified_epoch_secs`; force it
    // past the write's own mtime rather than trust two writes inside one
    // wall-clock second to land in different seconds (the plan's own
    // documented same-second reconciliation bound).
    moved_row.modified_epoch_secs = wire_row.modified_epoch_secs + 1;

    let follow_outcome = detail.follow(&LinkUpdate::Delta(
        ctx_traits_io::center::CenterDelta::RowChanged {
            row: Box::new(moved_row),
        },
    ));
    let resync = follow_outcome
        .request
        .expect("a moved fingerprint issues exactly one resync request");

    let resync_outcome = load_serving_claimed_task(&peer, &resync, "unclaimed", None);
    assert!(detail.apply(resync.generation, resync_outcome));

    let block = sessions_slots_block(detail.preview_state().as_ref())
        .expect("the resync-committed baseline carries the slots block");
    assert_eq!(block.heading, "slots");
    assert_eq!(block.rows.len(), 1);
    assert_eq!(block.rows[0].key, "scratch-note");
    assert_eq!(block.rows[0].value[0].text, "filled");

    drop(updates);
}

/// No block at all for a run with no slot evidence; the block is present for
/// one that has it (goal 2).
fn no_slots_block_without_evidence_and_a_block_with_it() {
    let guard = support::scratch("preview-state-blocks-slots-presence");
    let root = guard.0.clone();
    // SAFETY: see module doc.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_none =
        Utf8PathBuf::from_path_buf(root.join("repo").join("none.json")).expect("UTF-8 path");
    let ledger_some =
        Utf8PathBuf::from_path_buf(root.join("repo").join("some.json")).expect("UTF-8 path");
    let session_none =
        support::write_session_ledger(&ledger_none, "session-none", "run-none", false);
    let session_some = support::write_session_ledger_with_slot(
        &ledger_some,
        "session-some",
        "run-some",
        false,
        "scratch-note",
        serde_json::json!("arbitrary evidence"),
    );

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let row_none = support::row_from_ledger("repo", &ledger_none, &session_none, false);
    let row_some = support::row_from_ledger("repo", &ledger_some, &session_some, false);
    connection.serve_snapshot(&subscribe_id, &[row_none, row_some]);

    let snapshot_rows = recv_snapshot(&updates);
    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);

    let selected_none = rows
        .iter()
        .find(|row| row.ledger_path == ledger_none.as_str())
        .expect("no-evidence row present");
    let mut detail = RunDetail::default();
    let request = detail
        .select(selected_none)
        .expect("selects no-evidence run");
    let outcome = load_serving_claimed_task(&peer, &request, "unclaimed", None);
    assert!(detail.apply(request.generation, outcome));
    assert!(
        sessions_slots_block(detail.preview_state().as_ref()).is_none(),
        "a run with no slot evidence renders no slots block"
    );

    let selected_some = rows
        .iter()
        .find(|row| row.ledger_path == ledger_some.as_str())
        .expect("evidence row present");
    let request = detail.select(selected_some).expect("selects evidence run");
    let outcome = load_serving_claimed_task(&peer, &request, "unclaimed", None);
    assert!(detail.apply(request.generation, outcome));
    let block = sessions_slots_block(detail.preview_state().as_ref())
        .expect("a run with slot evidence renders a slots block");
    assert_eq!(block.rows.len(), 1);
    assert_eq!(block.rows[0].key, "scratch-note");

    drop(updates);
}

/// A selection change from a run with rows to a run without leaves no rows
/// behind (goal 2's second half): the atomic selection transaction, not a
/// half-replaced or stale block.
fn a_selection_change_to_a_run_without_slot_evidence_leaves_no_rows_behind() {
    let guard = support::scratch("preview-state-blocks-slots-transition");
    let root = guard.0.clone();
    // SAFETY: see module doc.
    let socket = unsafe { support::install_center_env(&root) };

    let ledger_some =
        Utf8PathBuf::from_path_buf(root.join("repo").join("some.json")).expect("UTF-8 path");
    let ledger_none =
        Utf8PathBuf::from_path_buf(root.join("repo").join("none.json")).expect("UTF-8 path");
    let session_some = support::write_session_ledger_with_slot(
        &ledger_some,
        "session-some",
        "run-some",
        false,
        "scratch-note",
        serde_json::json!("arbitrary evidence"),
    );
    let session_none =
        support::write_session_ledger(&ledger_none, "session-none", "run-none", false);

    let peer = support::FakePeer::bind(&socket);
    let updates = ctx_traits_desktop::center_link::start(None);

    let mut connection = peer.accept();
    let subscribe_id = connection.read_subscribe_id();
    let row_some = support::row_from_ledger("repo", &ledger_some, &session_some, false);
    let row_none = support::row_from_ledger("repo", &ledger_none, &session_none, false);
    connection.serve_snapshot(&subscribe_id, &[row_some, row_none]);

    let snapshot_rows = recv_snapshot(&updates);
    let rows = run_row::project(&snapshot_rows, &run_row::RepoScope::All);

    let selected_some = rows
        .iter()
        .find(|row| row.ledger_path == ledger_some.as_str())
        .expect("evidence row present");
    let mut detail = RunDetail::default();
    let request = detail.select(selected_some).expect("selects evidence run");
    let outcome = load_serving_claimed_task(&peer, &request, "unclaimed", None);
    assert!(detail.apply(request.generation, outcome));
    assert!(sessions_slots_block(detail.preview_state().as_ref()).is_some());

    let selected_none = rows
        .iter()
        .find(|row| row.ledger_path == ledger_none.as_str())
        .expect("no-evidence row present");
    let request = detail
        .select(selected_none)
        .expect("selects no-evidence run");
    let outcome = load_serving_claimed_task(&peer, &request, "unclaimed", None);
    assert!(detail.apply(request.generation, outcome));
    assert!(
        sessions_slots_block(detail.preview_state().as_ref()).is_none(),
        "the prior selection's rows must never survive a selection change"
    );

    drop(updates);
}
