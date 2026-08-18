import * as cdk from "@ctx-traits/cdk";

import { smart1 } from "../agent.ts";
import { plan, target } from "../data.ts";

export const compose = cdk.stage({
  agent: smart1,
  input: cdk.input.prompt`
    Turn ${target} into an actionable refactoring checklist.
    Read the target with your tools.
    List concrete steps only — no framing prose, no illustrative design.
    Keep it to what honestly fits one quick run.
`,
  output: cdk.output.prompt`
    Return the checklist (${plan}).
`,
});
