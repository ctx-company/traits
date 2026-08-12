import * as cdk from "@ctx-traits/cdk";
import { schema, slot } from "@ctx-traits/cdk";

import { currentBranch } from "../data.ts";

export const gitStatus = slot.text({
  id: "git-status",
  description: "Working-tree status captured before any work begins: git status --porcelain output verbatim.",
  hint: "Non-empty means the tree is dirty; the run parks before any bump when this is non-empty.",
});
// The runtime's own `check`-step contract, not a bare boolean (P565): a
// check step's output slot must declare `ok` (schema:boolean) plus `argv`
// (schema:text list) — the argv makes the gate self-describing so there is
// no second source of truth for "what proves this done".
export const gatePassed = slot({
  id: "gate-passed",
  schema: schema.object("release-gate-result", {
    ok: schema.field(schema.boolean(), { description: "True when the gate command exited successfully." }),
    argv: schema.field(schema.list(schema.text()), { description: "The exact argv the gate ran." }),
  }),
  description:
    'Whether the repo-declared preflight gate (["just", "test"]) exited successfully, and the exact argv that decided it.',
});

// Dirty tree or a red gate simply lets the enclosing flow.when close over
// nothing further — the run parks by doing nothing more, the same
// "maybe-commit" idiom every other family uses.
export function status(title: string) {
  return cdk.step.command(title, { argv: ["git", "status", "--porcelain"], output: gitStatus });
}
export function branch(title: string) {
  return cdk.step.command(title, {
    argv: ["git", "rev-parse", "--abbrev-ref", "HEAD"],
    output: currentBranch,
  });
}
// Fixed argv, mirroring `[merge] gate = [["just", "test"]]`: the trait
// never composes a gate command from model output.
export function gate(title: string) {
  return cdk.step.check(title, { argv: ["just", "test"], output: gatePassed, timeoutMs: 600_000 });
}
