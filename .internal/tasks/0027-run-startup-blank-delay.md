# 0027 — ~10s of blank screen before a run shows anything

**Status:** ready to implement (needs profiling first) · **Raised:** 2026-07-29

Starting a run leaves the terminal blank for roughly ten seconds before any
output appears. The work in that window is real (config resolution, trait load,
trust check, worktree creation, seed copy, warm clone, setup commands) — the
defect is that none of it is narrated.

## Approach

Profile first, then decide. Two different fixes depending on what dominates:

- If it is dominated by ONE slow step (worktree creation, `pnpm install`, the
  warm clone), narrate step boundaries — P551 already added a `progress`
  observer to worktree preparation for exactly this, so the seam exists.
- If it is spread across many small steps, the panel simply needs to appear
  BEFORE them and fill in, rather than after.

## Watch

- Do not fix this by deferring real work. A run that paints instantly and then
  fails on a bad config has moved the delay, not removed it.
- Measure with the warm clone both cold and warm: cloning a 48G `target/` is a
  plausible several-second contributor on its own, and it only happens on the
  first run in a fresh worktree.
- The `--progress stream` path has the same gap and the same fix.

## Done when

Something appears within ~1s of invocation, and every subsequent multi-second
step is named while it runs.
