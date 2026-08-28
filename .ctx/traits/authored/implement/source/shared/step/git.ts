import * as cdk from "@ctx-traits/cdk";

import { scribe } from "../agent.ts";
import { slot, port } from "../data.ts";

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
  output: port.commitReport,
});

export const commitMessage = cdk.defineStep.prompt({
  agent: scribe,
  input: cdk.input.prompt`
    The work for ${port.task} is being committed.
    Write a concise commit message from the work summary into ${slot.workSummary}.
    Owner rulings made during this run (if any): ${slot.ownerDecisions.optional()} — when one shaped the work, cite what the owner decided in the message body.
  `,
  output: cdk.output.prompt`
    Return exactly the finished commit message into (${slot.commitMessage}).
  `,
});
