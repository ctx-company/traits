import { behavior, condition, defineVariant, flow, signal, useBehavior, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";
import { experimentCommand } from "./data.ts";
import * as step from "./step/index.ts";

export default function () {
  defineVariant("Experiment", {
    summary:
      "Runs bounded experiments whose keep/discard decision is a deterministic guard over typed, N-run-aggregated command output.",
    metadata: { tag: shared.metadata.experimentTag },
    description:
      "Verify an isolated worktree, seed a trusted baseline, run an iteration-capped fresh-proposal experiment loop, and deterministically keep only command-measured improvements.",
  });
  useIntent(shared.intent.experiment);
  useBehavior({ method: [behavior.method.EvidenceFirst] });

  step.setup.setupStep("Prepare and verify the isolated workbench");

  flow.match("Gate mutation on isolated-worktree readiness", condition.fieldEquals(shared.data.readinessSlot, "status", "ready"), {
    [condition.True]: () => {
      shared.step.git.captureInitialRef("Capture the immutable baseline commit");
      shared.step.git.captureBestRef("Capture the fixed reset ref");
      shared.step.baseline.measureBaselineStep("Measure the baseline", experimentCommand);

      flow.match("Require a usable trusted baseline", condition.fieldEquals(shared.data.baselineResult, "status", "ok"), {
        [condition.True]: () => {
          shared.step.baseline.seedBestStep("Seed trusted best state and history");

          flow.loop("Run the iteration-capped experiment budget", (loop) => {
            loop.maxIterations(20, { onExhausted: signal.Continue });
            step.propose.proposeStep("Propose one fresh bounded experiment");
            step.apply.applyStep("Apply the proposed candidate");
            shared.step.measure.measureAggregateStep("Measure the candidate", experimentCommand, shared.data.candidateResult);
            shared.step.decide.decideCandidate();
          });

          shared.step.summary.deriveSummaryStep("Derive the typed experiment summary", "iteration-limit-reached");
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
