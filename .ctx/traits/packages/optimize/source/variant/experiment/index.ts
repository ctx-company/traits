import { condition, defineVariant, flow, intent, method, useBehavior, useIntent } from "@ctx-traits/cdk";

import { baselineResult, candidateResult, readinessSlot, summaryPort } from "../../shared/data.ts";
import { measureBaselineStage, seedBestStage } from "../../shared/stage/baseline.ts";
import { decideCandidate } from "../../shared/stage/decide.ts";
import * as git from "../../shared/stage/git.ts";
import { measureAggregateStage } from "../../shared/stage/measure.ts";
import { abortSummaryStage, baselineFailureSummaryStage, deriveSummaryStage } from "../../shared/stage/summary.ts";
import { experimentCommand } from "./data.ts";
import { applyStage } from "./stage/apply.ts";
import { proposeStage } from "./stage/propose.ts";
import { setupStage } from "./stage/setup.ts";

export default function () {
    defineVariant("Experiment", {
        summary: "Runs bounded experiments whose keep/discard decision is a deterministic guard over typed, N-run-aggregated command output.",
        metadata: { tag: ["first-party", "optimize", "experiments", "provenance"] },
        description: "Verify an isolated worktree, seed a trusted baseline, run an iteration-capped fresh-proposal experiment loop, and deterministically keep only command-measured improvements.",
    });
    useIntent({
        require: [intent.focus.Correctness, intent.require.GatesGreenBeforeCommit],
        avoid: [intent.avoid.RubberStampReview],
    });
    useBehavior({ method: [method.EvidenceFirst] });

    setupStage("Prepare and verify the isolated workbench");

    flow.match("Gate mutation on isolated-worktree readiness", condition.fieldEquals(readinessSlot, "status", "ready"), {
        [flow.True]: () => {
            git.captureInitialRef("Capture the immutable baseline commit");
            git.captureBestRef("Capture the fixed reset ref");
            measureBaselineStage("Measure the baseline", experimentCommand);

            flow.match("Require a usable trusted baseline", condition.fieldEquals(baselineResult, "status", "ok"), {
                [flow.True]: () => {
                    seedBestStage("Seed trusted best state and history");

                    flow.loop("Run the iteration-capped experiment budget", (loop) => {
                        loop.maxIterations(20, { onExhausted: "continue" });
                        proposeStage("Propose one fresh bounded experiment");
                        applyStage("Apply the proposed candidate");
                        measureAggregateStage("Measure the candidate", experimentCommand, candidateResult);
                        decideCandidate();
                    });

                    deriveSummaryStage("Derive the typed experiment summary", "iteration-limit-reached");
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
