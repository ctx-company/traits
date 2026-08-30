//! Integration proof for 0257.2: the real `RunRow -> select -> load ->
//! project` path, against a nested loop-body ledger plus a tolerant, partly
//! truncated activity sidecar. No env mutation, no center — the
//! `detail_reconstruction.rs` pattern.

mod support;

use camino::Utf8PathBuf;
use ctx_traits_desktop::detail::RunDetail;
use ctx_traits_desktop::detail_tree::FrameState;
use ctx_traits_desktop::{detail, detail_view, run_row};

#[test]
fn durable_hierarchy_and_states_survive_a_tolerant_partly_malformed_sidecar() {
    let guard = support::scratch("detail-frame-tree");
    let root = &guard.0;

    let ledger_path = Utf8PathBuf::from_path_buf(root.join("repo-a").join("nested-session.json"))
        .expect("UTF-8 ledger path");
    support::write_nested_session_ledger(&ledger_path, "nested-session", "nested-run");
    support::write_activity_sidecar(&ledger_path);

    let wire_rows = vec![support::wire_row(
        "repo-a",
        ledger_path.as_str(),
        "nested-run",
    )];
    let rows = run_row::project(&wire_rows, &run_row::RepoScope::All);
    let row = rows.first().expect("projected row present");

    let mut detail = RunDetail::default();
    let request = detail.select(row).expect("selection issues a request");
    let outcome = detail::load(&request);
    let skipped = outcome
        .as_ref()
        .expect("ledger and sidecar read")
        .skipped_activity_lines;
    assert!(detail.apply(request.generation, outcome));

    let tree = detail.tree().expect("loaded selection projects a tree");
    assert_eq!(tree.roots.len(), 1, "one top-level loop container");
    let container = &tree.roots[0];
    assert_eq!(
        container.state,
        FrameState::Pending,
        "the loop's own pending status is never overridden by its children"
    );
    assert_eq!(container.children.len(), 1, "one iteration group");
    let iteration = &container.children[0];
    assert_eq!(iteration.children.len(), 2, "two body items");
    assert_eq!(iteration.children[0].state, FrameState::Done);
    assert_eq!(iteration.children[1].state, FrameState::Ready);
    assert!(
        iteration.children[1].current,
        "the ready item is the current frame"
    );
    assert_eq!(
        iteration.children[1]
            .activity
            .as_ref()
            .and_then(|line| line.text.as_deref()),
        Some("editing the file"),
        "the sidecar's activity line attaches to the current frame"
    );
    assert_eq!(
        iteration.children[1].narration.as_deref(),
        Some("working on it")
    );

    assert_eq!(
        skipped, 1,
        "the truncated trailing sidecar line is counted by detail::load itself, not fatal"
    );

    // Deleting the sidecar entirely must not change the durable tree: a
    // fresh selection re-reads the (now sidecar-less) ledger and produces
    // structurally and state-wise identical roots.
    ctx_traits_io::activity_sidecar::remove_activity_for_ledger(&ledger_path);
    let mut without_sidecar = RunDetail::default();
    let request_without_sidecar = without_sidecar
        .select(row)
        .expect("selection issues a request");
    let outcome_without_sidecar = detail::load(&request_without_sidecar);
    assert!(without_sidecar.apply(request_without_sidecar.generation, outcome_without_sidecar));
    let tree_without_sidecar = without_sidecar
        .tree()
        .expect("loaded selection projects a tree");
    assert_eq!(tree_without_sidecar.roots, {
        // Activity/narration are the only fields the sidecar may touch;
        // clear them on the sidecar-backed tree before comparing the rest
        // structurally and state-wise.
        let mut roots = tree.roots.clone();
        clear_overlay(&mut roots);
        roots
    });
}

fn clear_overlay(nodes: &mut [ctx_traits_desktop::detail_tree::DetailNode]) {
    for node in nodes {
        node.activity = None;
        node.narration = None;
        clear_overlay(&mut node.children);
    }
}

#[test]
fn detail_element_renders_the_real_projected_tree_and_the_loading_failed_states() {
    let guard = support::scratch("detail-frame-tree-render");
    let root = &guard.0;
    let ledger_path = Utf8PathBuf::from_path_buf(root.join("repo-a").join("nested-session.json"))
        .expect("UTF-8 ledger path");
    support::write_nested_session_ledger(&ledger_path, "nested-session", "nested-run");

    let wire_rows = vec![support::wire_row(
        "repo-a",
        ledger_path.as_str(),
        "nested-run",
    )];
    let rows = run_row::project(&wire_rows, &run_row::RepoScope::All);
    let row = rows.first().expect("projected row present");

    let mut detail = RunDetail::default();
    let request = detail.select(row).expect("selection issues a request");
    let outcome = detail::load(&request);
    assert!(detail.apply(request.generation, outcome));
    let tree = detail.tree().expect("loaded selection projects a tree");

    let _rendered = detail_view::detail_element(&tree);
    let _loading = detail_view::loading_element();
    let _failed = detail_view::failed_element("bad json");
}
