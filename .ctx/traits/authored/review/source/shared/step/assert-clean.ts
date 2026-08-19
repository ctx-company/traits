import * as cdk from "@ctx-traits/cdk";
import { port, schema, slot } from "@ctx-traits/cdk";

// The runtime's own `check`-step contract (P565): a check step's output
// slot must declare `ok` (schema:boolean) plus `argv` (schema:text list).
const treeStatusSchema = schema.object("review-tree-status", {
  ok: schema.field(schema.boolean(), {
    description: "True when the reviewed tree has no uncommitted changes after the review step ran.",
  }),
  argv: schema.field(schema.list(schema.text()), { description: "The exact argv that decided it." }),
});
export const treeStatus = slot({
  id: "tree-status",
  schema: treeStatusSchema,
  description: "Detects (does not prevent) whether the review step mutated the reviewed tree.",
});

export const treeStatusReport = port.output.of(treeStatusSchema, {
  id: "tree-status-report",
  description:
    "Whether the reviewed tree came out clean. Absent when preflight parked on an unresolvable range or an empty diff.",
  optional: true,
  value: treeStatus,
});

// Detection, not prevention: the harness confinement layer deliberately
// allows writes inside the run worktree, so a true deny-all-writes mode is
// runtime work, filed as a leftover rather than built here
// (ship-pragmatic/park-rigorous).
export function assert(title: string) {
  return cdk.step.check(title, {
    argv: ["sh", "-c", 'test -z "$(git status --porcelain)"'],
    output: treeStatus,
  });
}
