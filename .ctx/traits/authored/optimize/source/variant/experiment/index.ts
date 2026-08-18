import { condition, defineVariant, flow, method, signal, useBehavior, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";
import { experimentCommand } from "./data.ts";
import * as stage from "./stage/index.ts";

export default function () {
  defineVariant("Experiment", {
    summary:
      "Runs bounded experiments whose keep/discard decision is a deterministic guard over typed, N-run-aggregated command output.",
    metadata: { tag: shared.metadata.experimentTag },
    description:
      "Verify an isolated worktree, seed a trusted baseline, run an iteration-capped fresh-proposal experiment loop, and deterministically keep only command-measured improvements.",
  });
  useIntent(shared.intent.experiment);
  useBehavior({ method: [method.EvidenceFirst] });

  stage.setup.setupStage("Prepare and verify the isolated workbench");

  flow.match("Gate mutation on isolated-worktree readiness", condition.fieldEquals(shared.data.readinessSlot, "status", "ready"), {
    [condition.True]: () => {
      shared.stage.git.captureInitialRef("Capture the immutable baseline commit");
      shared.stage.git.captureBestRef("Capture the fixed reset ref");
      shared.stage.baseline.measureBaselineStage("Measure the baseline", experimentCommand);

      flow.match("Require a usable trusted baseline", condition.fieldEquals(shared.data.baselineResult, "status", "ok"), {
        [condition.True]: () => {
          shared.stage.baseline.seedBestStage("Seed trusted best state and history");

          flow.loop("Run the iteration-capped experiment budget", (loop) => {
            loop.maxIterations(20, { onExhausted: signal.Continue });
            stage.propose.proposeStage("Propose one fresh bounded experiment");
            stage.apply.applyStage("Apply the proposed candidate");
            shared.stage.measure.measureAggregateStage("Measure the candidate", experimentCommand, shared.data.candidateResult);
            shared.stage.decide.decideCandidate();
          });

          shared.stage.summary.deriveSummaryStage("Derive the typed experiment summary", "iteration-limit-reached");
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
