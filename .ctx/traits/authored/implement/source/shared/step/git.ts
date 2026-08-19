import * as cdk from "@ctx-traits/cdk";

import { scribe } from "../agent.ts";
import { slot, port } from "../data.ts";

export const status = cdk.step({
  input: cdk.input.command`git status --porcelain`,
  output: slot.gitStatus,
});

export const commitStage = cdk.step({
  input: cdk.input.command`git add -A`,
  output: slot.stageOutput,
});

export const commitSubmit = cdk.step({
  input: cdk.input.command`git commit -m ${slot.commitMessage}`,
  output: port.commitReport,
});

export const commitMessage = cdk.step({
  agent: scribe,
  input: cdk.input.prompt`
    The work for ${port.task} is being committed.
    Write a concise commit message from the work summary into ${slot.workSummary}.
  `,
  output: cdk.output.prompt`
    Return exactly the finished commit message into (${slot.commitMessage}).
  `,
});
