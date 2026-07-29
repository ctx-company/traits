import { prompt, sequence } from "@ctx-traits/cdk";

import { agent } from "../agent.ts";
import { slot } from "../data.ts";

const commitMessage = sequence.prompt("commit-message", {
    title: "Write the commit message (scribe)",
    agent: agent.scribe,
    text: prompt.text`
                    The build loop for the annotated assignment ${slot.annotations} has ended in an approved review, and the work is being committed. The final verdict is ${slot.verdict}, and ${slot.hunkNotes} is the walkthrough of what actually changed.
                    Write a concise commit message: a short subject line naming the change, then a one-paragraph summary of what was implemented and how it was validated. Draw its substance from the walkthrough's stated intents rather than re-deriving the change from the working tree.
                    Save exactly that message to the file .git/CTX_COMMITMSG at the repo root (create or overwrite it), and return the same message as your output.
                    Do not run any git commands; staging, committing and pushing happen in later runtime steps.`,
    output: slot.commitMessage,
});

const stage = sequence.command("git-stage", {
    title: "Stage all changes except runtime state",
    argv: ["git", "add", "-A", "--", ":(exclude).agents/runs"],
    output: slot.stageOutput,
});

const commit = sequence.command("git-commit", {
    title: "Commit the work",
    argv: ["git", "commit", "-F", ".git/CTX_COMMITMSG"],
    input: [slot.commitMessage],
    output: slot.commitOutput,
});

const push = sequence.command("gated-push", {
    title: "Push through the gate (ctx-gate)",
    argv: ["ctx-gate", "run", "--", "git", "push"],
    input: [slot.commitOutput],
    output: slot.pushOutput,
});

export default [commitMessage, stage, commit, push];
