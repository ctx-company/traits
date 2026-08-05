# 0028 — Version-to-version migration: mechanical, then assisted

**Status:** mechanical tier implemented, assisted tier deferred (seam designed only) · **Raised:** 2026-07-29

A trait authored against schema version N must be migratable to N+1. Two tiers:

- **Mechanical** — deterministic rewrites (renamed key, moved table, changed
  default). No model. Reproducible, diffable, safe to run unattended.
- **Assisted** — a model handles what no rule can: prose that must be rewritten,
  a field whose meaning changed, a shape that now needs a decision.

## The hard case: skipping versions

Jumping N → N+2 is fine mechanically — compose the rewrites and apply in order.
It degrades badly when assisted: composing two model passes means the second
one reasons about the output of the first rather than about the author's
original intent, and errors compound.

Options, to be decided:

1. **Always step one version at a time**, assisted or not. Slow, N passes, but
   each pass sees a coherent input.
2. **Mechanical composes, assisted does not** — run the full mechanical chain
   N → N+2 in one go, then ONE assisted pass against the final target schema.
   The assisted pass sees the original prose and the final shape, never an
   intermediate.

Option 2 is likely right: it keeps the model's input authored-by-a-human rather
than authored-by-a-previous-model, which is the actual failure mode.

**Decided: Option 2.** Implemented in `ctx_traits_core::migrate` (module
doc-comment carries the same rationale): mechanical steps compose N→M over
one document; the assisted pass (not yet built) would run at most once,
against the final target schema, over the original author's prose captured
before any mechanical rewrite.

## Implementation (2026-08-05)

- `ctx_traits_core::migrate`: pure planning (`plan_migration`), an ordered
  `MIGRATION_STEPS` registry (one entry today: 0.2→0.3, a pure
  `schema-version` bump — every 0.3-gated feature is additive), round-trip
  validation via `encoding::decode_trait`, and `AssistedItem`/
  `assisted_needed` as the designed (unpopulated) assisted seam.
- `ctx traits migrate <trait>` CLI subcommand (plan/apply/`--json`),
  mirroring the `doctor migrate-state`/`migrate-config` plan/apply shape:
  plan mode prints a diff and digest before/after without writing; `--apply`
  writes and reports the canonical-digest move plus the
  `ctx traits trust approve` follow-up.
- Not built: the assisted model pass itself (blocks `--apply` today via a
  refusal if `assisted_needed` is ever nonempty — currently always empty,
  since the only registered step is a pure bump).

## Watch

- A migration must be refusable. "I cannot migrate this safely" is a valid
  outcome and better than a plausible-looking rewrite of something semantic.
- Mechanical output must be byte-stable, so a migration can be re-run and
  diffed rather than trusted.
- Digests move on migration. Trust re-approval follows, and the migration
  should say so rather than leaving the operator to discover it.

## Done when

A trait migrates N → N+1 mechanically with a reviewable diff; the multi-version
strategy is chosen and documented; an unsafe migration refuses rather than
guesses.
