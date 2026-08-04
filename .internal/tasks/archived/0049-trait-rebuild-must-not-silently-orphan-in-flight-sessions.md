# 0049 — A trait rebuild must not silently orphan in-flight sessions

**Status:** ready to implement · **Raised:** 2026-08-01 (owner hit it twice in one night; every
resume of a live session failed with an opaque manifest error)

## Correction, 2026-08-01 (verified before implementing — read this first)

The original premise below was WRONG in one important way, and the task must not be built on it.

**A rebuild is already caught, clearly.** Resuming a session whose trait SOURCE changed fails with

```
invalid manifest at drive.source-digest: stale run-session source:
loaded source digest does not match session ledger
```

which is exactly the readable guard this task asked for — it already exists. Verified by
zero-frame drives (`ctx traits drive --session <id> --max-frames 0`) against four ctx-gate
sessions on 2026-08-01. A session whose source digest still matches resumes fine EVEN WHEN its
canonical digest differs (canonical moves when the kit rebuilds from unchanged source).

**The `output_ports row port:park-report contradicts recomputed semantic output evidence`
failure is therefore a DIFFERENT, still-unreproduced defect.** Ruled out on 2026-08-01, each by
inspecting every ledger in both repos' stores:

- not the source/canonical digest mismatch (that has its own guard, above);
- not first-vs-last accepted value in `accepted_slot_values` (no ledger holds more than one
  entry for `slot:park-report`);
- not the empty-array omission rule (every `[]` park-report row is internally consistent:
  status `accepted`, row digest == slot digest);
- not the merge path (no merge frame records it);
- not reproducible from any persisted ledger — every stored ledger in both repos passes.

That leaves a transient in-memory state during a drive: most likely the deterministic `project`
steps of `deriveParkReportStep` (clear to `[]`, then append) updating `slot:park-report` without
refreshing the `output_ports` row in the same transition, so the next contract validation sees a
row that disagrees with the slot it is derived from. THAT is the hypothesis to test first —
reproduce by manually driving a quick session (`--no-drive` + `ctx traits call`) to a revise
verdict so the park-report projection runs, then validate the ledger.

Scope note: the pinning work below is still worth doing (a rebuild still ENDS a run rather than
letting it continue), but it is an enhancement, not the fix for this error.

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
- **Pin the shape so the run survives the rebuild** (owner ask 2026-08-01 — required, not
  optional): a session must be resumable against the exact trait bytes it started under, so
  rebuilding a trait mid-run never destroys the run. Refusing clearly is only the floor.

  What already exists, verified 2026-08-01 — do not rebuild these:

  - **Approval history is already permanent.** `~/.config/ctx/trust.toml` is keyed by
    CANONICAL DIGEST, one entry per approved byte-set (445 entries live), and nothing prunes
    it. A digest approved months ago is still approved today, so an older shape is already
    trusted for replay — no "historical approval" mechanism needs inventing.
  - **Different repos already run different versions of the same trait.** Trust is keyed by
    digest, not by trait id or repo, and every repo carries its own package copy with its own
    digest. ctx-gate ran `implement@98b08bb7` while ctx-trait moved past it, both approved,
    on 2026-08-01. This is a capability the product HAS.

  What is missing is only the bytes: nothing retains the canonical document a session started
  under. `~/.config/ctx/cache` is a per-repo/per-worktree build cache, NOT a digest-addressed
  canonical store, and the ledger records the digest plus a FILE PATH whose contents change.
  So the work is: persist the canonical document with (or addressable from) the session — it is
  content-addressed already, so a digest-keyed store or a per-session copy both work — and make
  resume load by digest, falling back to the recorded path only when the digest still matches.

- **Fix the reporting that makes coexistence look broken.** `ctx traits trust <id>` compares the
  current build against the latest approval for that ID and prints "rebuilt since — the bytes
  running today are unreviewed" even when the running digest is itself approved. That false
  drift report is what makes per-repo/per-version coexistence FEEL unsupported when it is not.
  Report per-digest state, so an approved older digest reads as approved.

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

A session started under digest A still drives to completion after its trait is rebuilt to
digest B — resumed against A's own pinned bytes, with A's existing trust approval honored and
no contract violation; a session whose trait is unchanged resumes exactly as before; when
pinned bytes genuinely cannot be recovered, the refusal is a one-sentence explanation naming
both digests and the options, never a contract-invariant message; an orphaned session is
identifiable from `run-status` without attempting a drive; and `trust <id>` no longer reports
drift for a digest that is itself approved.
