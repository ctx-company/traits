# 0078 — A race the machine can rerun is retried, not parked

**Status:** ready to implement · **Depends on:** nothing · **Raised:** 2026-08-04 (owner call: landings must survive a repository that moves under them)

Merging parks when `main` moves between capturing the rebase revision and the
fast-forward. The park text is `merge.rs:1866`:

```
{default_branch} advanced from {main_rev} to {current_main_rev}
since the rebase revision was captured; retry the merge
```

The machine has diagnosed the condition, named the remedy, and stopped to ask
a human to type it. That is the whole defect. With several runs landing in the
same hour the window is hit routinely, and every hit costs an owner round trip
for an operation that would succeed unchanged on a second attempt.

## Decisions

- **Split park reasons into two kinds and treat them differently.** A
  **verdict** is a statement about the work: the gate went red, a conflict
  needs judgment, the deep merger refused a landed-fix regression, disk is
  below the floor. A **race** is a statement about timing: the repository
  moved while we were working. Verdicts park, exactly as today. Races retry.
- **The races, by name:** `MAIN_ADVANCED_INFIX` (main moved since capture),
  `FAST_FORWARD_FAILED` (lost the fast-forward), `MAIN_NOT_CLEAN_INFIX` (the
  invocation checkout was mid-write), and the transient git control failures
  around them — a contended `index.lock`, `CHECKOUT_BRANCH_PROBE_FAILED`,
  `CHECKOUT_CLEANLINESS_PROBE_FAILED`. Each is resolved by doing the same
  thing again against current state.
- **These never retry:** `GATE_FAILED_PREFIX`, `GATE_LEFT_DIRTY`,
  `GATE_DISK_FLOOR_PREFIX`, `HARVEST_*`, `DEEP_REFUSE_REGRESSION_PREFIX`,
  `BRANCH_MISMATCH_PREFIX`. Retrying a verdict is how a red gate becomes a
  landed regression.
- **Shrink the window before widening the retry.** A retry re-rebases and
  re-runs the whole landing gate — minutes, not milliseconds. Capturing the
  revision and fast-forwarding under one hold of the merge lock removes most
  races outright; retry is the fallback for what is left, not the primary
  mechanism.
- **A retry always re-runs the gate.** The base changed, so the previous green
  is a statement about a tree that no longer exists. Carrying it forward is the
  one shortcut that would turn this feature into a way to land unproven work.
- **Bounded attempts with backoff and jitter.** Continuous landings must
  converge, not livelock. When the bound is exhausted, park with the attempt
  count and each attempt's observed revision — a park after five real attempts
  is a different fact from a park on first contact, and the report must say so.
- **`--no-wait` still fails fast.** It already means "do not queue behind a
  concurrent merge", and it must keep meaning "do not wait" rather than
  quietly acquiring a retry budget.
- **Never retry into model spend.** A race that would re-enter the standing
  merger agent's confirmation path retries the mechanical rebase only; if
  reconciliation genuinely needs the merger again, that is a park.
- **Reuse `RetryWarnings`.** Worktree preparation already retries transient
  failures and carries the record forward. This is the same idea one layer up,
  and the evidence should read the same way.

## Scope

A classification of park reasons into race vs verdict, in one place, with the
retry loop around the rebase→gate→fast-forward sequence in
`modules/cli/src/app/merge.rs`; attempt bounds and backoff in `[merge]` config
beside `wait` and `gate-seconds`; per-attempt evidence in the merge frames.

## Watch

- **Convergence is the risk, not correctness.** N runs racing must all land
  eventually. Jitter the backoff, and consider that the merge lock already
  serializes the critical section — a race means someone landed between our
  release and our fast-forward, so a retry queues behind the lock again.
- Do not fold the already-landed case into this. A merge re-run against a run
  whose work is already on `main` currently reports
  `worktree registration verification failed: … is not a registered git
  worktree`, which reads as a failure; the ledger holds `landed=<sha>` and
  could simply say so. Same family of complaint, different fix — worth its own
  task rather than a retry that finds nothing to do.
- The gate is the expensive part of every attempt. Anything that makes a retry
  cheap by skipping it is out of scope by construction.
- A park that follows exhausted retries must not read like a first-attempt
  park; `merge_story`'s translation layer needs the attempt count or the story
  will understate what happened.

## Done when

A merge that loses a fast-forward race re-rebases and lands without an owner
touching it; a red gate still parks on the first failure and is never retried;
retries are bounded, jittered, and recorded per attempt in the merge frames; a
park after exhausted attempts names the count and the revisions observed; and
concurrent runs landing continuously all converge rather than starving each
other.
