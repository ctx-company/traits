// NOT "./process.js": unlike @ctx-traits/cdk and @ctx-traits/config, this
// package's package.json "exports" points dev consumers straight at
// ./src/index.ts (no dist build step in the loop), so a CDK build's plain
// `node --eval` import never resolves through dist — Node only resolves a
// specifier that names a file that actually exists on disk, and dist/*.js
// is never consulted here. Verified: a "./process.js" specifier throws
// ERR_MODULE_NOT_FOUND from a real `ctx traits build` invocation.
export {
  blockerSchema,
  blockerStepSchema,
  clerkRole,
  CODE_INTEGRITY_DOCTRINE,
  DEFAULT_VARIANT_DOCTRINE,
  deviationReportSchema,
  INTEGRITY_DOCTRINE,
  LEFTOVER_DOCTRINE,
  leftoverSchema,
  ownerItemSchema,
  planAmendmentSchema,
  QUICK_VARIANT_DOCTRINE,
  REVIEW_VERDICT_DOCTRINE,
  reviewerRole,
  reviewVerdictSchema,
  SCOPE_SPLIT_DOCTRINE,
  scribeRole,
  SMART_VARIANT_DOCTRINE,
  STRICT_VARIANT_DOCTRINE,
  workerRole,
} from "@ctx-traits/toolkit";
export { FEASIBILITY_DOCTRINE, feasibilityGate, feasibilityVerdictSchema } from "./feasibility.ts";
export type { FeasibilityGateOptions, FeasibilityVerdictValue } from "./feasibility.ts";
export { commitTail, guardedProduction, reviewerVerdict } from "./process.ts";
export type {
  CommitTailOptions,
  GuardedProductionHandle,
  GuardedProductionOptions,
  GuardedProductionRole,
  ReviewerVerdictValue,
} from "./process.ts";
export { declareTaskSlot, stepSchema, subtaskNodeSchema, taskSchema } from "./task.ts";
export type { StepValue, SubtaskNodeValue, TaskValue } from "./task.ts";
