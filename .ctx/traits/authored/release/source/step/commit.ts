import * as cdk from "@ctx-traits/cdk";

import { scribe } from "../agent.ts";
import { entry } from "./changelog.ts";
import { commitOutput, newVersion } from "../data.ts";

export const message = cdk.slot.text({
  id: "commit-message",
  description: "Release commit message; injected directly into the git commit command step.",
});

export const stageOutput = cdk.slot.text({
  id: "stage-output",
  description: "Verbatim output of the git-add staging command.",
});

export const unstageOutput = cdk.slot.text({
  id: "unstage-output",
  description:
    "Verbatim output of the runtime-state unstage command (git reset -- .agents/runs); empty when nothing was staged there.",
});

export const tagOutput = cdk.slot.text({
  id: "tag-output",
  description: "Output evidence from the local annotated-tag command.",
});

export const writeMessage = cdk.step({
  agent: scribe,
  input: cdk.input.prompt`
    The version-bump and changelog reviews have both approved.
    Write the release commit message: subject line "release: v${newVersion}", then the changelog entry ${entry} as the body.`,
  output: message,
});

export function add(title: string) {
  return cdk.step.command(title, { argv: ["git", "add", "-A"], output: stageOutput });
}

export function unstage(title: string) {
  return cdk.step.command(title, {
    argv: ["git", "reset", "-q", "--", ".agents/runs"],
    output: unstageOutput,
  });
}

export function release(title: string) {
  return cdk.step.command(title, { argv: ["git", "commit", "-m", message], output: commitOutput });
}

export function tag(title: string) {
  return cdk.step.command(title, {
    input: cdk.input.command`sh -c 'git tag -a "v$1" -m "$2"' _ ${newVersion} ${message}`,
    output: tagOutput,
  });
}
