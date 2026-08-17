import * as cdk from "@ctx-traits/cdk";

import { smart1, smart2 } from "../agent.ts";
import { TASK_WRITE_SCOPE_RIDER, changedFiles, task, verdict1, verdict2, workSummary } from "../data.ts";

const REVIEW_BODY = `A BLOCKER is a correctness bug, an unmet item of the task's own Done when, clear over-build, or un-abstracted duplication. Everything else is advisory and never blocks.
    Your own verdict from last round is attached when one exists: carry every open blocker forward verbatim, verify each with your own tools, and flip to done only on confirmed evidence.
    Set status to revise while any blocker remains, approved when none do. Say "not yet" as many rounds as it takes; do not approve to end the loop, and do not invent a blocker to extend it.`;

export const first = cdk.stage({
  agent: smart1,
  input: cdk.input.prompt(
    `Review the working tree's implemented state for {task} against the task file's own contract — read the task file and the changed code with your tools. Work summary: {work-summary}. {changed-files} names every file changed this round (an index, not the change; it omits untracked files).
    ${REVIEW_BODY}
    ${TASK_WRITE_SCOPE_RIDER}`,
    { task, "work-summary": workSummary, "changed-files": changedFiles },
  ),
  output: verdict1,
  include: [verdict1.optional()],
});

export const second = cdk.stage({
  agent: smart2,
  input: cdk.input.prompt(
    `Independently review the working tree's implemented state for {task} against the task file's own contract. Do not trust the work summary {work-summary} or the first reviewer's verdict — read the task file and inspect the changed code yourself ({changed-files} is the index of this round's changes; it omits untracked files).
    ${REVIEW_BODY}
    ${TASK_WRITE_SCOPE_RIDER}`,
    { task, "work-summary": workSummary, "changed-files": changedFiles },
  ),
  output: verdict2,
  include: [verdict2.optional()],
});
