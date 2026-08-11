import { INTEGRITY_DOCTRINE } from "@ctx-traits/agents";
import * as cdk from "@ctx-traits/cdk";

import { smart1, smart2 } from "../../../shared/agent.ts";
import { plan, reviewVerdict1, reviewVerdict2, target, workSummary } from "../../../shared/data.ts";
import { architectureDialect } from "../../../shared/resource.ts";

const input = cdk.input.prompt(
    `Review the refactored state of {target} against the agreed design {agreedDesign}, independently of the other reviewer.
    Current work summary: {workSummary}.
    Cite the smell id or the deep-module violation for every blocker. ${INTEGRITY_DOCTRINE}`,
    {
        target,
        agreedDesign: plan,
        workSummary,
        phaseBrief: plan,
        productBrief: architectureDialect,
    },
);

export const reviewFirstStage = cdk.stage({
    agent: smart1,
    input,
    output: reviewVerdict1,
});
export const reviewSecondStage = cdk.stage({
    agent: smart2,
    input,
    output: reviewVerdict2,
});
