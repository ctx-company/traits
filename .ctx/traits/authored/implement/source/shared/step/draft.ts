import * as cdk from "@ctx-traits/cdk";

import { smart } from "../agent.ts";
import { draft, task } from "../data.ts";

// The plan's shape (scope, stages, approach, risks) and the stage doctrine
// live in the draft slot's schema descriptions, not here (0281.2).
export const compose = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    Draft the implementation plan for ${task}, read it and the relevant code with your tools.
`,
  output: draft,
});
