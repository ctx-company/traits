# 0052 — Repo-local runtime directories move under `.ctx/traits/`

**Status:** ready to implement · **Raised:** 2026-08-02 (owner)

## Today

The product's repo-local state is scattered directly under `.ctx/`, beside `.ctx/traits/`
rather than inside it:

```
.ctx/worktrees/          run worktrees          layout/mod.rs:97  WORKTREE_ROOT
.ctx/cache/traits/       build cache            layout/mod.rs:88  CACHE_ROOT
.ctx/cache/builtin-traits/  builtin store       layout/mod.rs:148 BUILTIN_STORE_ROOT
.ctx/runs/               legacy run ledgers     layout/mod.rs:96  RUN_ROOT
.ctx/debug/              debug traces           layout/mod.rs:92  DEBUG_ROOT
```

`.ctx/` is a shared namespace — other ctx tooling lives there too — so trait-runtime state
sitting at its top level is both a naming claim this product should not make and a layout that
reads as unrelated siblings when they are all one product's working state.

## Target

Everything this product owns lives under `.ctx/traits/`:

```
.ctx/traits/worktrees/
.ctx/traits/cache/traits/           (or .ctx/traits/cache/ — decide the nesting)
.ctx/traits/cache/builtin-traits/
.ctx/traits/runs/
.ctx/traits/debug/
```

The constants above are the single source for each path (`modules/io/src/layout/mod.rs`), so
the change is centralised — but every one of them is PERSISTED somewhere: ledgers record
worktree paths, the gitignore manages entries, existing checkouts have real directories on
disk with live runs inside them.

Managed gitignore (`modules/io/src/gitignore.rs`) must move with them: it currently writes
`worktrees/` and `cache/` into `.ctx/.gitignore`, and lists `.ctx/worktrees`, `.ctx/runs`,
`.ctx/debug` among the paths it manages (:28, :35, :171-175). After the move the ignore lines
belong under `.ctx/traits/`, and the OLD entries must keep being ignored so a checkout that
still has the old directories does not suddenly show them as untracked.

## Watch

- **Never move a directory out from under a live run.** A run's worktree path is recorded in
  its session ledger; relocating it mid-flight breaks the run exactly like the substrate moves
  this repo has been bitten by before. The migration must refuse (or skip) while any
  non-terminal session references a path under the old root, and say so.
- **Dual-read, one release.** Resolve the new location first and fall back to the old one when
  it exists, the way `BUILTIN_STORE_ROOT` already carries a transitional pre-P426 fallback
  (`layout/mod.rs:734`). Existing ledgers, worktrees and caches must keep resolving.
- Provide the migration as an explicit owner command (the `doctor --migrate-state` shape
  already exists for the legacy-to-global move) rather than an implicit relocation on next run.
- `git worktree` metadata records absolute paths in `.git/worktrees/*/gitdir`; moving a linked
  worktree directory without `git worktree repair` leaves it broken. Either move via git's own
  repair path or require the old worktrees to be landed/removed first.
- Sibling repos (ctx-gate) carry the same layout and their own live runs — the fallback has to
  cover them, and their vendored state must not be assumed migrated.
- Check for hard-coded literals beyond the constants: `harness_config.rs` builds worktree-scoped
  paths, `tripwire.rs` excludes `layout::WORKTREE_ROOT`, and test fixtures/proofs contain
  `.ctx/worktrees` strings.

## Done when

The five roots resolve under `.ctx/traits/`, with the old locations still resolving for one
release; the managed gitignore covers both; an owner-invoked migration relocates existing state
(worktrees repaired, not just moved) and refuses while any non-terminal session references the
old root; a fresh `ctx traits init` creates only the new layout; and a full run — dispatch,
worktree, gate, merge — completes in a repo migrated from the old layout and in one created
fresh.
