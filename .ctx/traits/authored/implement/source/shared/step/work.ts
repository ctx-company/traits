import * as cdk from "@ctx-traits/cdk";

import { worker } from "../agent.ts";
import { slot } from "../data.ts";

// The worker walks one step at a time (0283). Each dispatch sees the plan, the
// ONE step the for-each bound, and its own previous return on that step — never
// the reviewer's list or the rest of the queue. It returns a typed status; the
// runtime advances on done/blocked and re-dispatches the SAME step on not-done.
// There is no proof and no stage-claim: the worker never certifies its own
// work — the reviewer validates the tree.
export function implement(title: string, step: cdk.SlotHandle): void {
  worker.prompt(title, {
    input: cdk.input.prompt`
      Implement the plan: ${slot.plan}.
      Your current step — do exactly this and nothing beyond it: ${step}.
      Your previous return on this step, if any: ${slot.report.optional()}.
      Return your status for THIS step: done only when you verified the step's done-when holds in the working tree with your own tools; blocked only for something you genuinely cannot resolve from inside this run — a contradiction between the plan and the code, a decision only the owner can make — never difficulty, size, or a red tree; not-done when you made real progress but the done-when does not yet hold, so the next dispatch continues the SAME step. When the step carries a resolution, that is the reviewer's decision and the approach to take — follow it. A red tree is the work, not a reason to stop or to claim done.
    `,
    output: slot.report,
  });
}

// The unreviewed lane (quick): no reviewer opens a step list, so the worker
// implements the plan directly, stage by stage, carrying its own progress
// through its report until it returns done or blocked.
export function implementPlan(title: string): void {
  worker.prompt(title, {
    input: cdk.input.prompt`
      Implement the plan: ${slot.plan}, stage by stage in plan order; each stage's goal is its only requirement. There is no reviewer — carry your own progress through your report and keep going until the plan's goals hold.
      Your previous return, if any: ${slot.report.optional()}.
      Return your status: done when every stage's goal holds in the tree, verified with your own tools; blocked for something only the owner can settle; not-done when more remains and the next dispatch should continue.
    `,
    output: slot.report,
  });
}
