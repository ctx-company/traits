import * as cdk from "@ctx-traits/cdk";

import { smart } from "../agent.ts";
import { plan, target } from "../data.ts";

export const compose = cdk.step({
  agent: smart,
  input: cdk.input.prompt`
    Turn ${target} into an actionable refactoring checklist.
  `,
  output: cdk.output.prompt`
    Return the checklist plan: ${plan}.
  `,
});
