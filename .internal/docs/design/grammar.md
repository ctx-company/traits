# Visual grammar — normative rules for every ctx face surface

Each rule states the law, then an example from the reference screens.
Tokens referenced here are defined in `tokens.md`.

## 1 · Color is semantic, never decorative

- `accent` (blue) = happening **right now**: a live frame, a landing merge,
  a running state word. One surface should rarely show more than one or two
  accent elements — blue is a pointer, not a theme.
- `warn` (gold) = **needs the owner**: awaiting approval, open annotation,
  unresolved caveat, a task waiting on input.
- `ok` (green) = settled good: done frames, passing gates, `trusted`,
  `landed`, additions in diffs.
- `danger` (red) = failure and deletions only.
- `review` (purple) = the review/security agent's identity color.
- Agent identity colors appear at 0.8 opacity in small mono handles so they
  label without glowing. Dim variants (`*-dim`) are for the same meaning in
  an inactive/ambient position (e.g. a warn marker on a non-selected row).

Example: on Tasks — Board, exactly one row shows accent (`live · run-ab12ef`),
one shows warn (`awaiting owner`); everything else is neutral.

## 2 · States are words

`live`, `ready`, `draft`, `landed`, `complete`, `pass`, `valid`, `pinned`,
`synced`, `awaiting approval`, `discussing`, `idle`, `done`. Mono 10.5–11,
colored per rule 1. Never a checkmark, badge shape, or pill. Compound
states join with `·`: `live · round 2 · smart`.

## 3 · Glyphs are structural only

Allowed: `↰` (back/up navigation), `↳` (reply/origin reference),
`→` (consequence rows), `·` (separator), `∷` (context action button),
`–` (list bullet in prose docs). Nothing else — no icons, no emoji, no
arrows on action labels. No corner radius inside the canvas; radius 10
exists only on the window frame itself (device chrome).

## 4 · Dots mark status in stateful lists only

5px ellipse, colored by state (`ok` done, `accent` current/live,
`dot-idle` pending/ready, `dot-dim` draft, `warn`/`warn-dim` needs-owner).
Lists that qualify: frames, tasks, merges, repos in the rail. Prose,
chat turns, roster rows, sign-offs: no dots.

Selection: selected row gets `surface-raised` fill + `text-bright` title;
unselected row titles are `text`. Only one bright title per list.

## 5 · Two voices: sans is content, mono is machinery

Sans: titles, descriptions, messages, row titles, kv keys.
Mono: states, ids, hashes, times, paths, handles, stats, block headings,
bar text, actions. Block headings are mono 11 `text-muted`, lowercase
(`run`, `files`, `gates`, `sign-offs`, `in progress`, `details`).
Narration (the narrator's voice) is italic mono 10.5 `text-faint`/`text-ghost`
inline; narrated screen summaries are italic sans 12 `text-secondary`.

## 6 · Screen header

Title block, left-aligned: title (sans 13 `text-heading`) over one italic
narrated summary line (sans 12 `text-secondary`, fixed width 560). `∷`
(mono 14 `text-secondary`, pad [2,8]) top-right. No breadcrumbs anywhere.

Example summary lines: "Seven open across three states — one live run, one
waiting on you." / "Three seats, two engines, one trust list — runtime.toml
as loaded."

## 7 · Key/value rows

Row: space-between, alignItems center. Key sans 11.5 `text-secondary`;
value mono 10.5 `text` — state-colored only when the value IS a state
(`live · frame 5 of 9` in accent; `pass` in ok).

## 8 · The bottom bar

Every screen ends in a bar: border-soft 1, pad [10,14], space-between.
Left: state word (colored per rule 1) + `· detail` (mono 11
`text-secondary`). Right: actions, mono 11, `·`-separated (`text-faint`
bullets); primary action `text-bright`, secondary `text-secondary`,
destructive/tertiary `text-muted`. Buttons are ghost text — no fills,
no borders, no icons.

Examples: `running · frame 5 of 9 · review round 2 | watch raw · pause`;
`synced · .internal/tasks @ 81e5a394 | new task`.

## 9 · Overflow: clip + fade

Scrolling containers set clip. When content overflows, a full-pane-width
gradient overlay sits above the bar: canvas color, alpha 0 at top → opaque
at 80% → 100%, h 100, with `scroll to end` centered near its bottom
(mono 10.5 `text-faint`). No fade when content fits.

## 10 · The rail

w 260. `Spaces` heading (mono 11 `text-muted`) → repo rows (dot 5 +
name sans 13; active = raised fill + bright dot/name) → spacer → footer:

    Oskar Cieslik            (mono 10.5 text-secondary)
    ctx-company/traits · local   (mono 10.5 text-session · faint · faint)

Nothing else in v1. Centered 32×1 `border-strong` dividers may separate
future rail sections and precede the footer.

## 11 · Live activity fade

Under a live row/section: mono 10.5 `text-faint` lines, newest at top
opacity 1.0 fading stepwise to ~0.45, gap 3. Content is the agent's
actual activity (`review: reading modules/io/src/center.rs`).

## 12 · Provenance in chrome

Bars and preview footers name their source of truth:
`.internal/tasks @ <sha>`, `skill-lock @ <sha> · 5 digests`,
`runtime.toml · loaded 13:02`, `run-ab12ef · started 13:44`.
Preview footers are right-aligned mono 10 `text-faint`.

## 13 · Preview column blocks

w 380. Order: identity block (headerless lede or `details`/`run`/`merge`
kv block) → state blocks (`in progress` bordered item with `live` marker
and narrated line) → fact blocks → consequence block (`on approval`,
`landing`: `→`-prefixed sans 12 lines) → spacer → provenance footer.
Bordered "now" items: `row-open` fill, border-soft, pad [8,10], title
sans 12 + state word right, narrated line mono 10.5 `text-muted`.
