import * as cdk from "@ctx-traits/cdk";
import { condition, defineVariant, flow, intent, useIntent } from "@ctx-traits/cdk";

import { baselineResult, bestMetric, candidateResult, readinessSlot, summaryPort } from "../../shared/data.ts";
import { measureBaselineStage, seedBestStage } from "../../shared/stage/baseline.ts";
import * as git from "../../shared/stage/git.ts";
import { measureAggregateStage } from "../../shared/stage/measure.ts";
import { abortSummaryStage, baselineFailureSummaryStage, deriveSummaryStage } from "../../shared/stage/summary.ts";
import { benchmarkCommand, improvementTarget, reviewVerdictSlot, roundComplete, reviews } from "./data.ts";
import { decideCandidateWithMargin, deriveMarginStage, recordReviewRejected } from "./stage/decide.ts";
import { draftStage } from "./stage/draft.ts";
import { implementStage } from "./stage/implement.ts";
import { reviewStage } from "./stage/review.ts";
import { scopeStage } from "./stage/scope.ts";
import { setupStage } from "./stage/setup.ts";

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
    flow.match("Deterministically branch on the actual completion cause", condition.lte(bestMetric, improvementTarget), {
        [flow.True]: () => deriveSummaryStage("Derive the typed benchmark summary (target reached)", "target-reached", reviews),
        [flow.False]: () => deriveSummaryStage("Derive the typed benchmark summary (iteration limit reached)", "iteration-limit-reached", reviews),
    });

export default function () {
    defineVariant("Benchmark", {
        summary: "Optimizes a lower-is-better benchmark over a caller-selected code area, keeping only command-measured improvements that clear a noise threshold. Behavior-preserving boundary work belongs to refactor, not here — this trait's doctrine is measurement-gated.",
        metadata: { tag: ["first-party", "optimize", "benchmark", "review", "provenance"] },
        procedure: "Verify an isolated worktree, seed a trusted baseline benchmark, run an iteration-capped scope/draft/implement/review round loop, and deterministically keep only benchmark-measured improvements beyond the noise threshold.",
    });
    useIntent({
        require: [intent.require.ReviewBeforeFinal, intent.focus.Correctness],
        avoid: [intent.avoid.RubberStampReview],
    });

    setupStage("Prepare and verify the isolated workbench");

    flow.match("Gate mutation on isolated-worktree readiness", condition.fieldEquals(readinessSlot, "status", "ready"), {
        [flow.True]: () => {
            git.captureInitialRef("Capture the immutable baseline commit");
            git.captureBestRef("Capture the fixed reset ref");
            measureBaselineStage("Measure the baseline", benchmarkCommand);

            flow.match("Require a usable trusted baseline", condition.fieldEquals(baselineResult, "status", "ok"), {
                [flow.True]: () => {
                    seedBestStage("Seed trusted best state and history");
                    seedReviews();

                    flow.match("Complete immediately when the baseline already meets the target", condition.lte(bestMetric, improvementTarget), {
                        [flow.True]: () => {
                            deriveSummaryStage("Derive the typed benchmark summary (already met)", "target-reached", reviews);
                        },
                        [flow.False]: () => {
                            flow.loop("Run the iteration-capped benchmark round budget", (loop) => {
                                loop.maxIterations(12, { onExhausted: "continue" });
                                resetRound();
                                scopeStage("Scope one bounded benchmark-improvement attempt (smart-1)");
                                draftStage("Turn the scope into a concrete worker draft (smart-1)");
                                implementStage("Implement the draft (worker)");
                                reviewStage("Review the implemented candidate (smart-1)");
                                flow.match("Route on the review verdict", condition.fieldEquals(reviewVerdictSlot, "status", "approved"), {
                                    [flow.True]: () => {
                                        measureAggregateStage("Measure the candidate", benchmarkCommand, candidateResult);
                                        deriveMarginStage("Derive whether the candidate clears the noise threshold");
                                        decideCandidateWithMargin();
                                    },
                                    [flow.False]: () => {
                                        recordReviewRejected();
                                    },
                                });
                                // Exits only once the round's atomic record step has finished AND
                                // the target is met — pure round exhaustion is handled by the loop's
                                // own maxIterations/onExhausted, never by this until (the roundComplete
                                // half only guards against terminating mid-commit/mid-reset).
                                flow.until(condition.all([
                                    condition.equals(roundComplete, "complete"),
                                    condition.lte(bestMetric, improvementTarget),
                                ]));
                            });

                            runReasonGate();
                        },
                    });
                },
                [flow.False]: () => {
                    baselineFailureSummaryStage("Report the unusable baseline");
                },
            });
        },
        [flow.False]: () => {
            abortSummaryStage("Report the preflight abort");
        },
    });

    return { summary: summaryPort };
}
