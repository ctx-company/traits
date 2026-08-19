import * as cdk from "@ctx-traits/cdk";
import { port, schema, slot } from "@ctx-traits/cdk";

export const treeStatus = slot({
  id: "tree-status",
  schema: schema.text(),
  description: "The porcelain listing of uncommitted changes; empty when the reviewed tree stayed clean.",
});

export const treeStatusReport = port.output.of(schema.text(), {
  id: "tree-status-report",
  description: "Uncommitted changes left in the reviewed tree, empty when none.",
  optional: true,
  value: treeStatus,
});

export function assert(title: string) {
  return cdk.step.command(title, {
    input: cdk.input.command`git status --porcelain`,
    output: treeStatus,
  });
}
