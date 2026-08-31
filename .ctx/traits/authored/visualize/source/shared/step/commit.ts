import * as cdk from "@ctx-traits/cdk";

import { commitLog, slug } from "../data.ts";

export function commitStep(title: string): void {
  cdk.step.command(title, {
    id: "commit-visualization",
    argv: [
      "sh",
      "-c",
      'git add .internal/visualizations && { git commit -m "visualize: $1" -- .internal/visualizations 2>&1 || echo "nothing to commit"; }',
      "_",
      slug,
    ],
    output: commitLog,
  });
}
