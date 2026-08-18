import * as cdk from "@ctx-traits/cdk";
import { condition, flow } from "@ctx-traits/cdk";

import { bestMetric, candidateResult, decisions, history, keptCount, maxDelta, roundCount } from "#trait/shared/data.ts";
import * as git from "#trait/shared/stage/git.ts";

// Each optional cap contributes one composed arm: absent -> passes
// automatically; present -> the candidate must also report the capped
// field, and clear it. Supplied-but-unmeasurable therefore discards.
export function capGuard(cap: cdk.PortHandle<number>, field: string): cdk.GuardValue {
  return condition.any([
    condition.absent(cap),
    condition.all([
      condition.present(cap),
      condition.present(candidateResult, { field }),
      condition.fieldLte(candidateResult, field, cap),
    ]),
  ]);
}

export const maxDeltaGuard: cdk.GuardValue = capGuard(maxDelta, "delta-lines");

/**
 * The deterministic keep/discard guard: the model never gets to claim an
 * improvement — the measured aggregate decides. `extraGuards` composes in
 * only typed condition arms (e.g. benchmark's noise-margin check); a review
 * verdict must never appear here — review approval only routes INTO
 * measurement (see benchmark's review-gate), it never decides keep/discard.
 */
export function decideCandidate(
  extraGuards: readonly cdk.GuardValue[] = [],
  recordExtraProjections: readonly cdk.ProjectProjection[] = [],
): cdk.SequenceHandle {
  const keepGuard = condition.all([
    condition.fieldEquals(candidateResult, "status", "ok"),
    condition.fieldLt(candidateResult, "metric", bestMetric),
    maxDeltaGuard,
    ...extraGuards,
  ]);
  return flow.match("Apply the declared keep/discard decision", keepGuard, {
    [condition.True]: () => {
      git.deriveCommitMessage("Derive the kept-candidate commit message");
      git.stageCandidate("Stage the kept candidate");
      git.commitCandidate("Commit the kept candidate");
      git.captureCommittedRef("Capture the kept commit SHA");
      git.advanceBest("Advance trusted best state after commit");
      cdk.step.project("Record the kept candidate", {
        id: "record-kept-candidate",
        projections: [
          { source: cdk.operation.literal(1), destination: cdk.operation.over(roundCount, cdk.operation.Increment) },
          { source: cdk.operation.literal(1), destination: cdk.operation.over(keptCount, cdk.operation.Increment) },
          { source: candidateResult, destination: cdk.operation.over(history, cdk.operation.Append) },
          { source: cdk.operation.literal("kept"), destination: cdk.operation.over(decisions, cdk.operation.Append) },
          ...recordExtraProjections,
        ],
      });
    },
    [condition.False]: () => {
      git.resetCandidate("Discard the non-improving candidate");
      git.cleanCandidate("Remove untracked candidate files");
      cdk.step.project("Record the discarded candidate", {
        id: "record-discarded-candidate",
        projections: [
          { source: cdk.operation.literal(1), destination: cdk.operation.over(roundCount, cdk.operation.Increment) },
          { source: candidateResult, destination: cdk.operation.over(history, cdk.operation.Append) },
          {
            source: cdk.operation.literal("discarded"),
            destination: cdk.operation.over(decisions, cdk.operation.Append),
          },
          ...recordExtraProjections,
        ],
      });
    },
  });
}
