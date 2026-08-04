# 0084 — A command step declares its own silence budget

**Status:** ready to implement · **Depends on:** nothing · **Raised:** 2026-08-04 (owner design session; decisions in this file are the contract — they were settled deliberately, do not re-open them without a concrete contradiction)

Second of three component slices the plannotator trait (0086) needs. Land it after 0083.

0058 was right that silence means stuck — for a gate. A build still printing is working however long
it takes; one that has gone quiet is hung. That reasoning is exactly inverted for a step whose job
is to wait on a person: an interactive review tool prints its startup lines, opens a window, and
then says nothing at all until the human decides. Under the current repo bound it is killed after
ten minutes of someone thinking, mid-review, and the failure looks like the tool crashed.

There is no way to say so. `run.rs:2889` reads `idle_timeout_ms` from config only, with no trait
fallback, so a repo-wide gate setting silently governs a step that has nothing to do with gates.

## Decisions

- **`idle-timeout-ms` becomes authorable on a command step, and the step wins over config.** A step
  that declares "I am waiting on a human" must not be overridden by a number tuned for build
  hang-detection. This is the opposite precedence from the current `timeout_ms`, deliberately —
  see the next decision.
- **Fix the sibling defect in the same change.** `timeout_ms` takes the authored value only as a
  *fallback* (`run.rs:2888`, `.or(command.timeout_ms)`), so config wins whenever `[run]
  command-seconds` is set — which it is here, making `refactor`'s authored
  `timeoutMs: 3_600_000` inert today. Both bounds resolve step-over-config after this task.
  Leaving the two with opposite precedence is worse than either choice, because no reader can
  predict which number applies without reading the resolver.
- **This adds an exception; it does not weaken 0058.** A step that declares nothing keeps config's
  bound and today's semantics exactly. The gate stays governed by silence, because for the gate
  that is still the right test.
- **The wall bound stays the backstop.** `command-seconds` still applies, so "waits on a human"
  cannot silently become "waits forever". A step that wants a long human wait declares a long idle
  budget and inherits the wall ceiling unless it declares that too.
- **`doctor` reports the effective per-step bounds.** Two knobs with a precedence rule are
  unreadable from the source alone, and the failure mode — a step killed by a bound the author
  thought they had overridden — is the one this task exists to end.

## Scope

`idleTimeoutMs` on the CDK's command-step fields; the field on the core command item; the frame
builder carrying it; `run.rs`'s resolution flipped to step-over-config for both bounds; `doctor`
surfacing the effective values.

## Watch

- **Flipping `timeout_ms` precedence is a behaviour change for existing traits, not an addition.**
  Find every authored `timeoutMs` in the fleet before flipping — `refactor`'s is currently inert
  and will start taking effect. That is the point, but it must be a decision rather than a
  surprise.
- The idle bound and the wall bound now come from different places by default and can disagree
  (a step declaring a four-hour idle budget under a shorter configured wall is killed by the wall
  and the message must name which bound fired). 0058 already made timeouts name their kind; keep
  that.
- Do not let this become a general per-step budget system. Two bounds, both already existing, both
  already enforced in one place — nothing else moves into the step.

## Done when

A command step declaring `idle-timeout-ms` survives silence well past `[run] command-idle-seconds`
and is killed by its own bound when it does hang; config no longer silently overrides an authored
`timeout-ms`; a killed step names which of the two bounds fired; `doctor` reports the effective
per-step bounds; and a step declaring neither behaves exactly as it does today.
