# Screen: Sessions (Run — Live)

Reference: `../reference/sessions.png` / `.html`. Grammar rules apply; only
screen-specific facts here. Every leaf is tagged `data:` (source exists
today) or `placeholder:`.

## Layout

Title bar (menu: Sessions bright) → columns: rail 260 · main (fill) ·
preview 380.

## Main pane

- Header (rule 6): title `0243.4 — everyone asks the center` — data: task
  id + title of the claimed task. Summary italic: "Converting every session
  read to center subscriptions — the poll loop dies. Review round 2 · frame
  5 of 9." — data: run description + frame counter; the prose phrasing is
  placeholder: until a narrator seat serves it.
- Frame list (rows gap 4, clip + fade rule 9):
  - Done frame row: dot `ok` · step name sans 12.5 `text` · right time mono
    10.5 `text-muted` (`4s`, `3m 12s`) — data: ledger frames.
  - Loop narration line between frames: italic mono 10.5 `text-faint`
    "round 2 — refinement reviewed, review re-runs" — data: loop marker
    exists; copy phrasing placeholder:.
  - Current frame row: `surface-raised`, dot `accent`, name `text-bright`,
    second line description sans 11.5 `text-secondary` (w 430) — data: frame
    name; description placeholder: until frames carry intent text. Right:
    `live · round 2 · smart` mono 10.5 `accent` — data: state/round/seat.
  - Activity fade under current row (rule 11, pad-left aligns under text,
    8 lines) — data: center activity deltas.
  - Pending rows: dot `dot-idle`, name `text-muted`, no right side — data.
- Bottom bar: `running` accent `· frame 5 of 9 · review round 2` |
  `watch raw · pause` — data: state + verbs (0243.5). `watch raw` opens the
  raw view — v1 may no-op placeholder:.

## Preview column

- `run` block (kv): trait `implement · basic`, run `run-ab12ef · 42m`,
  task `0243.4 · in review` — data.
- `in progress` block: bordered item, title `review — refinement round 2`,
  `live` accent, narrated line "reading the diff against 0243.3's
  contract." — title data:, narrated line placeholder:.
- `verdict — round 1` block: kv `status` → `blocked · 2 findings` (warn);
  two blocker lines mono 10.5 `text-secondary` — data: typed verdict slots.
- `slots` block (kv): `review-verdict-1 · filled · r1`, `work-summary ·
  filled`, `changed-files · 12 files` — data: slot ledger.
- `landing` block: sans 11.5 `text-secondary` lines (worktree, merge, task
  close) — data: run config.
- Footer: `run-ab12ef · started 13:44` — data.
