# 0041 — Refuse what the run cannot do, instead of blaming the worker for it

**Status:** ready to implement · **Raised:** 2026-07-29

## The failure class

A run is asked to do something its environment cannot do. Nothing detects it.
The reviewer sees a false verdict, cannot tell *"couldn't run"* from *"ran and
failed"*, and writes it up as a blocker against the work. The worker cannot
clear it — a command step's argv lives in the **trait**, not in the run — so it
repeats every round until the budget is gone, and under
`onExhausted: "continue"` the run commits anyway.

Three instances on 2026-07-29 alone:

| what was asked | why it could not happen |
|---|---|
| `just test` (trait's check step) | ctx-gate had no `test` recipe |
| `just demo-bootstrap` (phase validation) | reads the gitignored `.env`, which the worktree does not have |
| write `$HOME/.local/state` | ctx-gate's own sandbox denies it |

The first cost six rounds with a **byte-identical** `repo-gates-passed` digest,
and by round 5 the reviewer had invented a cause:

> "The supplied repository gate ran the explicitly prohibited `just test`
> command and reports ok=false."

It blamed the worker for a command the worker never chose and could not change.

## Why it recurs

**The gate is typed and machine-checked. Everything else the reviewer enforces
is prose.** A phase's Validation list and Done-when are markdown, so they can
demand the impossible and nothing notices; the reviewer then enforces the
impossible faithfully.

Measured across this machine's 55 ctx-gate sessions:

- **45%** (25) exhausted the full 10-round budget
- **38%** (21) carried a blocker that repeated three or more rounds
- **55** distinct blockers repeated 3+ rounds
- **`wall-id` was populated zero times** — the mechanism built for exactly this
  is opt-in through an explicit `**Wall:** <id>` label in plan prose that nobody
  writes

## What to build

### 1. Preflight commands at dispatch

`modules/io/src/dispatch_preflight.rs` already exists and already refuses runs
before a session, worktree, or first frame (P414 standing walls). Its call site
is `modules/io/src/run.rs:413-424`; this goes directly after, on the same
`invalid_request` path.

Walk `trait_ref.sequences`, take each item's plan through
`r#trait::procedure::command::plan_for_item`, and prove the argv can run.
**Conservative — refuse only what can be proven:**

- `argv-from` supplies argv at runtime → skip.
- A **path-shaped** program (contains a separator) → skip; it may legitimately
  be built by a setup command or an earlier step and not exist yet.
- A **bare** program name must resolve on `PATH`.
- `just` additionally checks the named recipe (`just --show <recipe>` in the
  repo root, any failure-to-ask answering "fine"), because a launcher that
  exists while its subcommand does not is the exact shape that cost six rounds.

Other launchers (`npm run`, `make`) get no probe rather than a guessed one; add
them the same way if they ever host a check step.

The refusal must name the sequence, the argv, the reason, and the remedy —
"add a `test:` recipe to this repository's Justfile" is the whole point.

### 2. Automatic within-run repeat detection

A blocker id repeating unchanged across rounds means the last fix did not move
it. That is a wall, and it should park with a diagnosis rather than spend the
remaining budget. The storage and the concept already exist (`wall-id`, park
reports, `dispatch_preflight::find_standing_wall`); what is missing is that it
**fires by itself** instead of waiting for a prose label.

Threshold: three consecutive rounds carrying the same blocker id. Two is normal
— a fix that misses once is ordinary. Three is the loop telling you the
reviewer and the worker disagree about what the fix even is.

### 3. Phase validation must be runnable where the phase runs

An authoring rule, not code: a phase's declared Validation sequence must be
executable in the environment the phase actually runs in. `demo-bootstrap` in a
worktree fails this, and no amount of worker effort fixes it.

**Do not resolve that case by seeding `.env`.** ctx-gate's own Justfile unsets
six secrets by name after sourcing it, including the Slack bot token and the
ngrok authtoken; handing those to an agent breaks the same rule as "credentials
must never reach argv" (0003).

## Watch

- **A preflight that refuses too eagerly is worse than none.** It would block
  legitimate runs at dispatch with no way to override, which is a harder
  failure than the one being fixed. Every rule above is written to fail open;
  keep it that way, and prefer missing a bad command to refusing a good one.
- **This overlaps 0038 part 1 and both should land.** Carrying `exit-code` and
  a stderr tail in the check output is what stops the reviewer *inventing* a
  cause; the preflight is what stops the round being spent at all. Neither
  subsumes the other — a command can exist at dispatch and still fail for
  reasons only its output explains.
- Item 2 changes loop-exit semantics, so it interacts with the earned-exit work
  in `.plans/` (`carry`/`minRounds`). Read that first: both add terms to the
  same decision, and landing them blind to each other is how the exit predicate
  becomes something nobody can reason about.
- The 45%/38% figures come from ctx-gate ledgers on one machine. They are
  enough to justify the work and not enough to quote publicly.

## Done when

A trait declaring a command this repository cannot execute is refused at
dispatch with a message naming the remedy; a blocker repeating three rounds
parks with a diagnosis instead of exhausting the budget; and no phase ships a
Validation sequence its own run environment cannot execute.
