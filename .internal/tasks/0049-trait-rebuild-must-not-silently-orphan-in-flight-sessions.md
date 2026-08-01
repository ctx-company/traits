# 0049 — A trait rebuild must not silently orphan in-flight sessions

**Status:** ready to implement · **Raised:** 2026-08-01 (owner hit it twice in one night; every
resume of a live session failed with an opaque manifest error)

## What happens today

`ctx traits drive --session <id>` reloads the trait from the FILE PATH recorded in the ledger —
not a snapshot of the bytes the session actually started under. Rebuild the trait while a
session is in flight (a normal thing to do: fix a step, change a mode, disable a gate) and the
next resume validates a ledger written under shape A against trait shape B. The ledger-contract
check then rejects it:

```
invalid manifest at run-session.ledger: run ledger contract invalid:
output_ports row port:park-report contradicts recomputed semantic output evidence
```

Measured 2026-08-01: sessions started under canonical `98b08bb7…` (blocking feasibility gate)
became permanently unresumable the moment the trait was rebuilt in warn mode — one ctx-trait
run and one ctx-gate run, both mid-loop with hours of work in their worktrees. The recorded
`canonical-digest` on each ledger already proves the mismatch; nothing consults it before
attempting the load.

Two independent defects:

1. **No shape pinning on resume.** A session records `canonical-digest`, but resume does not
   compare it against the trait it is about to load, so a changed trait is discovered only as a
   downstream contract contradiction.
2. **The failure is unreadable.** The message names an internal invariant (`output_ports row
   port:park-report contradicts recomputed semantic output evidence`) rather than the actual
   cause — the trait changed under this session — and offers no next step. An owner reading it
   cannot tell whether their ledger is corrupt, their trait is broken, or (the truth) neither.

## What it should do

- **Compare first, fail clearly.** On resume, compare the ledger's `canonical-digest` with the
  digest of the trait about to be loaded. On mismatch, refuse with a plain sentence naming both
  digests, when the session started, and the fact that the trait was rebuilt since — before any
  contract validation runs, so the contract error can never be the first thing an owner sees.
- **Offer the honest options in the message**: resume against the original bytes if they can be
  recovered (`--file <path>` already exists as an override), or start a fresh run and harvest
  the worktree. Do not silently accept a changed trait: replaying a ledger against a different
  procedure shape is exactly the unsound thing the contract check is there to prevent.
- **Consider pinning by default**: record enough to reload the exact shape (the canonical
  document is content-addressed and already digested — the cache may already hold it), so a
  resume can be offered against the original trait rather than only refused. Decide explicitly
  whether that is worth the storage; refusing clearly is the floor, pinning is the upgrade.

## Watch

- The contract check itself is correct and must not be relaxed — this task changes WHEN and HOW
  the mismatch is reported, never whether a mismatched replay is allowed.
- The same class covers `ctx traits merge` and any other verb that re-reads a ledger.
- Related (0047's spirit): every failure an owner can hit should name its cause and one next
  action. A ledger error that requires reading Rust internals to interpret is the same silent
  failure class, one layer down.
- Cheap partial mitigation worth landing with it: `run-status`/dashboard rows showing "trait
  rebuilt since this session started" so an orphaned session is visible before someone tries to
  drive it.

## Done when

Resuming a session whose trait was rebuilt fails with a one-sentence explanation naming the
digest change and the available options, never with a contract-invariant message; a session
whose trait is unchanged resumes exactly as before; and an orphaned session is identifiable
from `run-status` without attempting a drive.
