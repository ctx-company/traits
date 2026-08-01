# 0044 — Live view pane polish: identify the run, name the pane for what it holds, kill the double label

**Status:** ready to implement · **Raised:** 2026-07-30 (owner, while watching a live run)

Three small fixes to the P552 four-pane live view, all in
`modules/cli/src/app/run_view.rs` unless noted.

## 1. Session & run ids are missing from the progress pane

The pane carries the run's standing facts (phase, harness assignments,
progress line) but not the two identifiers everything else keys on. An owner
watching a live pane cannot tell WHICH session/run this is without leaving the
view — every diagnostic path in this repo starts from those ids
(`run-status --session …`, ledger paths, `check run-…`), so the view that is
open during a problem is the one place they must be readable.

Add two rows (short forms acceptable, e.g. first 12 hex of each, as the
dashboard already abbreviates): `session <id>` and `run <id>`. Both are on the
session the panel already holds — no new data source.

## 2. Rename the `progress` pane to `information`

The pane holds standing facts — inputs, seats, ids, progress — and "progress"
describes only one row of it. Owner call 2026-07-30: it is the information
pane. Touch points: `PROGRESS_PANE` id (`run_view.rs:192`), the pane title
(`:1062`), and the dashboard's preview counterparts (`dashboard.rs` uses the
same pane set for preview/attach — keep the id change consistent across the
shared renderer, which after P552 is one place by design).

Decide whether the internal `PaneId` string changes with the display title or
only the title does; scroll/focus state keys on pane ids
(`SESSION_PANE_IDS`), so if the id changes, change it everywhere in the same
commit — a half-renamed id silently detaches focus handling.

## 3. Drop the inner "journey" label — it renders twice

The journey pane draws its own bordered title ("journey", `:1066`) AND the
legacy in-content muted label (`muted_line(&mut lines, "journey")`,
`:1951`) survives from the pre-P552 single-column layout — so the word
appears twice, stacked. Delete the in-content label; the pane border owns the
name now. Check the same function for a sibling "landing"/"arrival" muted
label with the same duplication (the P549 merge rows lived in the same flat
list).

## Watch

- The narrow-terminal fallback and the dashboard preview render from the same
  pane data (P552's one-renderer contract) — verify all three surfaces after
  the rename, not just the live TUI.
- `area.width < 109` breakpoint and `CURRENT_MIN_OUTER_ROWS` assume current
  title widths; "information" is longer than "progress" — confirm no
  truncation at the narrow boundary.
- Tests pin pane ids in `run_view.rs`/`dashboard.rs` test modules
  (`:4116-4117` expects `PROGRESS_PANE`); update alongside.

## Done when

The information pane shows session and run ids; no pane is labeled
"progress"; "journey" appears exactly once on screen; live, preview, and
attached surfaces all render the renamed pane correctly at normal and narrow
widths.
