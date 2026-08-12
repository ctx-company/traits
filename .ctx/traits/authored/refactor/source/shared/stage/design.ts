import * as cdk from "@ctx-traits/cdk";

import { smart1 } from "../agent.ts";
import { plan, refactorFrame, target } from "../data.ts";

export const draft = cdk.stage({
  agent: smart1,
  input: cdk.input.prompt`
    Design the refactor for the framed problem ${refactorFrame} on ${target}.
    Stay within the framing's constraints.
`,
  output: cdk.output.prompt`
    Return the complete agreed design ready to implement from (${plan}).
    Do not leave fidelity to be inferred from prose elsewhere in the design.
`,
});
