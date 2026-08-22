import * as cdk from "@ctx-traits/cdk";

import { worker } from "../agent.ts";
import { slot } from "../data.ts";

export const implement = cdk.defineStep.prompt({
  agent: worker,
  input: cdk.input.prompt`
    Implement the draft ${slot.draft}, which carries the task's restated scope and its validation plan.
    Treat that validation plan as the definition of done: run the checks it names before reporting.
    Optionally attached verdicts from previous rounds: ${slot.verdict1.optional()} & ${slot.verdict2.optional()}
    Attached verdicts are the source of truth — always follow their guidance.
  `,
  output: cdk.output.prompt`
    Return this round's work summary (what changed, how it was validated & open concerns): ${slot.workSummary}
  `,
});
