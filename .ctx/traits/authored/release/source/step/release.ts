import * as cdk from "@ctx-traits/cdk";

import { pushEvidence, publishOutput } from "../data.ts";

export function push(title: string) {
  return cdk.step.command(title, {
    argv: ["ctx-gate", "run", "--", "git", "push", "--follow-tags"],
    output: pushEvidence,
    timeoutMs: 28_800_000,
  });
}

export function publish(title: string) {
  return cdk.step.command(title, {
    argv: ["ctx-gate", "run", "--", "just", "release-publish"],
    output: publishOutput,
    timeoutMs: 28_800_000,
  });
}
