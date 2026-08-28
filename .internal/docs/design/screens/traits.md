# Screen: Traits (Library)

Reference: `../reference/traits.png` / `.html`.

## Layout

Title bar (menu: Traits bright) → rail 260 · main · preview 380.

## Main pane

- Header: title `traits — authored in ctx-company/traits`; summary italic
  "Five traits, all digest-pinned — implement carries the dogfood loop." —
  counts data:, phrasing placeholder:.
- Section `Authored — 5`: trait rows (no dots — traits are not stateful in
  the frame sense):
  - left stack: name sans 12.5 (`text`; selected bright+raised) +
    description sans 11 `text-secondary` — data: trait manifests.
  - right stack mono 10.5: top `trusted` (ok) `· N variants`
    (`text-muted`), bottom variant names (`text-muted`) — data: trust list
    + manifest.
- Bottom bar: `pinned` ok `· skill-lock @ <sha> · 5 digests` |
  `author trait` — data: lockfile; `author trait` v1 placeholder: action.

## Preview column (selected trait)

- `trait` block: kv name `implement · trusted` (ok) + description sans
  11.5 `text-secondary` — data.
- `facts` block (kv): version / trust / digest / agents — data.
- `variants` block (kv): basic / quick / complex rows — data.
- `ports` block (kv): task / duration — data.
- Footer: `skill-lock @ <sha> · 5 digests` — data.
