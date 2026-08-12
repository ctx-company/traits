import { INTEGRITY_DOCTRINE } from "@ctx-traits/agents";
import * as cdk from "@ctx-traits/cdk";

import { smart1 } from "../agent.ts";
import { plan, reviewVerdict1, workSummary } from "../data.ts";
import { architectureDialect } from "../resource.ts";

export const judge = cdk.stage({
  agent: smart1,
  input: cdk.input.prompt(
    `Review the implemented state of the working tree against the plan {plan}.
    Current work summary: {workSummary}.
    Cite the smell id or the deep-module violation for every blocker. ${INTEGRITY_DOCTRINE}`,
    {
      plan,
      workSummary,
      phaseBrief: plan,
      productBrief: architectureDialect,
    },
  ),
  output: reviewVerdict1,
});
