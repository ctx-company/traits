import * as cdk from "@ctx-traits/cdk";
import { condition, defineVariant, flow, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";
import { benchmarkCommand, improvementTarget, reviewVerdictSlot, roundComplete, reviews } from "./data.ts";
import * as step from "./step/index.ts";

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
      shared.step.summary.deriveSummaryStep("Derive the typed benchmark summary (target reached)", "target-reached", reviews),
    [condition.False]: () =>
      shared.step.summary.deriveSummaryStep(
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

  step.setup.setupStep("Prepare and verify the isolated workbench");

  flow.match("Gate mutation on isolated-worktree readiness", condition.fieldEquals(shared.data.readinessSlot, "status", "ready"), {
    [condition.True]: () => {
      shared.step.git.captureInitialRef("Capture the immutable baseline commit");
      shared.step.git.captureBestRef("Capture the fixed reset ref");
      shared.step.baseline.measureBaselineStep("Measure the baseline", benchmarkCommand);

      flow.match("Require a usable trusted baseline", condition.fieldEquals(shared.data.baselineResult, "status", "ok"), {
        [condition.True]: () => {
          shared.step.baseline.seedBestStep("Seed trusted best state and history");
          seedReviews();

          flow.match(
            "Complete immediately when the baseline already meets the target",
            condition.lte(shared.data.bestMetric, improvementTarget),
            {
              [condition.True]: () => {
                shared.step.summary.deriveSummaryStep("Derive the typed benchmark summary (already met)", "target-reached", reviews);
              },
              [condition.False]: () => {
                flow.loop("Run the iteration-capped benchmark round budget", (loop) => {
                  loop.maxIterations(12, { onExhausted: cdk.signal.Continue });
                  resetRound();
                  step.scope.scopeStep("Scope one bounded benchmark-improvement attempt (smart-1)");
                  step.draft.draftStep("Turn the scope into a concrete worker draft (smart-1)");
                  step.implement.implementStep("Implement the draft (worker)");
                  step.review.reviewStep("Review the implemented candidate (smart-1)");
                  flow.match(
                    "Route on the review verdict",
                    condition.fieldEquals(reviewVerdictSlot, "status", "approved"),
                    {
                      [condition.True]: () => {
                        shared.step.measure.measureAggregateStep("Measure the candidate", benchmarkCommand, shared.data.candidateResult);
                        step.decide.deriveMarginStep("Derive whether the candidate clears the noise threshold");
                        step.decide.decideCandidateWithMargin();
                      },
                      [condition.False]: () => {
                        step.decide.recordReviewRejected();
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
          shared.step.summary.baselineFailureSummaryStep("Report the unusable baseline");
        },
      });
    },
    [condition.False]: () => {
      shared.step.summary.abortSummaryStep("Report the preflight abort");
    },
  });

  return { summary: shared.data.summaryPort };
}
