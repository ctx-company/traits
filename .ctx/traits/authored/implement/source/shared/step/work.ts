import * as cdk from "@ctx-traits/cdk";

import { worker } from "../agent.ts";
import { slot } from "../data.ts";

export const implement = cdk.defineStep.prompt({
  agent: worker,
  input: cdk.input.prompt`
    Implement the draft task's drafted plan: ${slot.draft}.
    Verdicts from previous rounds (if available): ${slot.verdict1.optional()} & ${slot.verdict2.optional()}
    Attached verdicts are the source of truth - work on exactly what they request, to completion.
  `,
  output: cdk.output.prompt`
    Return this round's work summary: ${slot.workSummary}
  `,
});
