import * as cdk from "@ctx-traits/cdk";
import { input, slot } from "@ctx-traits/cdk";

import { scribe } from "../agent.ts";
import { changelogEntry } from "./changelog.ts";
import { commitOutput, newVersion } from "../data.ts";

export const commitMessage = slot.text({
  id: "commit-message",
  description: "Release commit message; injected directly into the git commit command step.",
});
export const stageOutput = slot.text({
  id: "stage-output",
  description: "Verbatim output of the git-add staging command.",
});
export const unstageOutput = slot.text({
  id: "unstage-output",
  description:
    "Verbatim output of the runtime-state unstage command (git reset -- .agents/runs); empty when nothing was staged there.",
});
export const tagOutput = slot.text({
  id: "tag-output",
  description: "Output evidence from the local annotated-tag command.",
});

const commitMessageText = input.prompt`
    The version-bump and changelog reviews have both approved. Write the release commit message: subject line "release: v${newVersion}", then the changelog entry ${changelogEntry} as the body.
    Return exactly that message. Do not run git commands and do not write files.`;

export const writeMessage = cdk.stage({
  agent: scribe,
  input: commitMessageText,
  output: commitMessage,
});

// Two steps, not one excluding pathspec — a pathspec that mentions a
// gitignored `.agents` exits 1 (run-42bd7fb2).
export function add(title: string) {
  return cdk.step.command(title, { argv: ["git", "add", "-A"], output: stageOutput });
}
export function unstage(title: string) {
  return cdk.step.command(title, {
    argv: ["git", "reset", "-q", "--", ".agents/runs"],
    output: unstageOutput,
  });
}
export function commitRelease(title: string) {
  return cdk.step.command(title, { argv: ["git", "commit", "-m", commitMessage], output: commitOutput });
}
// Local annotated tag only — still reversible, not gated.
export function tag(title: string) {
  return cdk.step.command(title, {
    input: input.command`sh -c 'git tag -a "v$1" -m "$2"' _ ${newVersion} ${commitMessage}`,
    output: tagOutput,
  });
}
