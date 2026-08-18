import * as cdk from "@ctx-traits/cdk";

import { smart1 } from "../agent.ts";
import { refactorFrame, survey, target } from "../data.ts";

export const select = cdk.stage({
  agent: smart1,
  input: cdk.input.prompt`
    From the survey ${survey} of ${target}.
    Verify the selection against the actual code with your tools.
`,
  output: cdk.output.prompt`
    Return the problem framing (${refactorFrame}).
`,
});
