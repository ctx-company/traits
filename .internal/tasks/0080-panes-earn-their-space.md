# 0080 — Panes earn their space: drop an empty history, drop startup's detail column, keep machinery out of history

**Status:** ready to implement · **Depends on:** nothing · **Raised:** 2026-08-04 (owner)

Three small ones. Each is screen space spent on something that says nothing.

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
- **History is the narrative, not the machinery.** Command steps — staging,
  committing, capturing a diff — are how the run does its work, not what
  happened in it. They belong in the ledger and the story, not in the pane an
  owner watches. Drop them.
- **`00:00:00` is a symptom, and it gets fixed on its own terms too.**
  `story_row_line` (`run_view.rs:2181`) computes
  `summary_at.or(elapsed).unwrap_or_default()`, so a step with neither prints a
  fabricated zero timestamp. Filtering commands out removes today's instance;
  it does not stop the next kind of row from doing the same. A row with no
  time should render without one rather than inventing midnight.
- **A check step's verdict stays.** The repository gate is a check, and a red
  gate is exactly what history is for. "Command" here means command-kind
  steps, not everything that runs a process. If that reads wrong in practice,
  the rule to reach for is "did it produce a verdict the owner cares about",
  not a longer list of exempt ids.

## Scope

The history arm of `pane_tree`; the startup pane tree in
`run_startup_view.rs::render`; and the history row source.

`HistoryStep` (`run_view.rs:402`) has no kind, and neither does the
`SequenceStatus` it is built from (`item_id`, `title`, `status`, `reason`,
`position_path` — `state.rs:253`). The kind lives on the procedure item, which
the panel already holds via its trait and plan, so this needs the kind carried
onto `HistoryStep` at construction in `history_step_from_status` — not a
guess from the label text.

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

- Dropping command rows makes an all-command stretch of a run produce an EMPTY
  history — which is now a hidden pane, by the first decision above. The two
  interact, and a run whose first minutes are setup commands should not flash
  the pane in and out as it goes.

## Done when

A run with no history renders no history pane and its width goes to the panes
that have content; a run that gains history gains the pane; run startup renders
its stages full width with no detail column; a failing startup stage still
shows why it failed; command steps do not appear in history while a check
step's verdict still does; and no history row prints a timestamp it does not
have.
