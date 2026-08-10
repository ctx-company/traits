# @ctx-traits/toolkit

The shared `ctx.traits` CDK toolkit for reviewed multi-agent build loops:
roles, typed schemas, doctrine strings, and sequence factories that more than
one family trait has proven and reused. `@ctx-traits/agents` and
`@ctx-traits/rust` re-export this package's symbols for backward
compatibility — this package is the sole definition site.

## Import

```sh
pnpm add -D @ctx-traits/toolkit
```

```ts
import {
  clerkRole,
  commitTail,
  feasibilityGate,
  forEachJoined,
  gateAndDiffEvidence,
  guardedProduction,
  reviewerRole,
  REVIEW_VERDICT_DOCTRINE,
  reviewVerdictSchema,
  scribeRole,
  workerRole,
} from "@ctx-traits/toolkit";
import { prompt, trait } from "@ctx-traits/cdk";
```

This import is authoring-time only: it resolves through ordinary package
management, never through `ctx.traits` dependency resolution. Editing this
package's `src/` surfaces as reviewable CDK drift on every consumer that
rebuilds, never as a silent behavior change.

## Package contents

- `src/agent.ts` — `workerRole`, `scribeRole`, `reviewerRole`, `clerkRole`,
  and the Rust-specialized pair `rustWorkerRole`/`rustReviewerRole`.
- `src/schema.ts` — `blockerSchema`, `blockerStepSchema`,
  `reviewVerdictSchema`, `reviewerVerdict`, `leftoverSchema`,
  `ownerItemSchema`, `deviationReportSchema`, `planAmendmentSchema`,
  `feasibilityVerdictSchema`.
- `src/data.ts` — the doctrine strings: `SCOPE_SPLIT_DOCTRINE`,
  `REVIEW_VERDICT_DOCTRINE`, `INTEGRITY_DOCTRINE`, `CODE_INTEGRITY_DOCTRINE`,
  `LEFTOVER_DOCTRINE`, `FEASIBILITY_DOCTRINE`, and the four reviewer-authority
  fragments `QUICK_VARIANT_DOCTRINE`/`DEFAULT_VARIANT_DOCTRINE`/
  `SMART_VARIANT_DOCTRINE`/`STRICT_VARIANT_DOCTRINE`.
- `src/sequence/guarded-production.ts` — `guardedProduction`, the
  produce-review refinement loop composite.
- `src/sequence/commit-tail.ts` — `commitTail`, the clean-tree-guarded git
  tail.
- `src/sequence/feasibility.ts` — `feasibilityGate`, the pre-build triage
  step.
- `src/sequence/evidence.ts` — `gateAndDiffEvidence`, the repository
  gate-check-plus-scoped-diff pair a refinement round runs before review.
- `src/sequence/park-report.ts` — `deriveParkReportStep`, the deterministic
  park-report projection from a round's verdict(s).
- `src/sequence/for-each.ts` — `forEachJoined`, per-item processing over a
  list slot with a joined result (wraps `sequence.forEach`).

Pure draft builders only: no filesystem access, commands, provider calls,
synthesis, or output writes.

## Watch

Doctrine strings are the sharpest over-abstraction risk in this package:
`implement-quick` was cut from five spliced doctrine blocks to zero
(2026-07-27) and reads better for it. These strings are exported as
available material, never as an implication that composing them is good
practice — most traits should splice fewer of them, not more.

`sequence.parallel` (fan-out-then-join) has no wrapper here: zero existing
consumers, and adopting it anywhere the store doesn't already use it would
change run behavior. It is a typed leftover for a future consumer (0119's
capture-reduce-fix loop), not a fourth builder in `sequence/`.

## Build

```sh
pnpm --dir packages/toolkit run build        # tsc -> dist/
pnpm --dir packages/toolkit run typecheck
pnpm --dir packages/toolkit run lint
pnpm --dir packages/toolkit run format:check
```
