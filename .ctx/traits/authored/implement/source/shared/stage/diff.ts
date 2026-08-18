import * as cdk from "@ctx-traits/cdk";

import { slot } from "../data.ts";

// sh strips the trailing newline via command substitution — bare
// `git rev-parse HEAD` output interpolates into the capture argv verbatim,
// and a "sha\n" revision argument fails every git diff (ctx-notify 0004,
// 2026-08-18).
export const baseline = cdk.stage({
  input: cdk.input.command`sh -c "printf %s \\"\\$(git rev-parse HEAD)\\""`,
  output: slot.diffBase,
});

export const capture = cdk.stage({
  input: cdk.input.command`git diff --name-status ${slot.diffBase}`,
  output: slot.changedFiles,
});
