# Screen: Tasks (Board)

Reference: `../reference/tasks.png` / `.html`.

## Layout

Title bar (menu: Tasks bright) → rail 260 · main · preview 380.

## Main pane

- Header: title `tasks — ctx-company/traits`; summary italic "Seven open
  across three states — one live run, one waiting on you." — counts data:,
  phrasing placeholder: until narrated.
- Sections (`In progress — 2`, `Ready — 3`, `Draft — 2`): heading mono 11
  `text-muted`; rows gap 4 — data: .internal/tasks derived states.
- Task row (pad [8,12], space-between, alignItems start):
  - left: dot col (pad-top 6) + stack: title sans 12.5 (`text`; selected
    `text-bright` on `surface-raised`) + description sans 11
    `text-secondary` — data: task files.
  - right stack (align end, mono 10.5): top state, bottom meta.
    State colors: claimed+live → `live · run-<id>` accent; waiting →
    `awaiting owner` warn (dot `warn`); ready → `ready`/`deps met`;
    draft → dot `dot-dim`, `draft`/`blocked by <id>` — all data.
- Bottom bar: `synced` ok `· .internal/tasks @ <sha>` | `new task` — data:
  sha of the tasks tree; `new task` action data (creates draft).

## Preview column (selected task)

- Task lede (headerless): title sans 13 `text-bright` + full description
  sans 11.5 `text-secondary` lh 1.45 — data.
- `details` block (kv): id `0243.4 · in review`, claimed `run-ab12ef ·
  implement`, state `live · frame 5 of 9` (accent) — data.
- `facts` block (kv): status/raised/parent/depends-on — data.
- `checks` block: sans 11.5 `text-secondary` lines — data: task checks.
- `landing` block: "Closes automatically when run-ab12ef lands" — data.
- Footer: `.internal/tasks · synced 14:52` — data.
