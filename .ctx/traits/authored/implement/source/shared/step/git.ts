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

// Commit-report dropped (owner ruling 2026-09-02): the step still needs
// one output slot (command items must), so it captures into a throwaway
// commitLog that gates nothing. Tolerant of an already-clean tree — a
// prior seat may have committed — and always prints, so it never rejects
// on empty output or "nothing to commit".
export const commitSubmit = cdk.defineStep.command({
  input: cdk.input.command`sh -c 'if git diff --quiet && git diff --cached --quiet; then echo "nothing to commit"; else git commit -m "$1" && echo committed; fi' _ ${slot.commitMessage}`,
  output: slot.commitLog,
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

// The tree lane has no task port: the owner's annotations are the task.
export const commitMessageAnnotated = cdk.defineStep.prompt({
  agent: scribe,
  input: cdk.input.prompt`
    The work for the owner's tree annotations is being committed: ${slot.annotations}.
    Write a concise commit message from the work summary into ${slot.workSummary}.
  `,
  output: cdk.output.prompt`
    Return exactly the finished commit message into (${slot.commitMessage}).
  `,
});
