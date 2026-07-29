# 0016 — `story` reads like a story, not a ledger dump

**Status:** ready to implement · **Raised:** 2026-07-28

## What it prints today

Real output, `ctx traits story run-3d5ef0a1ff…`:

```
  spine: slot-revisions
  enrichment: ledger-only
  beats: 21
    [1] procedure:draft-writing[0] — Draft the work
        reason: all declared outputs accepted
        slot:draft by smart-1@claude-code (Replace)
        preview="# P530 implementation draft — Fold the implement…" bytes=16276
        summary (typed-output): # P530 implementation draft — Fold the implement…
    [2] procedure:building[1]/loop:building-body[0]#iteration:0/item:building-produce[0]#iteration:0 — Building Produce
```

Everything wrong with it is on that page:

- **`procedure:building[1]/loop:building-body[0]#iteration:0/item:building-produce[0]#iteration:0`** — the raw position path. It encodes "Building Produce, round 1" in 88 characters of internal addressing.
- **`spine: slot-revisions` / `enrichment: ledger-only` / `beats: 21`** — implementation vocabulary in the header. A reader does not know what a spine is and does not need to.
- **`reason: all declared outputs accepted`** on every single beat — noise when it is the normal case; worth saying only when it is not.
- **`preview=… bytes=16276`, then `summary (typed-output):`** — the same text twice, once with a byte count.
- **No structure.** Twenty-one beats in one flat list, so the run's shape — a draft, then N rounds, then a commit — is invisible.

## Target

Apply the house design language the live view now uses: sections, bullet separators, one line per event, human words.

**Default level** — the run as a narrative, sectioned:

```
Implement P530 · implement-quick · 1h 00m · completed, merged

JOURNEY
  ✓ Draft the work · smart-1@claude-code · 5m 25s
  ~ Building, round 1
      ✓ produce · worker@claude-code · 12m 04s
      ✓ gates · passed
      ✗ review · smart-1 · revise — 1 blocker
  ~ Building, round 2
      …

BLOCKERS RAISED
  phase-gates-clippy-large-enum-variant · round 1 · cleared round 2
  implement-fold-not-landed · round 2 · never cleared

COMMANDS
  just implement-phase-gates · 4× · passed 3, failed 1
  git diff --stat · 4×

OUTCOME
  committed a2ae6a3 · merged to main
```

The groupings are the point: **journey**, **what the agent actually ran**, **what the reviewer objected to**, **how it ended**. Those are the four questions a person opens a story to answer, and today they must reconstruct all four from a flat beat list.

**Assisted level** keeps the same sections and replaces the per-step lines with narrated prose — same skeleton, richer text, so the two levels are recognisably the same document.

## Watch

- **Round grouping comes from `position-path`'s `iteration`, never a counter** — the same rule as task 0007's history lines, and for the same reason: a resumed run breaks a view-local counter. Nested loops need a stated rule (outermost, innermost, or both).
- **Never invent.** The blocker section is derived from accepted verdict values; "cleared round 2" means the blocker id stopped appearing, which is a fact the ledger supports. Do not let the assisted level infer causation the evidence does not carry.
- **Keep `--json` byte-stable.** This is presentation; every adapter reading `story --json` must be unaffected, and the markdown output should follow the same sectioning rather than becoming a third format.
- **Position paths stay in `--json` and in `--level detailed`.** They are real evidence and the debugging surface needs them — they simply do not belong in the default human read.
- Do not let the header lose the disposition. `completed`, `parked`, `blocked` is the first thing a reader needs, and P550 requires it to lead — a story that opens like a success and ends badly is the failure mode that phase exists to prevent.

## Done when

The default story opens with the run's disposition and reads as sectioned narrative; no raw position path, `spine`, `enrichment` or `beats` vocabulary appears in the human output; per-round grouping is visible; commands and blockers have their own sections; the assisted level shares the skeleton; `--json` is unchanged.
