import * as cdk from "@ctx-traits/cdk";

import { scribe } from "../agent.ts";
import { port, slot } from "../data.ts";

export const status = cdk.stage({
  input: cdk.input.command`git status --porcelain`,
  output: slot.gitStatus,
});

export const commitStage = cdk.stage({
  input: cdk.input.command`git add -A`,
  output: slot.stageOutput,
});

export const commitSubmit = cdk.stage({
  input: cdk.input.command`git commit -m ${slot.commitMessage}`,
  output: slot.commitOutput,
});

// Shared by every variant's commit-message stage, including the
// annotation-driven guided variant's own target-free copy.
export const commitMessageOutput = cdk.output.prompt`
    Return exactly the finished commit message into (${slot.commitMessage}).
`;

export const commitMessage = cdk.stage({
  agent: scribe,
  input: cdk.input.prompt`
    The refinement for the refactor of ${port.target} has ended and the work is being committed.
    Write the commit message from the plan ${slot.plan}.
    Final reviewer verdicts: ${slot.verdict1.optional()} & ${slot.verdict2.optional()}.
`,
  output: commitMessageOutput,
});
