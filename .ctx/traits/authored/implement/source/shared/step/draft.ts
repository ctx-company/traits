import * as cdk from "@ctx-traits/cdk";

import { smart } from "../agent.ts";
import { draft, task } from "../data.ts";

export const compose = cdk.step({
  agent: smart,
  input: cdk.input.prompt`
    Draft the implementation plan for ${task}, read it and the relevant code with your tools.
    Scope, files to touch, approach, reuse opportunities, validation plan, risks.
`,
  output: draft,
});
