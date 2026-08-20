import * as cdk from "@ctx-traits/cdk";

import { smart } from "../agent.ts";
import { slot, port } from "../data.ts";

export const primary = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    Review the working tree's implemented state for ${port.task} against the task file's own contract as source of truth.
    A BLOCKER is a correctness bug, an unmet item of the task's own Done when, clear over-build, or un-abstracted duplication.
    Say "not yet" as many rounds as it takes; do not approve to end the loop, and do not invent a blocker to extend it.
    Optionally attached verdicts from previous rounds: ${slot.verdict1.optional()} & ${slot.verdict2.optional()}
    Worker's summary: ${slot.workSummary}, Changed files ${slot.changedFiles}.
  `,
  output: cdk.output.prompt`
    Return the typed review verdict — status is revise while any blocker remains, approved when none do (${slot.verdict1})
  `,
});

export const secondary = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    Review the working tree's implemented state for ${port.task} against the task file's own contract as source of truth.
    A BLOCKER is a correctness bug, an unmet item of the task's own Done when, clear over-build, or un-abstracted duplication.
    Say "not yet" as many rounds as it takes; do not approve to end the loop, and do not invent a blocker to extend it.
    Optionally attached verdicts from previous rounds: ${slot.verdict1.optional()} & ${slot.verdict2.optional()}
    Worker's summary: ${slot.workSummary}, Changed files ${slot.changedFiles}.
  `,
  output: cdk.output.prompt`
    Return the typed review verdict — status is revise while any blocker remains, approved when none do (${slot.verdict2})
  `,
});
