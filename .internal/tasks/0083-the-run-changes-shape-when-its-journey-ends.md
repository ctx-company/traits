# 0083 — The run changes shape when its journey ends

**Status:** ready to implement · **Depends on:** nothing; overlaps 0080 (both touch the journey/current-activity panes) and 0082 (journey follow) — coordinate rather than land blind · **Raised:** 2026-08-04 (owner)

Four changes to what happens after a run's own steps finish. Three are layout,
one is a name.

## Where things are today

Landing is not a pane. `journey_lines_with_active_row` (`run_view.rs:2708`)
appends a `landing` label and the merge rows to the END of the journey pane's
line list, deliberately — the comment there says it makes "the landing one
continuous journey rather than a separate section" (P549).

**Items 3 and 4 reverse that decision.** That is the point of them, but the
implementer should know it is a recorded choice being overturned, not an
oversight being corrected.

## Decisions

- **`End` scrolls to the true end, landing rows included.** Page keys are
  routed through `pending_keys` rather than scrolling a pane directly
  (`run_view.rs:1278`, `:1968`); whatever that path computes as "the end" has
  to be the end of the content actually rendered, which now includes the
  landing rows appended after the steps. Same for `Home` and the true start.
- **"landing" is renamed.** It reads as jargon — the label is
  `muted_line(&mut landing, "landing")` at `run_view.rs:2714`, and the word
  also appears through `MergeStage::Landing` and `merge_story`'s prose. Pick
  one word for what this phase IS — reconciling, gating and publishing the
  work — and use it everywhere those three surfaces say "landing". "Post
  processing" is the owner's suggestion and is fine; the requirement is that
  it stops sounding like machinery and stays a phase name rather than a
  sentence.
- **A finished journey drops its `in`/`out` port lines.** `RunStep` carries
  `inputs`/`outputs` (`run_view.rs:474`) and renders them per step. While a
  run is moving they say what each step consumes and produces; once it is done
  they are noise between the reader and the shape of what happened. Show the
  completed steps, nothing else.
- **The finished journey takes over the current-activity pane.** Current
  activity has nothing to report once the run's steps are done, and the
  completed journey is what the reader wants to keep looking at.
- **The pane the journey vacates becomes the landing pane.** What is appended
  to the journey today gets its own space, at the moment it becomes the only
  thing still happening.
- **This is one transition, at one moment.** Both swaps happen together when
  the journey completes, not as two independent conditions that can disagree
  and produce a layout with no journey and no landing.

## Scope

`journey_lines_with_active_row`'s landing section; the pane tree's completed
arm; the `in`/`out` rendering on a done step; the page-key end/start
resolution; and the `landing` wording in `run_view`, `MergeStage` and
`merge_story`.

## Watch

- **A run can complete without landing at all** — `--no-merge`, a park, a
  merge that never starts. The vacated-journey pane must not become an empty
  box titled after a phase that will never run; decide what that pane is when
  there is no landing to show.
- **And landing can start before the journey looks done**, since a completed
  run whose merge is still going renders both today. Whatever triggers the
  swap has to be the journey's own completion, not the presence of merge rows.
- 0080 hides an empty history and gives command rows verdicts, and 0082
  changes journey follow. All three edit the same panes; landing them
  independently will conflict.
- P552's one-renderer contract: live, preview and attached surfaces share this
  renderer — and 0081 makes the attached surface the same view — so check both
  width breakpoints and the narrow single-leaf tree.
- Renaming a phase touches recorded park reasons and their translations.
  `merge_story` maps `ParkClass::Landing*` to owner-facing text; the internal
  identifiers can stay as they are, but any string a person reads should not
  be half-renamed.

## Done when

`End` and `Home` reach the true end and start of the journey including its
landing rows; the phase has one non-mechanical name everywhere a person reads
it; a completed journey shows its done steps with no `in`/`out` lines; the
completed journey occupies the current-activity pane and landing occupies the
journey's; the swap happens once, on journey completion; and a run that never
lands does not leave an empty pane behind.
