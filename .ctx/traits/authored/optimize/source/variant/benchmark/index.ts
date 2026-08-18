import * as cdk from "@ctx-traits/cdk";
import { condition, defineVariant, flow, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";
import { benchmarkCommand, improvementTarget, reviewVerdictSlot, roundComplete, reviews } from "./data.ts";
import * as stage from "./stage/index.ts";

const seedReviews = () =>
  cdk.step.project("Seed empty review history", {
    id: "seed-reviews",
    projections: [{ source: cdk.operation.literal([]), destination: reviews }],
  });
const resetRound = () =>
  cdk.step.project("Mark the round open", {
    id: "reset-round",
    projections: [{ source: cdk.operation.literal("open"), destination: roundComplete }],
  });

// Never reconstructed from correlated counters: re-evaluates the same
// runtime-owned condition that could have ended the loop, in priority order
// (target reached takes precedence over pure iteration exhaustion).
const runReasonGate = () =>
  flow.match("Deterministically branch on the actual completion cause", condition.lte(shared.data.bestMetric, improvementTarget), {
    [condition.True]: () =>
      shared.stage.summary.deriveSummaryStage("Derive the typed benchmark summary (target reached)", "target-reached", reviews),
    [condition.False]: () =>
      shared.stage.summary.deriveSummaryStage(
        "Derive the typed benchmark summary (iteration limit reached)",
        "iteration-limit-reached",
        reviews,
      ),
  });

export default function () {
  defineVariant("Benchmark", {
    summary:
      "Optimizes a lower-is-better benchmark over a caller-selected code area, keeping only command-measured improvements that clear a noise threshold. Behavior-preserving boundary work belongs to refactor, not here — this trait's doctrine is measurement-gated.",
    metadata: { tag: shared.metadata.benchmarkTag },
    description:
      "Verify an isolated worktree, seed a trusted baseline benchmark, run an iteration-capped scope/draft/implement/review round loop, and deterministically keep only benchmark-measured improvements beyond the noise threshold.",
  });
  useIntent(shared.intent.benchmark);

  stage.setup.setupStage("Prepare and verify the isolated workbench");

  flow.match("Gate mutation on isolated-worktree readiness", condition.fieldEquals(shared.data.readinessSlot, "status", "ready"), {
    [condition.True]: () => {
      shared.stage.git.captureInitialRef("Capture the immutable baseline commit");
      shared.stage.git.captureBestRef("Capture the fixed reset ref");
      shared.stage.baseline.measureBaselineStage("Measure the baseline", benchmarkCommand);

      flow.match("Require a usable trusted baseline", condition.fieldEquals(shared.data.baselineResult, "status", "ok"), {
        [condition.True]: () => {
          shared.stage.baseline.seedBestStage("Seed trusted best state and history");
          seedReviews();

          flow.match(
            "Complete immediately when the baseline already meets the target",
            condition.lte(shared.data.bestMetric, improvementTarget),
            {
              [condition.True]: () => {
                shared.stage.summary.deriveSummaryStage("Derive the typed benchmark summary (already met)", "target-reached", reviews);
              },
              [condition.False]: () => {
                flow.loop("Run the iteration-capped benchmark round budget", (loop) => {
                  loop.maxIterations(12, { onExhausted: cdk.signal.Continue });
                  resetRound();
                  stage.scope.scopeStage("Scope one bounded benchmark-improvement attempt (smart-1)");
                  stage.draft.draftStage("Turn the scope into a concrete worker draft (smart-1)");
                  stage.implement.implementStage("Implement the draft (worker)");
                  stage.review.reviewStage("Review the implemented candidate (smart-1)");
                  flow.match(
                    "Route on the review verdict",
                    condition.fieldEquals(reviewVerdictSlot, "status", "approved"),
                    {
                      [condition.True]: () => {
                        shared.stage.measure.measureAggregateStage("Measure the candidate", benchmarkCommand, shared.data.candidateResult);
                        stage.decide.deriveMarginStage("Derive whether the candidate clears the noise threshold");
                        stage.decide.decideCandidateWithMargin();
                      },
                      [condition.False]: () => {
                        stage.decide.recordReviewRejected();
                      },
                    },
                  );
                  // Exits only once the round's atomic record step has finished AND
                  // the target is met — pure round exhaustion is handled by the loop's
                  // own maxIterations/onExhausted, never by this until (the roundComplete
                  // half only guards against terminating mid-commit/mid-reset).
                  loop.until(
                    condition.all([
                      condition.equals(roundComplete, "complete"),
                      condition.lte(shared.data.bestMetric, improvementTarget),
                    ]),
                  );
                });

                runReasonGate();
              },
            },
          );
        },
        [condition.False]: () => {
          shared.stage.summary.baselineFailureSummaryStage("Report the unusable baseline");
        },
      });
    },
    [condition.False]: () => {
      shared.stage.summary.abortSummaryStage("Report the preflight abort");
    },
  });

  return { summary: shared.data.summaryPort };
}
