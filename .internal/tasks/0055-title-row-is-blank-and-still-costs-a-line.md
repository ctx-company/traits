# 0055 — The run title never appears, and its reserved row leaves a blank line above the grid

**Status:** ready to implement · **Raised:** 2026-08-02 (owner, watching a live run)

## Symptom

The live view shows an empty line above the four panes where the run title used to be. Nothing
is rendered into it, and the panes start one row lower than they should.

## Two defects, and both need fixing

**1. The row is reserved whether or not there is a title.** `PaneTitleRow::Reserved(Option<&Line>)`
(`run_view.rs:1758`) always carves one row off the top via `pane_body_area`, but the render only
draws when the option is `Some`:

```rust
if let PaneTitleRow::Reserved(Some(title_line)) = &data.title { … }
```

So `Reserved(None)` = a blank row that costs height and shows nothing. A missing title must
collapse the row (`PaneTitleRow::None`) so the panes reclaim it; reserving space for a value that
does not exist is what produces the blank line.

**2. The title is not arriving.** `state.title` is only ever set by `RunPanel::set_title`, called
from one place — `drive.rs:3720`, at the end of the session-title narration path. That path
returns early without setting anything when: no narrator seat resolves (documented as
"permanently title-less"), the narration call fails (`let Ok(title) = result else { return }`), or
`record_session_title` fails to persist. Any of those leaves the panel title-less for the whole
run, which combined with defect 1 is exactly the blank row.

Find out WHICH of those it is on a current run before changing behaviour — the narrator seat
assignment, the narration call result, and the ledger write are three different failures with
three different fixes. `record_session_title` writes to the ledger, so a run's persisted
`session-title` (readable via `persisted_session_title` in `dashboard.rs:2906`) tells you whether
the title was ever produced: present in the ledger but missing from the view is a panel-wiring
bug; absent from both is a narration/seat problem.

## Watch

- The dashboard reads the same persisted title (`dashboard.rs:1738`, `:2699`, `:2757`) and shows
  it in session rows — check whether it is blank there too. Blank in both points at production;
  blank only in the live view points at the panel.
- A title that arrives mid-run must repaint into a row that was previously collapsed — the layout
  has to grow by a row at that moment without disturbing scroll positions or follow state.
- Keep the no-title case honest: no placeholder text, no "untitled" filler. The row simply is not
  there until a title exists.
- Same renderer as 0044/0048/0051/0053/0054; `title_row_line` is also used by the attached
  dashboard view, so verify all three surfaces and both width breakpoints.

## Done when

A run with a title shows it on the top row (title · trait · started-at, as `title_row_line`
composes it) in the live view and the attached view; a run without one shows no row at all and
the panes use the full height; a title arriving mid-run appears without disturbing scroll or
follow; and the reason titles were missing is identified and fixed at its own layer rather than
papered over with a placeholder.
