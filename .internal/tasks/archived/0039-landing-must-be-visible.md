# 0039 — The landing must be visible: compact the journey once it is done

**Status:** ready to implement · **Raised:** 2026-07-29

## The behaviour

A run merges and removes its worktree and the owner never sees it happen.

## Why

The section already exists. P549 renders `landing` inline, directly after the
run's own steps (`modules/cli/src/app/run_view.rs:1510-1516`), deliberately so
that "the landing is one continuous journey rather than a separate section."
That was the right call for a short run. It does not survive a long one:

`render_step_group` (`run_view.rs:1814`) emits **three lines per step** — a
summary, an `in` port line, an `out` port line. A seven-round `implement` run is
one draft step plus 7 × 4 loop steps ≈ 29 groups ≈ **87 rows before landing
begins**. On a normal terminal the landing is several screens below the fold, at
exactly the moment the run stops producing new output to scroll toward — and
getting there depends on scrolling, which is independently broken (0006, 0014).

So the feature works and is unobservable. That is the whole defect.

## What to build

**1. Compact the journey when the run completes.** On completion, collapse each
finished step group to its summary line and drop the `in`/`out` port lines. Three
lines become one, ~87 rows become ~29, and the landing rises into view without
scrolling. The port detail is not deleted — it is what the expanded/history view
is for, and a completed run is precisely when the per-step data flow has stopped
being the thing the owner is watching.

Compact on *completion*, not progressively: while the run is live the `in`/`out`
lines are the most informative rows on the screen.

**2. Keep the landing visible while it runs.** Whether it stays inline or becomes
its own pane under the journey is an implementation detail; the requirement is
that a merge in progress is on screen without the owner scrolling to find it. If
(1) frees enough rows, inline is still the better shape and P549's reasoning
stands. Measure after (1) before adding a pane.

**3. Rename `landing`.** It reads as jargon next to `journey`. Recommendation:
**`arrival`** — it completes the metaphor `journey` already sets up and is a
plain noun in the house style. `merge` is the honest literal alternative and is
better if the metaphor is not worth keeping. Owner's call; the string appears at
`run_view.rs:1512` and in the P549 tests around `"merge:landing"` (`:3069`).

## Watch

- **Do not compact into a summary that hides a failure.** A step that failed, was
  retried, or produced a rejected submission must stay legible after compaction —
  collapsing is for the routine rows, and a run that ended badly is exactly when
  the detail is wanted. Decide what "routine" means before writing the fold.
- **This interacts with 0016 and 0033.** 0016 wants the journey to read as a
  story and 0033 wants loop steps indented; both change the same rows. Land
  whichever is first, then re-measure the row budget rather than assuming the
  ~29-row figure above still holds.
- Compaction changes what `active_row` points at (`run_view.rs:1495-1502`), which
  is what the viewport scrolls to. A fold that forgets to remap it will scroll to
  the wrong step at the moment of completion — the same class of bug as 0014.
- The landing is also where `git worktree remove` reports, and that removal now
  takes minutes on a repo with per-worktree `target/` dirs (2026-07-29 timeout
  fix). Visible progress there is worth more than it looks: it is the difference
  between "finished" and "apparently hung."

## Done when

A completed run shows its landing on screen without scrolling; finished steps
collapse to one row each while failures stay legible; a merge in progress is
visible as it runs; and the section has a name that reads like the rest of the
view.
