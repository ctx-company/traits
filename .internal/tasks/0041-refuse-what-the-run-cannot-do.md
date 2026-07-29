# 0041 — Refuse what the run cannot do, instead of blaming the worker for it

**Status:** item 1 LANDED 2026-07-29 · items 2–3 below · **Raised:** 2026-07-29

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

### 1. Preflight commands at dispatch — LANDED

Implemented in `modules/io/src/dispatch_preflight.rs`
(`unrunnable_check_commands`, `unrunnable_refusal_message`), called from
`modules/io/src/run.rs` immediately after the standing-wall preflight. Verified
in a scratch repository with no `test` recipe:

```
invalid manifest at run.trait: sequence building-body declares `just test`,
which cannot run here: the Justfile has no recipe `test`. add a `test:` recipe
to this repository's Justfile so the step has something to run
```

and verified to pass once the recipe exists. Full suite green (739 passed).

The original specification follows, as the record of what was built:

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

### 2. Automatic within-run repeat detection — RESCOPED, and here is why

The obvious version of this is not achievable as first written, and the reason
is worth stating before someone tries it.

"A blocker id repeating three rounds should park" requires whatever enforces it
to understand `blockers[].id`. **The runtime has no such vocabulary and must
not gain one.** It advances loops over slots and guards; `blockers`, `status`,
and `advisory` are one trait's schema, invented in `implement`'s own TypeScript.
Teaching `guards.rs` to parse them would make the generic loop engine depend on
one trait's output shape — the same category error as hard-coding `just test`
into a package meant to be installed elsewhere, which is what this whole task
exists to correct.

Two achievable shapes, and the choice is a real one:

**(a) The trait declares its own repeat identity.** One optional field on the
loop: which slot carries findings, and the path within it that identifies one
(`blockers[].id`). The runtime compares that projection across iterations and
parks after N. Keeps the vocabulary in the trait, where it belongs; costs a
small typed addition to the loop declaration and the canonical model.

**(b) Byte-identical stall detection.** Fully generic — park when a designated
slot's digest is unchanged for N iterations, no schema knowledge at all. Cheap,
and it would have caught the ctx-gate gate slot, whose digest was byte-identical
for six rounds. It would NOT have caught the repeats seen in
`run-56d1b1346536`, where the same blocker id came back with reworded prose
each round, so the digest moved every time.

**Correction after reading the loop code (2026-07-29): (b) has no useful
vocabulary-free form, so (a) is the only real option.**

"Park when a designated slot's digest is unchanged" still has to be told WHICH
slot — which is a declaration, which is (a) with a smaller field. The only
genuinely declaration-free version is "park when *every* slot written this
iteration is byte-identical to the last two", and that is too strict to fire:
in `run-56d1b1346536` the work summary and verdict changed every round while
the blocker did not, so the full set never repeated. It would not have caught
the case it exists for.

So the cost is honest and unavoidable: **a new field on the loop declaration**,
which means the canonical model, a schema version, and its migration (0029's
gate). Worth doing, but it is a model change, not a cheap addition — do not
start it expecting the latter.

The hook point is `modules/core/src/procedure/runtime/control_flow.rs:1735`,
where `next_iteration >= max_iterations` decides exhaustion; a stall check sits
immediately before it and stops with its own reason rather than
`STOP_MAX_ITERATIONS_EXHAUSTED`, so a parked-for-stall run is distinguishable
from one that merely ran out.

Threshold either way: three consecutive rounds. Two is normal — a fix that
misses once is ordinary. Three is the loop telling you the reviewer and the
worker disagree about what the fix even is.

### 3. Phase validation must be runnable where the phase runs — APPLIED

An authoring rule, not code: a phase's declared Validation sequence must be
executable in the environment the phase actually runs in. `demo-bootstrap` in a
worktree fails this, and no amount of worker effort fixes it.

Applied to both ctx-gate phases on 2026-07-29: `just demo-bootstrap` removed
from both Validation blocks, DEMO-CRITICAL-4's scenario required to be
self-contained from the canonical fixture, and DEMO-CRITICAL-5's real
Slack/browser/tunnel proof moved into an explicit *Owner acceptance* section
marked not-a-blocker. Both phases also state that the repository gate is
`just test`, that it runs no test target, and that a gate result is not evidence
of prohibited work — the misreading that cost six rounds.

Nothing enforces this rule mechanically, and item 1's preflight does not reach
it: a phase's Validation list is prose in a plan document, not a declared
command. That gap is the reason this class recurs, and closing it means giving
phases typed validation — a much larger change, deliberately not chartered here.

**Do not resolve the `.env` case by seeding it.** ctx-gate's own Justfile unsets
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

1. **Done.** A trait declaring a command this repository cannot execute is
   refused at dispatch with a message naming the remedy.
2. A loop that stops converging parks with a diagnosis instead of exhausting its
   budget — by (b) at minimum, by (a) if the typed declaration is chartered.
3. **Applied to ctx-gate.** No phase ships a Validation sequence its own run
   environment cannot execute. Mechanically unenforced; see the note above.
