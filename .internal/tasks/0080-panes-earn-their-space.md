# 0080 — Panes earn their space: drop an empty history, drop startup's detail column

**Status:** ready to implement · **Depends on:** nothing · **Raised:** 2026-08-04 (owner)

Two small ones. Both are a pane holding screen width while saying nothing.

## Decisions

- **An empty history pane is not rendered.** `pane_tree`
  (`run_view.rs:1544`) already composes conditionally — it branches on
  `data.progress.is_some()` and `data.journey.is_some()` and picks a different
  split for each combination. History is simply not one of those conditions.
  Give it the same treatment rather than inventing a second mechanism.
- **Run startup has no detail column.** `run_startup_view.rs` splits 70/30 into
  `startup-stages` and a `Detail` leaf. During startup the detail is one line
  that the stage rows already carry; the column costs 30% of the width to
  restate them. The stage list takes the full width.
- **Empty means nothing to show, not zero rows yet.** A pane that will fill a
  moment later should not flicker in and out — decide on whether the run has
  produced that kind of content at all, not on the current row count.

## Scope

The history arm of `pane_tree`, and the startup pane tree in
`run_startup_view.rs::render`.

## Watch

- The combinations multiply: `pane_tree` already has a `(has_progress,
  has_journey)` match, and history makes it eight. If that reads badly, the
  fix is to compute the visible set first and lay it out from that, not to add
  a third boolean to the same match.
- P552's one-renderer contract: live, preview and attached surfaces share this
  renderer, and 0048/0053/0055 touch it too. Check both width breakpoints.
- Startup's detail leaf is also where a FAILURE detail lands. Removing the
  column must not remove the failure text with it — that has to survive on the
  stage row itself, which is where `startup_pty_commits_each_failed_stage_
  before_restoring_the_terminal` will hold it.

## Done when

A run with no history renders no history pane and its width goes to the panes
that have content; a run that gains history gains the pane; run startup renders
its stages full width with no detail column; and a failing startup stage still
shows why it failed.
