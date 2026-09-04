import * as cdk from "@ctx-traits/cdk";

import { worker } from "../agent.ts";
import { slot } from "../data.ts";

// The worker's inputs, in order of authority (0281.1): the plan is what to
// do; the verdict is the reviewer's measurement of the current state with
// the owner's annotations already applied to it (rulings, deferred and
// dropped steps, order); nothing else instructs the worker. The
// conventions — blockers in listed order, ruled steps as ruled, deferred
// and dropped steps untouched — live in the verdict schema's own field
// descriptions, not here.
export const implement = cdk.defineStep.prompt({
  agent: worker,
  input: cdk.input.prompt`
    Implement the plan: ${slot.draft}.
    The reviewer's verdict on the current state, with the owner's annotations applied (if any): ${slot.verdict1.optional()} & ${slot.verdict2.optional()}
    Don't discard your progress if you can't finish the task to completion.
  `,
  output: cdk.output.prompt`
    Return this round's work summary: ${slot.workSummary}
  `,
});
