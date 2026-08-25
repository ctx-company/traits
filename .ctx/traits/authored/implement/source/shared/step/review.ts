import * as cdk from "@ctx-traits/cdk";

import { smart } from "../agent.ts";
import { slot } from "../data.ts";

export const primary = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    Review the implemented state against the draft ${slot.draft} as source of truth.
    Worker's summary: ${slot.workSummary}, Changed files ${slot.changedFiles}.
    Verdicts from previous rounds (if available): ${slot.verdict1.optional()} & ${slot.verdict2.optional()}
    Don't rush the approval, take as many rounds as it needs, while staying pragmatic and task-focused.
  `,
  output: cdk.output.prompt`
    Return the typed review verdict — status is revise while any blocker remains, approved when none do (${slot.verdict1})
  `,
});

export const secondary = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    Review the implemented state against the draft ${slot.draft} as source of truth.
    Worker's summary: ${slot.workSummary}, Changed files ${slot.changedFiles}.
    Verdicts from previous rounds (if available): ${slot.verdict1.optional()} & ${slot.verdict2.optional()}
    Don't rush the approval, take as many rounds as it needs, while staying pragmatic and task-focused.
  `,
  output: cdk.output.prompt`
    Return the typed review verdict — status is revise while any blocker remains, approved when none do (${slot.verdict2})
  `,
});
