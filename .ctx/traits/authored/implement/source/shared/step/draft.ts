import * as cdk from "@ctx-traits/cdk";

import { smart } from "../agent.ts";
import { draft, planAnswer, task } from "../data.ts";

// The plan's shape (scope, stages, approach, risks) and the stage doctrine
// live in the draft slot's schema descriptions, not here (0281.2). The
// owner's plan-gate corrections (0281.3), when present, are binding input
// to the redraft; the prior draft itself is not re-read — the corrections
// carry the exact plan text they were made on.
export const compose = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    Draft the implementation plan for ${task}, read it and the relevant code with your tools.
    The owner's corrections to the previous draft, when present, are binding: ${planAnswer.optional()}
`,
  output: draft,
});
