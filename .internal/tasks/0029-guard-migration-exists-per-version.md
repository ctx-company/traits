# 0029 — Gate our own releases on a migration existing for the new version

**Status:** ready to implement (depends on 0028) · **Raised:** 2026-07-29

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
