# 0138 — Merge/worktree hygiene remainder: park prunes declared caches, doctor debris sweep

**Status:** ready to implement (first step: audit what landed — the disk-preflight half exists as `DEFAULT_MERGE_DISK_FLOOR_MB` + `[merge] disk-floor-mb`, and 0057's leased cache slots changed the economics the original contract assumed) · **Raised:** 2026-08-05 (0032 migration from the retired `.plans/` board — was P462, owner-ordered after the ENOSPC incident chain; LANGUAGE-AGNOSTIC by owner directive)

Incident basis: parked worktrees each retained a multi-GB regenerable build
directory; a park-heavy stretch filled the disk; a landing gate died mid-build
with a cryptic toolchain exit (misdiagnosed as a code defect); the ENOSPC also
killed merge cleanups midway — 47 dead branches and 87 phantom worktree
registrations swept by hand.

Remaining halves:

**(a) Park prunes regenerable artifacts:** when any merge path parks a run, the
worktree is retained for triage MINUS its regenerable artifact directories —
defined as the directories named by the project's build-cache DECLARATIONS (the
cache-shaped entries of the worktree env overlay; today's `{cache-slot}`
CARGO_TARGET_DIR changes this calculus — a leased slot outside the worktree is
not the worktree's to prune, so re-derive what "regenerable inside the worktree"
still means before implementing). Deletion is safe BY DEFINITION only for a
declared cache; sources, ledger evidence, and seeded files untouched. Never
hardcode any language's build dir.

**(b) Doctor debris sweep:** `doctor` learns three findings with `--apply` fixes,
all idempotent: worktree registrations whose directory no longer exists (prune);
run branches whose tip is already contained in the default branch (delete — the
corpses an interrupted cleanup leaves); stale landing-gate temp binaries
(delete). Doctor NEVER touches unmerged branches, present worktrees, or anything
outside those three classes.

## Watch

- Park semantics otherwise unchanged (parking preserves everything triage needs).
- The free-space floor is config, not code (already landed — verify the wiring).
- Grep-provable absence of cargo/pnpm/npm literals in the merge/park/doctor code
  paths.

## Done when

Parking a run leaves zero declared-cache bytes behind (under the current
cache-slot model, with the re-derived definition recorded) while sources/evidence
survive; doctor on a debris-seeded fixture reports exactly the three classes and
`--apply` clears exactly them, twice-idempotent; the toolchain-literal grep over
the touched code paths is clean.

Full original contract: `archived/board/execution-plan.md` (Group 106, P462).
