# Screen: Config (Runtime)

Reference: `../reference/config.png` / `.html`.

## Layout

Title bar (menu: Config bright) → rail 260 · main · preview 380.

## Main pane

- Header: title `runtime — ctx-company/traits`; summary italic "Three
  seats, two engines, one trust list — runtime.toml as loaded." — data:
  derivable counts; phrasing placeholder:.
- Sections `Agents` / `Runtime` / `Trust` — config rows (no dots; config is
  settings, not state):
  - left stack: name sans 12.5 (`text`; selected bright+raised) +
    description sans 11 `text-secondary`.
  - right stack mono 10.5 (align end): value top `text-secondary`
    (model id, `opencode 0.4.2`, path), qualifier bottom `text-muted`
    (`effort high`, `medium`, `low`).
  - Agents: smart / worker / scribe — data: runtime.toml seats.
  - Runtime: harness / center / store — data.
  - Trust: `approved traits` row, desc = trait names, right `5 digests` —
    data: trust list.
- Bottom bar: `valid` ok `· runtime.toml · loaded 13:02` |
  `edit runtime.toml` — data; edit action opens the file (v1: reveal in
  editor placeholder: acceptable).

## Preview column (selected seat)

- `seat` block: kv name `smart · strong seat` + description sans 11.5
  `text-secondary` — data: seat role; prose placeholder:.
- `facts` block (kv): model / effort / used by / source — data.
- Footer: `runtime.toml · valid · 13:02` — data.
