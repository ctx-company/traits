# 0083 — A loop may declare no iteration bound

**Status:** ready to implement · **Depends on:** nothing · **Raised:** 2026-08-04 (owner design session; decisions in this file are the contract — they were settled deliberately, do not re-open them without a concrete contradiction)

First of three component slices the plannotator trait needs. Land it first: it is the smallest
and the only one that blocks that trait from building at all.

Some loops genuinely have no iteration count. A build loop that ends when the reviewer *and* the
owner both approve ends when the work is right, not on the sixth try — and a bound picked to
approximate "never" is a lie in the source that a reader has to decode. Today the only way to say
"no bound" is a large number.

## Decisions

- **Fix it where it is enforced, not by adding a concept.** Static validation already permits
  absence — `validate.rs:1262` reads "loop sequence item must declare sequence, optional
  max-iterations/until/stop-if/on-exhausted/on-stop". The runtime is what refuses, in the `else`
  branch at `control_flow.rs:814-819` ("loop step is unbounded and will not run"). That branch plus
  the required CDK type (`sequence.d.ts:214`) is the whole change.
- **Absent means bounded by the run, not by a new knob.** `[run] max-frames` and `[run]
  total-seconds` are the real ceilings, they already exist, and they already stop a runaway loop.
  Adding a second "default iterations" setting would put the bound in two places and make neither
  authoritative.
- **`onExhausted` without a bound is meaningless authoring and is refused.** Exhaustion cannot
  occur, so declaring a policy for it reads as meaning something it does not. This is the same
  reasoning that already rejects `emits` on a loop (`validate.rs:1255-1258`) rather than silently
  ignoring it.
- **A loop with neither a bound nor an exit guard is refused at validation.** Unbounded is a
  deliberate pairing with `until`/`stop-if`, never an omission. A loop that can neither exhaust nor
  exit is a run that dies on its run budget having proven nothing, which is the worst outcome
  available and must fail at build time.
- **`max-iterations-from` is untouched.** A dynamic bound resolved from a port is still a bound;
  none of this changes how it resolves or what happens when its input is missing.

## Scope

`iterations` becomes optional in the CDK's loop fields; `control_flow.rs`'s `else` branch stops
erroring and encodes "no bound"; the loop-advance path treats that encoding as "never exhausts";
validation gains the two refusals above. The generated schema already carries `max-iterations` as
optional, so no trait that declares one moves its digest — this is additive.

## Watch

- **`frame.max_iterations: None` currently means "not a loop"** (`control_flow.rs:769` sets it for
  every non-loop frame). Reusing `None` for "unbounded loop" overloads a value that already has a
  meaning, and the collision is invisible until a loop and a non-loop frame are compared. Pick an
  explicit encoding rather than making `None` mean two things.
- The exhaustion path and the guard-exit path are different code today. Prove an unbounded loop
  takes the guard path and never touches the exhaustion path, rather than assuming a missing bound
  makes exhaustion unreachable.
- A loop that runs until the run budget expires must still park honestly with whatever it produced.
  That is existing behaviour for a run that runs out of time, but it is now reachable by design
  rather than by accident, so it is worth proving once.

## Done when

A loop declaring `until` and no `iterations` builds, runs, and stops on its guard; `on-exhausted`
declared without a bound is refused by name; a loop with neither a bound nor an exit guard is
refused by name; `max-iterations-from` still resolves as it does today; and no existing trait's
digest moves.
