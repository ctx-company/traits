import * as cdk from "@ctx-traits/cdk";

import { scribe } from "../../../shared/agent.ts";
import { plan, reviewVerdict1, reviewVerdict2, target } from "../../../shared/data.ts";
import { commitMessageOutput } from "../../../shared/stage/git.ts";

export * from "../../../shared/stage/git.ts";

export const commitMessage = cdk.stage({
    agent: scribe,
    input: cdk.input.prompt`
        The refinement loop for the refactor of ${target} has ended and the work is being committed.
        The final reviewer verdicts are ${reviewVerdict1} and ${reviewVerdict2}.
        Write the commit message from the agreed design ${plan}.
    `,
    output: commitMessageOutput,
});
