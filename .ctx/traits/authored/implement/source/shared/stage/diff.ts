import * as cdk from "@ctx-traits/cdk";

import { changedFiles } from "../data.ts";

export const capture = cdk.stage({
  input: cdk.input.command`git diff --name-status HEAD`,
  output: changedFiles,
});
