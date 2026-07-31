# 0048 — TUIs fill the terminal, and the dashboard stops flashing "Loading…"

**Status:** ready to implement · **Raised:** 2026-07-31 (owner, while watching live runs)

## 1. Every TUI takes full height and width with a real pane layout

All interactive surfaces — the live run view, the dashboard (and its preview/attach panes),
and any story/merge interactive views — must occupy the FULL terminal: full height, full
width, panes laid out to spend the space, at every terminal size the breakpoints support.
No surface may render as a shrunken or partial-height block with dead terminal area around
it, and resizing the terminal must re-layout to the new full size.

Where: `modules/cli/src/app/run_view.rs`, `modules/cli/src/app/dashboard.rs`, and the shared
kit (`tui_ratatui.rs` / `tui_kit.rs`) — P552's one-renderer contract means the fix belongs
in the shared layout path, not per-surface patches. Check the narrow-terminal fallback and
`CURRENT_MIN_OUTER_ROWS`-style constants while there: "full size" includes the degraded
layouts, which should still span the terminal.

Adjacent, not folded in: task 0044 (live-view pane polish — ids in the information pane,
pane rename, duplicate journey label). Same files; do them together if convenient, but 0044
keeps its own contract.

## 2. Dashboard must not show "Loading…" at refresh intervals

The dashboard periodically repaints a "Loading…"-style placeholder while its inventory
refreshes, so a steady view flickers to a loading state on an interval even when nothing
changed. Refresh must be non-destructive: keep rendering the last complete state until the
new snapshot is ready, then swap — the loading text may appear at most once, on true first
paint before any data has ever loaded. If a refresh fails, show the stale data with an
unobtrusive staleness marker, never a placeholder that erases the screen's content.

Where: the dashboard's tick/refresh path (`dashboard.rs`, its `InventoryCache` consumers) —
find where the placeholder branch renders and make it first-paint-only.

## Done when

Every TUI surface spans the full terminal at normal and narrow sizes, including after a
live resize, with panes consuming the space; an idle dashboard left open shows zero
"Loading…" flashes across refresh intervals (placeholder appears only before first data);
and the shared renderer, preview, and attached views all inherit the layout fix from one
place rather than per-surface copies.
