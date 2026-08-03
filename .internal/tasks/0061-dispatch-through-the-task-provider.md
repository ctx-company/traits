# 0061 — Dispatch resolves, materialises and digests the task; unmet dependencies refuse

**Status:** ready to implement · **Depends on:** 0060 · **Raised:** 2026-08-03 (owner design session; decisions in this file are the contract — they were settled deliberately, do not re-open them without a concrete contradiction)

Third slice of the task-interface arc, and the one that pays off immediately even alone.

## Decisions

- **Resolution goes through the provider.** `dispatch_preflight::task_file_name_in_board`'s
  filename/stem/numeric-prefix matching is retired — that logic is meaningless for any backend
  that is not a directory.
- **Dispatch materialises the task document into the run and digests it into the ledger**, next
  to the trait digest.
- **A dependency preflight refuses to dispatch a task with an unmet `depends-on`**, in the same
  preflight family as the standing-wall check, with a readable sentence naming the dependency and
  its current status. Enforcement lives at dispatch, not in the dashboard — a check that only
  exists in the UI is bypassed by the CLI.
- **An override exists and is RECORDED.** You will sometimes know a dependency is irrelevant to
  the slice you are running. The run's provenance must then say it started against an unmet
  dependency. An override that leaves no trace is exactly the silent dishonesty this product
  exists to remove.

## Why materialise

Measured 2026-08-03: a worktree is cut from HEAD at dispatch, so a task edited but not yet
committed is invisible to the run. Task 0050 was amended at 07:56, committed at 08:22:01, and the
run whose worktree was cut at 08:21:51 executed the *stale* text and spent ten rounds chasing
scope the amendment had removed. Materialising from the invocation repository's working tree at
dispatch removes that trap, and the digest makes the run's actual contract provable afterwards —
which is what keeps the guarantee alive once a backend is remote and mutable.

## Watch

- Read from the invocation repo's working tree, write into the worktree — that ordering is the
  whole fix.
- The stepless-task guard is NOT here; it needs the CDK Task type and lands with 0062.
- The digest belongs in the ledger beside the trait digest, so "what was this run told to do" is
  answerable from evidence alone.
- Do not silently prefer HEAD when the working tree differs — that is the current behaviour and
  it is the bug.

## Done when

Dispatch resolves the task through the provider; the ledger records the task document's digest;
editing a task file and dispatching WITHOUT committing runs the edited text; a task with an unmet
dependency refuses with a one-sentence explanation naming the dependency and its status; the
override dispatches and is visible in the run's provenance.
