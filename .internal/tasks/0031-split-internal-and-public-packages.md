# 0031 — Split internal from public packages; ship only what is needed

**Status:** design needed · **Raised:** 2026-07-29

Decide, per package, whether it is PUBLIC (consumed by users of ctx.traits) or
INTERNAL (ours), and make the generated/published artifacts match. Today the
distinction is implicit and generated output is emitted broadly.

## What to establish

- An explicit public surface. `@ctx-traits/cdk` and the config types are
  plausibly public; the agents kit, harness client, and plugin scaffolds are
  arguably internal.
- Generated files emitted only where a consumer needs them. Generation that
  exists solely to satisfy our own build should not ship.
- A gate, so the split does not rot: publishing an internal package, or a
  public one importing an internal one, fails.

## Watch

- `packages/config/src/generated.ts` is generated FROM the Rust schema and
  consumed by authors — it is public and must stay generated, not hand-kept.
  Use it as the worked example of the boundary rather than a special case.
- npm publication currently has no wiring at all (release.yml ships binaries
  and a Homebrew formula only), so this decision is being made before anything
  is published — the cheap moment.
- Splitting changes import paths for anyone already depending on the current
  names. Nothing external does yet; that will not be true after launch.

## Done when

Every package is explicitly public or internal; generated output is scoped to
real consumers; a gate refuses an internal package leaking into the public set.
