import * as cdk from "@ctx-traits/cdk";

import { publishOutput } from "../data.ts";

// Irreversible edge, owner-gated (0098): fixed argv, the trait never
// composes shell from model output.
export function publish(title: string) {
  return cdk.step.command(title, {
    argv: ["ctx-gate", "run", "--", "just", "release-publish"],
    output: publishOutput,
    timeoutMs: 28_800_000,
  });
}
