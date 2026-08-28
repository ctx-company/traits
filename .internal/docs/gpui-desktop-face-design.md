# The gpui desktop face — visual design charter

Companion to `gpui-desktop-face-plan.md`. Same gate (0243.4 + 0243.5), same
rulings. Source of visual truth: the owner-held pencil file (`ctx.traits.pen`,
2026-08-28 state). Workers do not access pencil; they build from the derived
artifacts this charter defines. Dark theme only for v1 (light values are
recorded in the tokens file for later, not implemented).

## Scope

v1 screens — what the dashboard TUI serves today:

- **Sessions** (Run — Live): frame list, live activity, run bar, preview column
- **Tasks** (Board): state sections, task rows, board bar, task preview
- **Traits** (Library): authored rows, trust states, lockfile provenance bar
- **Config** (Runtime): seats / runtime / trust rows, runtime.toml bar
- **Merges** (Queue): landing / awaiting / landed sections, gates, sign-offs

Designed but parked (not v1): chat family, plan review, run brief/diff,
light theme, mobile. Their designs stay in the .pen untouched.

## Work item 0 — design artifact extraction (pencil-side, owner+assistant)

Produce, commit, and keep current under `.internal/docs/design/`:

- `tokens.md` — every color/font/size/spacing constant, verbatim from the
  .pen variables (dark values normative for v1; light recorded for later)
- `grammar.md` — the normative rules, expanded with examples
- `screens/<screen>.md` — component tree per v1 screen: exact tokens, copy,
  and a `data | placeholder` tag on every element
- `reference/<screen>.png` and `reference/<screen>.html` — pencil exports;
  the HTML is worker-readable ground truth (pixel values and colors as text)

Acceptance: a worker implements any v1 screen from these files alone.
Only this item touches pencil; every later item is ordinary Rust work.

## The grammar (normative, condensed — grammar.md expands each)

1. Color is semantic: accent = live right now; warn = needs the owner;
   ok = done/pass; danger = failure/deletion; review-purple = review-agent
   identity. Neutrals carry everything else.
2. States are words, not icons: `live · round 2 · smart`, `ready`, `draft`,
   `landed`, `pass`, `pinned`, `synced`, `awaiting approval`.
3. Glyphs are structural only: `↰` back, `↳` reply/origin, `→` consequence,
   `·` separator, `∷` context action. No other iconography. No corner radii
   inside the canvas — radius exists only on window chrome.
4. Dots (5px) mark status only in stateful lists (frames, tasks, repos,
   merges). Selected row = surface-raised fill + bright title; unselected
   titles are normal text. Dots never appear in prose.
5. Sans is content; mono is machinery (states, ids, times, paths, labels).
   Block headings: mono 11 muted, lowercase. Narrated summaries: italic
   sans 12 secondary.
6. Every screen header: title (sans 13, heading tone) + one italic narrated
   summary line + `∷` on the right. No breadcrumbs.
7. Key/value rows: key sans 11.5 secondary left; value mono 10.5 right,
   state-colored only when semantic.
8. Bottom bar per screen: left = state word (colored) + `· detail`
   (secondary); right = mono 11 actions, `·`-separated, primary bright.
9. Overflow: container clips; full-pane bottom fade (canvas alpha 0 → 1 at
   80%) with centered `scroll to end` (mono 10.5 faint). Fade only when
   content actually overflows.
10. Rail: `Spaces` heading (mono 11 muted) + repo rows + spacer + footer
    `Oskar Cieslik` / `ctx-company/traits · local` (mono 10.5,
    secondary / session + faint).
11. Live activity: mono 10.5 faint lines fading 1.0 → 0.45 opacity.
12. Chrome states provenance: bars and footers cite their source —
    `.internal/tasks @ <sha>`, `skill-lock @ <sha>`, `runtime.toml`.

## Placeholder doctrine

Anything the center/store cannot serve today renders as design-faithful
static content from one `placeholders` module — never inline literals —
so wiring later is a grep, not an archaeology dig. Known v1 placeholders:
loop-narration lines, "in progress" narration text, merge-queue contents
(until a merge surface exists), sign-off entries.

## Relationship to the plan's work items

Item 0 (extraction) precedes and feeds item 1 (walking skeleton: the
skeleton's window/rail/list ARE tokens + grammar rules 3/4/10). Items 2–3
render Sessions per `screens/sessions.md`. Tasks/Traits/Config/Merges
screens join as new work items after item 3, each one screen, read-mostly,
placeholder-marked. Decomposition follows screens, never stack layers
(plan doctrine holds).
