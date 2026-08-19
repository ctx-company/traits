import * as cdk from "@ctx-traits/cdk";
import { input, slot } from "@ctx-traits/cdk";

import { range } from "../data.ts";

export const diffStat = slot.text({
  id: "diff-stat",
  description: "git diff --stat output for the range: file inventory only, never patch bodies.",
  hint: "Index pattern — the reviewer opens actual content with its own tools (git show, git diff <range> -- <file>).",
});
export const commitLog = slot.text({
  id: "commit-log",
  description: "git log --oneline output for the range.",
});

export function captureDiffStat(title: string) {
  return cdk.step.command(title, {
    input: input.command`git diff --stat ${range}`,
    output: diffStat,
  });
}
export function captureCommitLog(title: string) {
  return cdk.step.command(title, {
    input: input.command`git log --oneline ${range}`,
    output: commitLog,
  });
}
