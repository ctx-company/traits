import * as cdk from "@ctx-traits/cdk";
import { condition } from "@ctx-traits/cdk";

import { bestMetric, candidateResult, decisions, roundCount } from "../../../shared/data.ts";
import { decideCandidate } from "../../../shared/stage/decide.ts";
import * as git from "../../../shared/stage/git.ts";
import { marginResult, noiseThreshold, reviewVerdictSlot, roundComplete, reviews } from "../data.ts";

const deriveMarginScript = `
const [candidateText, bestText, thresholdText] = process.argv.slice(1);
const candidate = JSON.parse(candidateText);
const best = Number(bestText);
const threshold = Number(thresholdText);
const meets = Boolean(candidate) && candidate.status === "ok"
    && Number.isFinite(candidate.metric) && Number.isFinite(best)
    && Number.isFinite(threshold) && threshold >= 0
    && (best - candidate.metric) > threshold;
process.stdout.write(JSON.stringify(meets));
`.trim();

export function deriveMarginStage(title: string) {
  return cdk.step.command(title, {
    id: "derive-margin",
    argv: ["node", "--input-type=module", "--eval", deriveMarginScript, candidateResult, bestMetric, noiseThreshold],
    output: marginResult,
  });
}

/** Review approval only routes INTO measurement; keep/discard is decided strictly by the measured-aggregate + noise-margin condition chain — never by the review verdict itself. */
export function decideCandidateWithMargin(): cdk.SequenceHandle {
  return decideCandidate(
    [condition.equals(marginResult, true)],
    [
      { source: reviewVerdictSlot, field: "status", destination: cdk.operation.over(reviews, cdk.operation.Append) },
      { source: cdk.operation.literal("complete"), destination: roundComplete },
    ],
  );
}

export function recordReviewRejected(): void {
  git.resetCandidate("Discard the non-improving candidate");
  git.cleanCandidate("Remove untracked candidate files");
  cdk.step.project("Record the review-rejected round", {
    id: "record-review-rejected",
    projections: [
      { source: cdk.operation.literal(1), destination: cdk.operation.over(roundCount, cdk.operation.Increment) },
      { source: reviewVerdictSlot, field: "status", destination: cdk.operation.over(reviews, cdk.operation.Append) },
      {
        source: cdk.operation.literal("review-rejected"),
        destination: cdk.operation.over(decisions, cdk.operation.Append),
      },
      { source: cdk.operation.literal("complete"), destination: roundComplete },
    ],
  });
}
