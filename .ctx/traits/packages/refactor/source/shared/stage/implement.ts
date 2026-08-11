import * as cdk from "@ctx-traits/cdk";

import { worker } from "../agent.ts";
import { plan, reviewVerdict1, reviewVerdict2, workDeviationReport, workSummary } from "../data.ts";

export const implementStage = cdk.stage({
    agent: worker,
    input: cdk.input.prompt`
    Implement the plan ${plan}.
    Execute exactly as specified, with no departure from what is written.
    Fix reviewer verdicts if present: ${reviewVerdict1.optional()} and ${reviewVerdict2.optional()}.
    When no verdicts are present, then implement the plan in full.
`,
    output: cdk.output.prompt`
    Return exactly two outputs:
    - the work summary, replacing any prior one: what changed, files, gate results, open concerns (${workSummary})
    - the complete deviation report: every rejected-but-considered proposal, including any new ones this round (${workDeviationReport})
`,
});
