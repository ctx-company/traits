import * as cdk from "@ctx-traits/cdk";

import { worker } from "../agent.ts";
import { slot } from "../data.ts";

// The worker's inputs (0281.7): the plan is what to do; the brief is where
// to do it now — the first open stage, its goal and proof, and the front
// step with its frozen done-when — copied out of the reviewer's verdict by
// a deterministic step, so the worker never sees the rest of the queue;
// its own previous report is its memory of the previous dispatch; the
// proof result is what the stage's proof command said about its last
// claim. The conventions (one unit of attention, the stage as the unit of
// completeness, what a claim means) live in the brief's and the report's
// field descriptions, which the runtime renders beside the values.
export const implement = cdk.defineStep.prompt({
  agent: worker,
  input: cdk.input.prompt`
    Implement the plan: ${slot.draft}.
    Your work now: ${slot.brief}.
    Your previous report on this stage, if any: ${slot.report.optional()}.
    What the stage's proof said about your last claim, if you made one: ${slot.proofResult.optional()}.
  `,
  output: cdk.output.prompt`
    Return your report on this dispatch: ${slot.report}
  `,
});

// The unreviewed lane (quick): no verdict and so no brief — the plan's
// first open stage is the objective, and the worker keeps its own ledger
// through its report.
export const implementPlan = cdk.defineStep.prompt({
  agent: worker,
  input: cdk.input.prompt`
    Implement the plan: ${slot.draft}, stage by stage in plan order; each stage's goal is its only requirement and its proof says what green means for it.
    Your previous report, if any: ${slot.report.optional()}.
  `,
  output: cdk.output.prompt`
    Return your report on this dispatch: ${slot.report}
  `,
});
