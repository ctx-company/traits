# 0022 — Re-assert the output contract per turn in persistent sessions

**Status:** blocked on persistent sessions being used again · **Raised:** 2026-07-29

## Why

A persistent (warm) session spawns ONE process and feeds it many prompts. The
output contract delivered on turn 1 drifts out of attention by turn 12, and
nothing restates it.

Precedent: the opencode plugin (P500) re-asserts the resolved trait set on
every call for exactly this reason.

## What to do

In a persistent session, emit the `<output>` envelope on every turn rather than
relying on the spawn-time contract. At ~4.4 KB (~1–1.5k tokens) it is noise
against a frame costing tens of thousands.

## Why this is parked, not urgent

`warm-argv` was removed from this repo's config on 2026-07-29: every seat is
`session-mode = "per-frame"`, so the warm path was dead config that was ALSO
costing `--json-schema` on claude-code seats (the two are mutually exclusive —
a flag fixed at spawn cannot carry a per-frame schema).

This task becomes live the moment a seat goes back to `persistent`.

## Watch

- Re-asserting the contract does not restore `--json-schema`. The exclusivity is
  structural: one process, one spawn, many different per-frame schemas. If
  provider enforcement matters more than session warmth, per-frame is the
  answer, not re-assertion.

## Done when

A persistent session carries the full `<output>` envelope on every turn, and
the exclusivity with `--json-schema` is documented where an author choosing a
session mode will see it.
