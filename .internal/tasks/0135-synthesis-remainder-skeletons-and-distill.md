# 0135 — Dynamic-synthesis remainder: skeleton-constrained synthesis + `distill <run-id>`

**Status:** ready to implement · **Raised:** 2026-08-05 (0032 migration from the retired `.plans/` board — was P381/P382; P380's `do` verb should be state-checked first)

The dynamic-path family: `story` landed (P383); `do` (P380) may be partially
covered by the existing `synth`/`generate` surface — verify what `ctx traits
synth` actually does before building, and treat `do` as in-scope here only if the
gap is real. The two clearly-open halves:

**Skeleton-constrained synthesis** (was P381): freeform generation produces
plausible-but-novel shapes; synthesized traits should START inside proven shapes.
Synthesis selects a skeleton by task shape (one-shot task → plan-direct-like
single step; iterate-until-good → implement-like capped loop with typed verdict;
risky change → guarded-change) and parameterizes it — prompts filled, slots
renamed, gates chosen — rather than inventing structure; the chosen skeleton id +
parameters land in synthesis provenance. Freeform remains a `--freeform` escape
hatch.

**`ctx traits distill <run-id>`** (was P382): session ledger → trait package
draft. Ledgers are TYPED — steps, roles, slot schemas, verdicts, command
evidence — so distillation emits a BUILDABLE package draft, not prose:
reconstruct the effective procedure (frames → steps, roles → agents,
accepted-slot schemas → slots, loop iterations → loop with observed cap, command
frames → command steps with argv), synthesize prompts from resolved prompt texts
(digests already recorded), cite source run ids + digests in a provenance header.
Multi-ledger input generalizes (shared shape wins, divergences become
parameters). Output `status = draft`, never auto-trusted.

## Watch

- Distillation must never fabricate what the ledger doesn't evidence — steps
  appearing in only one of N runs become optional/flagged, not silently included.
- Secrets/paths hygiene per audit rules.
- No gate skipping anywhere in the dynamic path — that IS the product.

## Done when

The same synthesis request produces a trait whose procedure graph byte-matches its
skeleton modulo declared parameters, with the skeleton named in provenance;
distilling an implement-family run reproduces a recognizably implement-shaped
draft that builds and checks green, with provenance naming every source ledger.

Full original contract: `archived/board/execution-plan.md` (Group 94, P380–P382).
