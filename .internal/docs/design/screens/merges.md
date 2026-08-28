# Screen: Merges (Queue)

Reference: `../reference/merges.png` / `.html`.

NOTE: no merge-queue surface exists in the backend today. This screen is
the most placeholder-heavy of v1 — render the design faithfully from the
`placeholders` module, structured so each region can be wired when a merge
surface lands. Landed rows MAY be derived from git log of the default
branch (runs stamp their landings); treat that as optional wiring, not a
requirement.

## Layout

Title bar (menu: Merges bright — new menu item between Tasks and Config) →
rail 260 · main · preview 380.

## Main pane

- Header: title `merges — ctx-company/traits`; summary italic "One landing
  now · two awaiting your approval." — placeholder:.
- Sections `Landing — 1` / `Awaiting approval — 2` / `Landed — 2`; merge
  rows shaped exactly like task rows (dot + title/desc | state stack):
  - Landing (selected): dot `accent`, title `doctor schema guard`, desc
    `run-1a2b3c → main · guarded-change · closes 0257`; right
    `landing · deep merge` accent / `gates green · +27 −1` — placeholder:.
  - Awaiting: dot `warn`, right `awaiting approval` warn / stats+signoff
    line muted — placeholder:.
  - Landed: dot `ok`, right `landed` / commit sha muted — placeholder:
    (optionally data: from git log).
- Bottom bar: `landing` accent `· run-1a2b3c → main · deep merge` |
  `watch · hold` — placeholder:.

## Preview column (selected merge)

- `merge` block: kv run / target (`main · deep`) / state (`landing`
  accent) + prose description — placeholder:.
- `gates` block (kv): cargo test / drift · embed / ts-format / worktree —
  all `pass`/`clean` in ok — data: gate results exist per run; wiring
  optional in v1, else placeholder:.
- `sign-offs` block: rows agent mono 10.5 identity-color 0.8 left
  (`architect@1.4`, `security@0.9`, `@oskar` in `text-session`), right
  role · time mono 10 `text-faint` — placeholder: until sign-offs exist.
- `landing` block: "Merges deep into main · closes 0257 · removes
  wt-1a2b3c" — placeholder:.
- Footer: `merge queue · 3 pending · 2 landed this week` — placeholder:.
