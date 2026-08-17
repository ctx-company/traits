// The one worker stage, in three include-shapes: quick's single pass takes
// no verdicts; default's loop attaches the first reviewer's verdict;
// complex's loop attaches both. One prompt, so the contract cannot drift
// between variants.
import * as cdk from "@ctx-traits/cdk";

import { worker } from "../agent.ts";
import { TASK_WRITE_SCOPE_RIDER, task, verdict1, verdict2, workSummary } from "../data.ts";

const PROMPT = cdk.input.prompt(
  `Implement {task}.
    The task's file lives in .internal/tasks/ (matched by key, name, or filename) — read it with your tools; its content, scope, and validation sections are the contract. Honor Watch, and treat Done when as the definition of done: run the checks it names before reporting.
    No reviewer verdict attached means this is round 1: implement the task in full. On every later round a verdict IS attached and redefines your work: fix its blockers, largest first. Your own work summary from the previous round is your memory of what prior rounds did — extend it, never restart it.
    ${TASK_WRITE_SCOPE_RIDER}
    Return the cumulative work summary: what changed (files), how it was validated, open concerns.`,
  { task },
);

export const apply = cdk.stage({
  agent: worker,
  input: PROMPT,
  output: workSummary,
});

export const applyReviewed = cdk.stage({
  agent: worker,
  input: PROMPT,
  output: workSummary,
  include: [verdict1.optional(), workSummary.optional()],
});

export const applyDualReviewed = cdk.stage({
  agent: worker,
  input: PROMPT,
  output: workSummary,
  include: [verdict1.optional(), verdict2.optional(), workSummary.optional()],
});
