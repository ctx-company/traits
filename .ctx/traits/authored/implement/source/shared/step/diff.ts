import * as cdk from "@ctx-traits/cdk";

import { slot } from "../data.ts";

export const baseline = cdk.defineStep.command({
  input: cdk.input.command`git rev-parse HEAD`,
  output: slot.diffBase,
});

export const capture = cdk.defineStep.command({
  input: cdk.input.command`git diff --name-status ${slot.diffBase}`,
  output: slot.changedFiles,
});
