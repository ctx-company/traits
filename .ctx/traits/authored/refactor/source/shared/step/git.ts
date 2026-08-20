import * as cdk from "@ctx-traits/cdk";

import { scribe } from "../agent.ts";
import { port, slot } from "../data.ts";

export const status = cdk.defineStep.command({
  input: cdk.input.command`git status --porcelain`,
  output: slot.gitStatus,
});

export const commitStage = cdk.defineStep.command({
  input: cdk.input.command`git add -A`,
  output: slot.stageOutput,
});

export const commitSubmit = cdk.defineStep.command({
  input: cdk.input.command`git commit -m ${slot.commitMessage}`,
  output: slot.commitOutput,
});

// Shared by every variant's commit-message step, including the
// annotation-driven guided variant's own target-free copy.
export const commitMessageOutput = cdk.output.prompt`
    Return exactly the finished commit message into (${slot.commitMessage}).
`;

export const commitMessage = cdk.defineStep.prompt({
  agent: scribe,
  input: cdk.input.prompt`
    The refinement for the refactor of ${port.target} has ended and the work is being committed.
    Write the commit message from the plan ${slot.plan}.
    Final reviewer verdicts: ${slot.verdict1.optional()} & ${slot.verdict2.optional()}.
`,
  output: commitMessageOutput,
});
