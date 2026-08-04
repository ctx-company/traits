# 0082 — The journey pane follows the current step, showing as much of its ladder as fits

**Status:** ready to implement · **Depends on:** nothing · **Raised:** 2026-08-04 (owner)

When a run moves from one step to the next, the step it moved to should be
visible without scrolling. Today it can sit below the fold, and the pane keeps
showing where the run used to be.

## Decisions

- **The target is the deepest pending step, not its container.** Inside a
  loop, that is the actual pending step of the current iteration — a run does
  not "do a loop", it does a step within one.
- **The target goes to the TOP of the viewport.** The existing
  `FollowTarget::ActiveRow` (`run_view.rs:1363`) computes
  `active_row - (rows - 1)`, which pins the target to the BOTTOM — everything
  visible is history and the work ahead is off-screen. Following forward wants
  the opposite.
- **Prefer the outermost enclosing container that still fits — the ladder.**
  If the current step and its enclosing loop's header both fit, the loop
  header is the top line and the step is visible beneath it. If not, step in
  one level and try again. With loops inside loops this repeats, so the pane
  always shows the most context the height allows and never less than the
  current step itself.
- **The step itself is the floor.** When even one enclosing header does not
  fit, the current step is the top line. It is never scrolled out to preserve
  context — context is the thing that yields.
- **Following yields to the reader and resumes.** `journey_follow`
  (`run_view.rs:245`) and `is_following` already model this for the existing
  targets; scrolling by hand must stop the auto-scroll and returning to the
  follow position must resume it. A pane that fights the reader is worse than
  one that does not follow at all.

## Scope

A journey follow target that resolves the ladder, in `FollowTarget` beside the
existing `Tail` and `ActiveRow` alignments; the position-path walk that finds
the deepest pending step and its enclosing containers; the journey arm of the
follow wiring.

## Watch

- The ladder is a property of the POSITION PATH, not of the rendered text.
  `SequenceStatus::position_path` (`state.rs:266`) already carries the
  procedure/loop/item segments with their iteration indices — resolve the
  ladder from those rather than by pattern-matching indentation in the lines.
- **Recompute on step CHANGE, not on every repaint.** The journey re-renders
  constantly while a step runs; recomputing the scroll each time risks a pane
  that twitches, and makes "the reader scrolled away" hard to distinguish from
  "the follow position moved".
- A loop's header row and its current iteration can be far apart once the
  iteration has produced many rows. The ladder decision is about what FITS in
  `rows`, so it has to be made against the real viewport height at render
  time, not a guess.
- P552's one-renderer contract: live, preview and attached surfaces share this
  renderer, so verify at both width breakpoints — and note 0081 makes the
  attached surface the same view, so this lands there too.

## Done when

Advancing from one step to the next leaves the new step visible without manual
scrolling; the deepest pending step is the target, including inside nested
loops; the outermost enclosing container that fits is the top line and the
current step is the top line when none fits; scrolling by hand suspends the
follow and returning resumes it; and the pane does not twitch while a single
step runs.
