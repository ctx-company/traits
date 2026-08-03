# 0057 — Worktree runs pay a full cold Rust build; they need a warm cache that survives

**Status:** IMPLEMENTED 2026-08-03 — leased cache slots (option 1's warmth without option 1's cost
to forensics). See "What shipped" · **Raised:** 2026-08-03

## The problem

Every run builds the entire Rust workspace from scratch. On a warm checkout the repo gate is
trivial — 56 s of test execution, ~22 s of incremental compile — but inside a fresh worktree it
exceeded 30 minutes, and the timed-out `stop-if` parked three of five runs at ROUND 1
(run-d9183ad4 task 0004, run-003644ea task 0051.1, task 0051). Two concurrent dispatches each pay
it independently.

## Root cause, verified (do not re-derive)

**Cargo anchors freshness to absolute paths.** Every dep-info file records its own outputs by
absolute path — 454 of 454 in this repo's target reference `/…/ctx-trait/target/...`. Move or copy
the directory and every unit is stale, including third-party crates whose sources never moved
(they live in `~/.cargo/registry`, unchanged).

Proven by controlled experiment on 2026-08-03: identical source tree, identical mtimes, identical
environment, only `CARGO_TARGET_DIR` pointed at a copy-on-write clone of the existing target —
cargo immediately recompiled `memchr`, `libc`, `serde`, `winnow`, `ctx-traits-core`. One variable,
full rebuild. In the parked worktree, all 511 crate fingerprints carried the run's start time.

Ruled out: cargo lock contention (no "Blocking waiting for file lock" in any gate tail; targets are
per-worktree and genuinely isolated), mtime skew (the CoW clone preserves them), source churn
(registry crates rebuilt too).

## Already done (stopgaps, not the fix)

- `[worktree] warm = ["target"]` REMOVED — it cloned ~14 GB per run for a target cargo discards
  (13 worktrees held 190 GB).
- `repo-gates` `timeout-ms` raised 30 min → 90 min so a cold build no longer parks a run at round 1.

## What shipped

Neither listed option as written. Option 1 kept both paths stable but made worktrees reusable, which
would have destroyed the per-run forensic artifact this loop is repeatedly harvested from; option 2
kept one path stable but made concurrent runs share it. What shipped keeps the worktree per-run and
disposable and moves ONLY the build directory to a stable path, leased:

- `modules/io/src/target_slot.rs` — a small fixed set of directories under the repository's global
  cache root (`<cache-root>/build-slots/slot-N`, `DEFAULT_TARGET_SLOTS = 4`), assigned per worktree
  through a lock-guarded registry. The same worktree always resolves to the same slot (a run stays
  warm across frames, and a parked worktree stays warm for its resume); no two LIVE worktrees hold
  the same slot; a slot whose worktree no longer exists is handed to the next run, warm. No release
  hook and no pid liveness, so a killed driver leaks nothing.
- `{cache-slot}` — a `[worktree.env]` token beside `{worktree}`, dropped (not resolved) when there is
  no worktree, and refusing `..` exactly as `{worktree}` does. This repo now declares
  `CARGO_TARGET_DIR = "{cache-slot}"`.
- `justfile` — the hard `CARGO_TARGET_DIR` override is gone; an inherited value now wins
  (`gate_target_dir`). This is the trap noted below, and it is safe to reverse precisely because the
  slot is leased: the override existed to stop runs SHARING a cache, which leasing prevents.

Verified end-to-end through the real resolver: same worktree → same slot twice; a second live
worktree → a different slot; a removed worktree's slot → reused by the next run.

**Honest limits.** The first run in each slot is still cold — a cache cannot be seeded by copying,
which is the whole finding above. Warmth starts from the second run per slot.

More importantly, this warms DEPENDENCIES, not the workspace's own crates. Registry crates build
from `~/.cargo/registry`, a path that never moves, so a slot's copies of them stay valid for every
run that inherits it. Our ~50 local crates build from the WORKTREE, a path that is new every run, so
they are rebuilt each time by the same path-anchoring this task documents. That is the residual
cost, and it is why 0056 matters alongside this: with the ~40 proof binaries moved to the landing
gate, what a round still pays for is roughly those 50 local units rather than the whole graph.
Eliminating even that needs the source path to recur too — the worktree POOL of option 1 — which was
declined here for the forensic reason above. Measure a real run's gate before deciding it is worth
revisiting. And a fifth concurrent
run, or four parked worktrees holding every slot, falls back to sharing the least-recently-assigned
slot and pays the toolchain's build lock; prune worktrees, or raise the slot count, if that becomes
common.

## Options considered (the trade each carried)

1. **Fixed worktree pool.** N slots at constant paths (`slot-1`…`slot-N`), leased per run, reset
   between runs. Both the source path and the target path recur, so dependencies AND local crates
   stay warm; no lock waiting, no cross-branch thrash. Costs: lease/reset machinery, and worktrees
   stop being per-run forensic artifacts — a run must land or export its diff before its slot is
   reused. This is the complete fix.
2. **One shared target via the git common dir.** `CARGO_TARGET_DIR=$(git rev-parse
   --path-format=absolute --git-common-dir)/cargo-target` — verified to return the same path from
   the main checkout and from inside a worktree. Keeps the ~460 dependency units warm. Two costs:
   concurrent builds serialize on cargo's build lock (bearable — waiting minutes for a warm build
   beats 30 minutes of parallel cold rebuild), and concurrent runs on DIFFERENT revisions overwrite
   each other's workspace-crate artifacts, so our own crates plus ~40 proof binaries thrash.
   **Implementation trap:** `just test`/`just lint` hard-set `CARGO_TARGET_DIR` (justfile:52,57), so
   the worktree env alone changes nothing — both must move together. Note those overrides were
   added deliberately after cross-worktree cache flakes; this reverses that decision.
3. **`sccache`** with `SCCACHE_BASEDIRS` for path normalisation. Owner deprioritised 2026-08-03.
4. **Rewrite the absolute prefix inside copied `.d` files.** Recovers dependencies only (local crate
   source paths still change) and depends on a private cargo format. Hack of last resort.

## The measurement that decides between them

What share of a cold build is the ~460 dependency units versus our ~50 local units and ~40 proof
binaries? If dependencies dominate, option 2 alone may be enough and the pool never needs building.
Take this measurement before committing to a design.

## Watch

- Whatever lands must keep concurrent runs genuinely independent — the reason per-worktree targets
  exist is that a shared cache once produced phantom "cannot find X" failures that read as real gate
  failures.
- Task 0056 (cheap per-round gate, full suite at merge) is complementary, not a substitute: a
  build-only gate still pays the cold compile once per worktree.

## Done when

A second and subsequent run in a given slot (or against the shared cache) compiles incrementally
rather than from scratch; a repo gate in a run's worktree completes in single-digit minutes;
concurrent runs neither wait on each other for the whole build nor invalidate each other's
workspace crates; and the gate `timeout-ms` stops needing to track the repo's growth.
