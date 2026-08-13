import * as cdk from "@ctx-traits/cdk";
import { input, slot } from "@ctx-traits/cdk";

import { rangeResolved } from "./resolve-range.ts";

export const diffStat = slot.text({
  id: "diff-stat",
  description: "git diff --stat output for the resolved range: file inventory only, never patch bodies.",
  hint: "Index pattern — the reviewer opens actual content with its own tools (git show, git diff <range> -- <file>).",
});
export const commitLog = slot.text({
  id: "commit-log",
  description: "git log --oneline output for the resolved range.",
});

export function captureDiffStat(title: string) {
  return cdk.step.command(title, {
    input: input.command`sh -c 'git diff --stat "$1"' _ ${rangeResolved}`,
    output: diffStat,
  });
}
export function captureCommitLog(title: string) {
  return cdk.step.command(title, {
    input: input.command`sh -c 'git log --oneline "$1"' _ ${rangeResolved}`,
    output: commitLog,
  });
}
