# 0032 — Move what remains of `.plans/` into `.internal/tasks/`

**Status:** ready to implement · **Raised:** 2026-07-29

Work is tracked as tasks now. Fold the remaining `.plans/` content into
`.internal/tasks/` and retire the directory.

## Watch

- **`.plans/EXECUTION_PLAN.md` is referenced by the `implement` trait** as a
  declared resource, and phase runs read their contract from it. Moving it
  without updating the trait breaks every implement run — the resource is
  digest-pinned, so this is a trait rebuild, relock, and re-approval, not a
  file move.
- Landed-phase records are history, not tasks. Decide explicitly whether they
  are archived or dropped; converting each into a task file would bury the
  actual queue.
- `.plans` is globally gitignored here, so nothing in it is committed and a
  move cannot be recovered from git. Copy before deleting.

## Done when

No live work is tracked outside `.internal/tasks/`; the implement trait reads
its phase contract from wherever the plan now lives; the trait is rebuilt,
relocked, and re-approved.
