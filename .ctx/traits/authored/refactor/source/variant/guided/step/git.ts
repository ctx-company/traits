import * as cdk from "@ctx-traits/cdk";

import { scribe } from "#trait/shared/agent.ts";
import { plan, verdict1 } from "#trait/shared/data.ts";
import { commitMessageOutput } from "#trait/shared/step/git.ts";

// Annotation-driven commit-message step: guided has no `target` input, so
// it cannot route through the target-based shared/step/git.ts commitMessage
// without widening the procedure's interface (0200 round-2 review blocker).
export const commitMessage = cdk.step({
  agent: scribe,
  input: cdk.input.prompt`
    The review of the implemented annotation checklist has ended and the work is being committed.
    Final reviewer verdict: ${verdict1}.
    Write the commit message from the checklist ${plan}.
`,
  output: commitMessageOutput,
});
