# 0128 — Trait-invokes-trait: typed sub-procedure composition

**Status:** ready to implement (DESIGN-FIRST; owner 2026-07-19: affirmed as the longer-term vision — priority anchor for the post-launch arc) · **Raised:** 2026-08-05 (0032 migration from the retired `.plans/` board — was P388)

Competitors compose programs by import/spawn with typed I/O — our biggest
structural gap. Their composition is code calling code; ours should be an ARTIFACT
declaring a dependency on another artifact: a `sequence.call` step naming a trait
ref + typed port mapping (child input ports ← parent slots; child outputs → parent
slots), version-pinnable via `[dependencies]`, locked, drift-checked.

Design doc first: core model (call step kind); session semantics (nested run vs
inlined frames); ledger (child run-id reference + digest); CDK
(`sequence.call(traitRef, {inputs, outputs})`). Implementation phases follow the
design review.

## Watch

- Composition must not launder trust — a local trait calling a restricted trait
  inherits the gate, not the bypass.
- Recursion/depth bounds declared (the overdecomposition lesson, typed).

## Done when

The design doc covers the call model, trust inheritance, session semantics, and
ledger evidence, and is owner-ratified; implementation tasks are filed from it.

Full original contract: `archived/board/execution-plan.md` (Group 96, P388).
