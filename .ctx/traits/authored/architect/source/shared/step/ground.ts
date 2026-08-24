import * as cdk from "@ctx-traits/cdk";

import { smart } from "../agent.ts";
import { grounding, target, task } from "../data.ts";

export const resolve = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    Read-only resolution: locate exactly one unambiguous TaskDocument TOML file under .internal/tasks/ for ${task}, accepting its key, name, or filename. Inspect filenames and TOML metadata with tools. Do not edit any file. Refuse ambiguity rather than selecting a candidate.
  `,
  output: cdk.output.prompt`Return the resolved immutable key and repo-relative file path: ${target}`,
});

export const inspect = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    Read-only grounding for ${target}: read that exact task, its parent and siblings where present, relevant code, existing commands, invariants, and reusable logic. Establish every concrete fact the execution plan needs. Do not edit any file.
  `,
  output: cdk.output.prompt`Return evidence-grounded context for the one rewrite: ${grounding}`,
});
