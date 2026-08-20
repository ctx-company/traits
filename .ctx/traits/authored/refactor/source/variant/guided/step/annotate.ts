import * as cdk from "@ctx-traits/cdk";

import * as data from "../data.ts";

export const collect = cdk.defineStep.command({
  input: cdk.input.command`ctx-annotate`,
  timeoutMs: 3_600_000,
  output: data.slot.annotations,
});
