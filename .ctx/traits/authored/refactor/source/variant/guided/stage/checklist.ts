import * as cdk from "@ctx-traits/cdk";

import { smart1 } from "#trait/shared/agent.ts";
import { plan } from "#trait/shared/data.ts";

import * as data from "../data.ts";

export const compose = cdk.stage({
  agent: smart1,
  input: cdk.input.prompt`
    Turn the captured annotations ${data.slot.annotations} into a checklist.
`,
  output: cdk.output.prompt`
    Return the checklist (${plan}).
`,
});
