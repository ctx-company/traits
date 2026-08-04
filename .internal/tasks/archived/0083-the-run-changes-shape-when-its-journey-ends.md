# 0083 — The run changes shape when its journey ends

**Status:** ready to implement · **Depends on:** nothing; overlaps 0080 (both touch the journey/current-activity panes) and 0082 (journey follow) — coordinate rather than land blind · **Raised:** 2026-08-04 (owner)

Three changes to what happens after a run's own steps finish: two layout, one a
name. (A fourth — dropping the per-step `in`/`out` lines — landed separately as
`9e08fc07`.)

## Where things are today

Landing is not a pane. `journey_lines_with_active_row` (`run_view.rs:2708`)
appends a `landing` label and the merge rows to the END of the journey pane's
line list, deliberately — the comment there says it makes "the landing one
continuous journey rather than a separate section" (P549).

**Items 3 and 4 reverse that decision, deliberately** (owner, 2026-08-04,
after living with it): the UI should MORPH in response to what is happening
rather than accumulate one long scroll, and that reads better across the
different harnesses than the continuous journey does. P549's choice is not
wrong so much as superseded — do not treat it as a constraint to preserve.

## Decisions

- **`End` scrolls to the true end, landing rows included.** Page keys are
  routed through `pending_keys` rather than scrolling a pane directly
  (`run_view.rs:1278`, `:1968`); whatever that path computes as "the end" has
  to be the end of the content actually rendered, which now includes the
  landing rows appended after the steps. Same for `Home` and the true start.
- **"landing" becomes "post-run"** (owner call), as one third of a vocabulary
  used everywhere: **pre-run, run, post-run**. The value is not the individual
  word — it is that the three phases are named as a set, so a reader who
  learns one has learned all three. Anywhere a surface currently invents its
  own word for one of these phases, it says the set's word instead.
- **Rename thoroughly, including the stored strings.** These reason strings
  are matched by text to translate already-recorded parks
  (`reason.starts_with(GATE_FAILED_PREFIX)`), so renaming them means ledgers
  written before the change lose their plain-language rendering and show the
  raw reason. **Accepted** (owner: alpha, breaking is fine). Do NOT add
  dual-spelling matching or a compatibility shim for it — carrying two
  vocabularies to spare old ledgers is a worse outcome than the ledgers
  reading rough.
- **"pre-landing gate" does not become "pre-run gate".** It is the one name
  that does not map mechanically: that gate runs INSIDE post-run, before the
  fast-forward — it has nothing to do with the phase before the run. It is the
  post-run gate. Getting this one wrong would leave the vocabulary actively
  misleading rather than merely inconsistent.
- ~~A finished journey drops its `in`/`out` port lines.~~ **DONE ELSEWHERE**:
  `9e08fc07` collapsed the three-row step display to one width-aware row and
  took the `in`/`out` lines out of the journey entirely, for every step rather
  than only finished ones. Nothing left to do here.
- **The journey does not move.** It stays exactly where the reader has been
  looking at it for the whole run. Nothing about finishing is a reason to
  relocate the one pane they have been tracking.
- **Post-run takes the entire right column**, replacing BOTH current
  activity and history with one full-height pane. Current activity has nothing
  left to report, and history is redundant with a journey that is now complete
  and visible — the journey already tells that story, so the space it was
  duplicating goes to the phase that is actually running.
- **One column changes, and only once.** The morph is a single substitution on
  the right, not a rotation of three panes through each other's positions. The
  point of the design is that the reader's eye does not have to re-find
  anything: the journey is where it was, and the new pane is where the panes
  that went quiet used to be.
- **The trigger is journey-done AND something still running** (owner call).
  Journey done with nothing after it: stop where it is, no morph — the
  finished journey stays where the reader last saw it. Journey done with a
  merge or post-gate underway: morph. So the condition is not "the journey
  finished" alone; it is "the journey finished and there is a next phase to
  give the space to".

## Scope

`journey_lines_with_active_row`'s landing section; the pane tree's completed
arm; the page-key end/start resolution; and the `landing` wording in
`run_view` and `merge_story`.

## Watch

- **The empty-pane case is answered by the trigger, not by a placeholder.**
  A run that never lands — dispatched without `--merge`, or with `--no-merge`
  clearing the intent (`drive.rs:158`) — simply never satisfies "something
  still running", so no morph happens and there is no pane to fill. Nothing
  needs an "operation not applicable" box. This is worth stating because the
  obvious reading of items 3 and 4 ("swap when the journey is done") produces
  exactly that box, and it is wrong.
- 0080 hides an empty history and gives command rows verdicts, and 0082
  changes journey follow. All three edit the same panes; landing them
  independently will conflict. 0080 in particular now has two reasons history
  can be absent — empty, and superseded by post-run — and they must not
  be implemented as two competing conditions on the same pane.
- The right column is not always two panes. `pane_tree` already picks
  different splits by what has content, including a narrow-terminal
  single-leaf tree, so "replace current activity and history" has to mean
  "take the whole right column, whatever it currently holds" rather than
  substituting two named leaves that may not both be there.
- P552's one-renderer contract: live, preview and attached surfaces share this
  renderer — and 0081 makes the attached surface the same view — so check both
  width breakpoints and the narrow single-leaf tree.
- **Rust identifiers stay.** `MergeStage::Landing`, `ParkClass::Landing*` and
  friends are never displayed, so renaming them buys nothing and only widens
  the diff. The vocabulary is a thing readers see, not a thing the compiler
  sees.
- The displayed wording is spread wider than the one journey label:
  `merge_story` alone carries `GATE_FAILED_PREFIX` (`"pre-landing gate "`),
  `GATE_DISK_FLOOR_PREFIX`, the dirty-worktree refusal, the stage prose at
  `:370-372`, and `MergeStage::Landing => "landing"` at `:573`. A rename that
  catches the label and misses these leaves the vocabulary half-applied, which
  is worse than not starting.

## Done when

`End` and `Home` reach the true end and start of the journey including its
post-run rows; the phase is called post-run everywhere a person reads it, with
the gate inside it named for post-run rather than pre-run, and no surface still
saying "landing"; the journey stays where it was and post-run replaces the
whole right column, current activity and history together, at full height; the
morph happens once, only when the journey is done AND a next phase is running;
and a run that ends with nothing after it simply stays as it was.
