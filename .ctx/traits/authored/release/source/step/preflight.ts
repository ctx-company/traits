import * as cdk from "@ctx-traits/cdk";

import { currentBranch } from "../data.ts";

export const gitStatus = cdk.slot.text({
  id: "git-status",
  description: "Working-tree status captured before any work begins: git status --porcelain output verbatim.",
  hint: "Non-empty means the tree is dirty; the run parks before any bump when this is non-empty.",
});

export const gatePassed = cdk.slot.of({
  id: "gate-passed",
  schema: cdk.schema.object("release-gate-result", {
    ok: cdk.schema.field(cdk.schema.boolean(), { description: "True when the gate command exited successfully." }),
    argv: cdk.schema.field(cdk.schema.list(cdk.schema.text()), { description: "The exact argv the gate ran." }),
  }),
  description:
    'Whether the repo-declared preflight gate (["just", "test"]) exited successfully, and the exact argv that decided it.',
});

export function status(title: string) {
  return cdk.step.command(title, { argv: ["git", "status", "--porcelain"], output: gitStatus });
}

export function branch(title: string) {
  return cdk.step.command(title, {
    argv: ["git", "rev-parse", "--abbrev-ref", "HEAD"],
    output: currentBranch,
  });
}

export function gate(title: string) {
  return cdk.step.check(title, { argv: ["just", "test"], output: gatePassed, timeoutMs: 600_000 });
}
