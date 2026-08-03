# 0070 — A handoff surface updates in place as the run progresses

**Status:** ready to implement · **Depends on:** 0069 · **Raised:** 2026-08-03 (owner design session; decisions in this file are the contract — they were settled deliberately, do not re-open them without a concrete contradiction)

Third slice of the handoff arc. One message per run, kept current — not a stream of pings.

Progress notification is normally a noise problem. It stops being one when the surface is edited
rather than re-posted, which is why this is in the arc at all and why "progress pings" are not.

## Decisions

- **Update is not a new mechanism.** It is the same pipeline fired again: snapshot `RunState` →
  `render(spec)` → `deliver` with `Repeat::Upsert`. If the log holds a reference for
  `(session, channel)`, the channel edits in place; if not, it creates and stores one. There is no
  second code path and no "update" verb.
- **Cadence is declared per channel, not globally**, in `capabilities()`:
  `Update { Terminal, OnMilestone { debounce_seconds } }`. A file channel updates freely because it
  is free; a Slack card wants a debounce; a PR body wants a longer one. The host debounces — a
  channel never sleeps.
- **Milestones are structural, never per-frame:** run started, round completed, gate result, park,
  complete, landed. Per-frame updates burn rate limits to say nothing, and the temptation to add
  "and also when the worker writes a file" is how this becomes the noise problem it exists to avoid.
- **The delivery log carries a monotonic sequence and refuses out-of-order writes.** A late milestone
  arriving after the terminal update must not repaint a finished run as `running`. This is cheap now
  and unpleasant to retrofit once three channels exist.
- **`Once` channels never participate.** A pager-style alert is `Once` and is emitted at one phase
  only. Only `Upsert` and `Overwrite` channels join the milestone loop.
- **A vanished target is not recreated.** If an update fails because a human deleted the message or
  closed the PR, record the failure and stop. They deleted it deliberately; reposting is both rude
  and unrecallable.
- **The host driving the session fires updates.** Not the dashboard, which may not be running, and
  not the run. A session with no live host simply gets its terminal delivery when one next observes
  it — updates are an enhancement, never the only path to being told.

## Scope

`Update` in `capabilities()`; a milestone hook in the host's drive path; host-side debouncing;
sequence numbers on delivery-log rows with an out-of-order refusal; `Repeat::Upsert` wired to the
stored reference; the file channel re-rendering on milestones as the offline proof of the loop.

## Watch

- **Two caches, two cadences** — the dashboard's session-inventory poll and this milestone hook are
  different clocks. 0063 already flags this for the TASKS screen; do not let the handoff hook quietly
  become a third poller of the same store.
- Re-rendering per milestone re-reads the ledger. That is local and cheap, but the *delivery* is not:
  respect the debounce even when several milestones land together, and coalesce to the latest
  snapshot rather than sending each.
- A run that resumes after a crash must find its reference and continue editing the same surface, not
  open a second one. That is the first thing to test, because it is the failure the whole stored-log
  design exists to prevent.
- Do not let phase-dependent content drift between the mid-run and terminal renders. Same `RunState`,
  same renderer, only the `phase` differs — a special-cased "in progress" template will fall behind.

## Done when

A routed `Upsert` channel shows one surface per run that changes as the run progresses; a resumed or
re-driven session edits the existing surface rather than creating a second; a late milestone cannot
repaint a terminal run; a deleted target records a failure without reposting; debouncing coalesces
bursts to the latest snapshot; and `Once` channels are provably untouched by the milestone loop.
