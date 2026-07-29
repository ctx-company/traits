# 0028 — Version-to-version migration: mechanical, then assisted

**Status:** design needed · **Raised:** 2026-07-29

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
