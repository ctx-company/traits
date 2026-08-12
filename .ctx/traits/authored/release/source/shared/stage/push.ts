import * as cdk from "@ctx-traits/cdk";
import { slot } from "@ctx-traits/cdk";

export const pushOutput = slot.text({
  id: "push-output",
  description:
    "Output evidence from the ctx-gate-guarded tag push. Absent when the owner denies or the gate times out.",
});

// Irreversible edge, owner-gated (0098): fixed argv, the trait never
// composes shell from model output.
export function push(title: string) {
  return cdk.step.command(title, {
    argv: ["ctx-gate", "run", "--", "git", "push", "--follow-tags"],
    output: pushOutput,
    timeoutMs: 28_800_000,
  });
}
