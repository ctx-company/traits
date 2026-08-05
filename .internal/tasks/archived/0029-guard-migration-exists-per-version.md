# 0029 — Gate our own releases on a migration existing for the new version

**Status:** done · **Raised:** 2026-07-29 · **Closed:** 2026-08-05

Implemented as two lib tests in `modules/core/src/migrate.rs` under
`tests::release_gate`: `every_supported_version_step_has_a_declared_migration`
asserts `MIGRATION_STEPS` mirrors `SUPPORTED_SCHEMA_VERSIONS` exactly (catches
missing/stray/out-of-order steps), and `every_declared_step_actually_migrates`
runs `plan_migration` per declared step against a minimal fixture. Both run
inside `just test` (`cargo test --workspace --lib`), the repo's own per-round
gate — no new gate wiring. `SUPPORTED_SCHEMA_VERSIONS` in
`modules/core/src/trait/mod.rs` now doc-comments the requirement.

A schema version bump must not land without its migration. Add a gate: if the
canonical schema version changes and no mechanical+assisted migration is
declared for that step, the build fails.

## Why this is worth doing beyond correctness

This is the product's own thesis applied to itself: a typed, versioned artifact
whose migration is a first-class, gated, reproducible object rather than a
release note. It is a strong showcase — we ship a tool that says behavior should
be provable and reproducible, and our own version bumps are gated on exactly
that.

## Watch

- The gate must key on the SCHEMA version, not the crate version. They move
  independently and only one of them invalidates authored traits.
- A version bump with genuinely nothing to migrate still needs an explicit
  declaration ("no-op, nothing changed for authors") — silence must not pass.
  A silent pass is indistinguishable from a forgotten migration.
- Keep it in the same gate the phase loop already runs, or it will be skipped
  by the path that matters.

## Done when

Bumping the schema version without a declared migration fails the gate; a
declared no-op passes explicitly; the check runs in the repository's own gate.
