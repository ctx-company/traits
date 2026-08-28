# Design tokens — extracted verbatim from ctx.traits.pen (2026-08-28)

Dark values are normative for v1. Light values are recorded for later —
do not implement, do not drift them.

## Fonts

| token | value |
|---|---|
| font-sans | IBM Plex Sans |
| font-mono | IBM Plex Mono |

## Colors

| token | dark | light | role |
|---|---|---|---|
| canvas | #0c0d10 | #faf9f7 | window background |
| card | #0e0f12 | #fdfcfb | (reserved) |
| surface | #14161b | #f5f3f1 | file headers, composer-like fills |
| surface-raised | #16181d | #f3f1ef | selected/active row fill |
| row-open | #101215 | #f7f5f3 | open/emphasized block fill (diff blocks, now items) |
| button | #1c1e24 | #fdfcfb | (legacy; buttons are ghost text now) |
| border | #1a1c21 | #f0eeea | faintest line |
| border-soft | #23262d | #e8e5e1 | standard box border (bars, blocks, annotations) |
| border-strong | #2b2e35 | #d3cec7 | emphasized box border, rail dividers |
| text-heading | #f2f4f7 | #262421 | screen titles |
| text-bright | #eceef2 | #2f2d2a | selected titles, own messages, primary actions |
| text | #d7dae0 | #454340 | body content |
| text-session | #a7abb5 | #5b5854 | the owner's handle / current space |
| text-secondary | #8b909c | #6a6762 | descriptions, secondary actions |
| text-ghost | #7f8590 | #96928c | narration text, receded content |
| text-fact | #6b7078 | #8c8881 | reference lines (commits, citations) |
| text-muted | #585d68 | #a5a099 | labels, block headings, timestamps |
| text-faint | #4d525c | #bcb8b0 | activity lines, hints, separators |
| text-version | #3f434b | #d4d0c9 | dimmest — line numbers |
| dot-idle | #33363d | #ddd9d3 | pending/ready state dot |
| dot-dim | #4a4f5a | #c0bbb3 | draft state dot |
| crumb | #2c2f36 | #d9d5cf | (legacy) |
| spine | #23262d | #d8d4ce | (legacy) |
| accent | #7fb2ff | #4479db | LIVE now; architect identity |
| accent-bright | #a8ccff | #2f66cc | (rare emphasis) |
| accent-dim | #5778ac | #a9c4f0 | accent, ambient/inactive |
| ok | #7fc79a | #3f9a68 | done · pass · trusted · landed |
| warn | #d8b46a | #ab7f2a | needs the owner; open decisions |
| warn-dim | #917a4a | #ddc79e | warn, ambient/inactive |
| danger | #e08b7f | #c96552 | failure; deletions in diffs |
| review | #c49af0 | #7a52cc | review/security agent identity |
| review-dim | #8469a2 | #c3b2e8 | review, ambient/inactive |

Diff row washes (dark, literal — not variables): additions `#7fc79a14`,
deletions `#e08b7f14`. Selection wash: `#7fb2ff12`.

## Type scale (px, IBM Plex unless noted)

| size | usage |
|---|---|
| 16 | document titles (rare; in-doc H1) |
| 14 | ∷ context-action glyph |
| 13 | screen titles (heading tone), body/message text (lh 1.5–1.6) |
| 12.5 | list-row titles, chat/annotation comment text |
| 12 | narrated descriptions (italic), row descriptions gain 11 — see below |
| 11.5 | kv keys, sub-descriptions, check/landing lines |
| 11 | block headings (mono, lowercase, muted), bar text, code lines, mono actions |
| 10.5 | mono meta: states, handles, times, values, activity lines, hints |
| 10 | smallest: row timestamps, diff stats, footers, sign-off right side |

Row descriptions inside list rows: sans 11 secondary.
Weights: 500 only for emphasized in-block titles; everything else normal.
Italic marks narration/narrated-summary only.

## Layout constants

| constant | value |
|---|---|
| window | 1440×900 (core screens), cornerRadius 10, clip |
| title bar | h 36, padding [0,20], traffic lights 12px, logo 34×18, menu gap 18, menu 12.5 sans |
| rail | w 260, padding [18,0]; heading pad [0,18,12,18]; repo rows pad [0,10] gap 3, row pad [8,10] |
| rail footer | pad [14,18,0,18], gap 6 |
| main pane | fill, padding [18,40,20,40], gap 28 |
| preview column | w 380, padding [18,22] |
| list rows | pad [6–8, 10–12]; rows gap 2–4; section gap 12; dot 5px, dot-text gap 10–12 |
| blocks | gap 8, bottom padding 20; bordered boxes pad [8,10–12] |
| bottom bar | pad [10,14], border-soft 1 |
| bottom fade | h 100, full pane width, stops: canvas-alpha 0 → opaque at 0.8 → 1 |
| rail divider | 32×1 border-strong, centered, pad [12–14, 16–18] |
