import * as cdk from "@ctx-traits/cdk";

import { worker } from "../agent.ts";
import { slot, port } from "../data.ts";

export const implement = cdk.stage({
  agent: worker,
  input: cdk.input.prompt`
    Implement ${port.task} following the draft ${slot.draft} — the task's file lives in .internal/tasks/ (matched by key, name, or filename).
    Honor Watch, and treat Done when as the definition of done: run the checks it names before reporting.
    Optionally attached verdicts from previous rounds: ${slot.verdict1.optional()} & ${slot.verdict2.optional()}
    Optionally attached work summary from previous rounds: ${slot.workSummary.optional()}
  `,
  output: cdk.output.prompt`
    Return the cumulative work summary (what changed, how it was validated & open concerns): ${slot.workSummary}
  `,
});
