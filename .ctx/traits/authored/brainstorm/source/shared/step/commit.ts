import * as cdk from "@ctx-traits/cdk";

import { commitLog, slug } from "../data.ts";

export function commitStep(title: string): void {
  cdk.step.command(title, {
    id: "commit-brainstorm",
    argv: [
      "sh",
      "-c",
      'git add .internal/brainstorms && { git commit -m "brainstorm: $1" -- .internal/brainstorms 2>&1 || echo "nothing to commit"; }',
      "_",
      slug,
    ],
    output: commitLog,
  });
}
