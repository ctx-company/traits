# 0027 — ~10s of blank screen before a run shows anything

**Status:** DONE 2026-07-29 (both pieces) · **Raised:** 2026-07-29 · **Scope
fixed by owner 2026-07-29: paint first, profile later**

**Landed.** Piece 1: `ctx run · initialization` prints on stderr the moment the
Run dispatch is entered — before config resolution, trait load, or worktree
work — on both the driven and `--no-drive` paths (a handler-local line would
have missed every ordinary driven run), silent under `--json`. Piece 2:
measured a real `--worktree` session start at ~24s, then wired the P551
progress observer through `prepare_worktree` (it was being dropped as `None` —
the observer and its messages already existed). The full story now narrates on
one stderr channel:

```
ctx run · initialization
ctx run · creating worktree
ctx run · seeding
ctx run · warming target
ctx run · setup: pnpm install --prefer-offline
ctx run · setup done (1s)
ctx run · setup: just ts-build
ctx run · setup done (4s)
```

Profile result for the record: setup commands ~5s; the remaining ~18s is
`git worktree add` + seed + warm clone on a 60 GB checkout, each phase
boundary named before it starts. MCP hosts and `generate`'s internal eval runs
keep `narrate_progress: false`.

Starting a run leaves the terminal blank for roughly ten seconds before any
output appears. The work in that window is real (config resolution, trait load,
trust check, worktree creation, seed copy, warm clone, setup commands) — the
defect is that none of it is narrated.

## Approach

**Paint the run view immediately, showing `initialization`, and do it without
waiting for any profiling** (owner ruling 2026-07-29). Ten seconds of nothing is
the defect on its own. A view that appears at once and says `initialization` is
already an acceptable answer, whatever the profile later turns out to say —
"slow but accounted for" is a different product from "apparently hung", and only
the second one makes the owner wonder whether to kill it.

So this task is now two independent pieces, and the first does not depend on the
second:

1. **The view appears within ~1s, reading `initialization`.** Ship this alone.
2. **Then** profile, and narrate whatever dominates — one slow step gets step
   boundaries (P551 already added a `progress` observer to worktree preparation,
   so the seam exists), many small ones just fill in behind the panel that is
   now already on screen.

## Watch

- Do not fix this by deferring real work. Painting the panel earlier is not the
  same as reordering what runs behind it: a run that paints instantly and then
  fails on a bad config has moved the delay, not removed it. Piece 1 changes
  when the view appears, never the order of config resolution, trust check, or
  worktree preparation.
- `initialization` must give way to something more specific as soon as there IS
  something specific to say. A status that sits on one word for ten seconds is
  the same defect wearing a label — it buys the run the benefit of the doubt,
  and Piece 2 is what earns it.
- Measure with the warm clone both cold and warm: cloning a 48G `target/` is a
  plausible several-second contributor on its own, and it only happens on the
  first run in a fresh worktree.
- The `--progress stream` path has the same gap and the same fix.

## Done when

**Piece 1:** the run view is on screen within ~1s of invocation showing
`initialization`, with no profiling required to land it. **Piece 2:** every
subsequent multi-second step is named while it runs.
