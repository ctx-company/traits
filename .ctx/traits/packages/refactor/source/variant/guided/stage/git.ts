import * as cdk from "@ctx-traits/cdk";

import { scribe } from "#trait/shared/agent.ts";
import { plan, reviewVerdict1 } from "#trait/shared/data.ts";
import { commitMessageOutput } from "#trait/shared/stage/git.ts";

export const commitMessage = cdk.stage({
    agent: scribe,
    input: cdk.input.prompt`
    The review of the implemented annotation checklist has ended and the work is being committed.
    The final reviewer verdict is ${reviewVerdict1}.
    Write the commit message from the checklist ${plan}.
`,
    output: commitMessageOutput,
});
