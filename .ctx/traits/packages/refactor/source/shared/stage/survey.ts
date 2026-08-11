import * as cdk from "@ctx-traits/cdk";

import { smart1 } from "../agent.ts";
import { survey, target } from "../data.ts";

export const surveyStage = cdk.stage({
    agent: smart1,
    input: cdk.input.prompt`
    Survey ${target} for refactoring opportunities.
    Read the target and its callers/callees with your tools — explore organically.
`,
    output: cdk.output.prompt`
    Return a numbered candidate list — for each:
        - the cluster of code involved (paths),
        - why it is coupled,
        - which smell ids or dialect pillar it violates,
        - how the boundary would change what tests can prove.
    List every real candidate even if it cannot fit one run in ${survey}.
`,
});
