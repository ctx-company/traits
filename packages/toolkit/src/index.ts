// NOT "./agent.js" etc: unlike @ctx-traits/cdk and @ctx-traits/config, this
// package's package.json "exports" points dev consumers straight at
// ./src/index.ts (no dist build step in the loop), so a CDK build's plain
// `node --eval` import never resolves through dist — Node only resolves a
// specifier that names a file that actually exists on disk, and dist/*.js
// is never consulted here. Verified (packages/agents/src/index.ts): a
// "./process.js" specifier throws ERR_MODULE_NOT_FOUND from a real
// `ctx traits build` invocation.
export { clerkRole, reviewerRole, rustReviewerRole, rustWorkerRole, scribeRole, workerRole } from "./agent.ts";
export {
  CODE_INTEGRITY_DOCTRINE,
  DEFAULT_VARIANT_DOCTRINE,
  FEASIBILITY_DOCTRINE,
  INTEGRITY_DOCTRINE,
  LEFTOVER_DOCTRINE,
  QUICK_VARIANT_DOCTRINE,
  REVIEW_VERDICT_DOCTRINE,
  SCOPE_SPLIT_DOCTRINE,
  SMART_VARIANT_DOCTRINE,
  STRICT_VARIANT_DOCTRINE,
} from "./data.ts";
export {
  blockerSchema,
  blockerStepSchema,
  deviationReportSchema,
  feasibilityVerdictSchema,
  leftoverSchema,
  ownerItemSchema,
  planAmendmentSchema,
  reviewerVerdict,
  reviewVerdictSchema,
} from "./schema.ts";
export type { FeasibilityVerdictValue, ReviewerVerdictValue } from "./schema.ts";
export { commitTail } from "./sequence/commit-tail.ts";
export type { CommitTailOptions } from "./sequence/commit-tail.ts";
export { gateAndDiffEvidence } from "./sequence/evidence.ts";
export type { GateAndDiffEvidenceOptions } from "./sequence/evidence.ts";
export { feasibilityGate } from "./sequence/feasibility.ts";
export type { FeasibilityGateOptions } from "./sequence/feasibility.ts";
export { forEachJoined } from "./sequence/for-each.ts";
export type { ForEachJoinedOptions } from "./sequence/for-each.ts";
export { guardedProduction } from "./sequence/guarded-production.ts";
export type {
  GuardedProductionHandle,
  GuardedProductionOptions,
  GuardedProductionRole,
} from "./sequence/guarded-production.ts";
export { deriveParkReportStep } from "./sequence/park-report.ts";
