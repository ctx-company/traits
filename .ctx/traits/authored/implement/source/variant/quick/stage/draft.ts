import * as cdk from "@ctx-traits/cdk";

import { smart } from "../agent.ts";
import { draft, task } from "../data.ts";
import { taskBoard } from "../resource.ts";

export const compose = cdk.stage({
  agent: smart,
  input: cdk.input.prompt`
        Create an implementation draft for ${task} from its file on the task board ${taskBoard}. Task files are named NNNN-kebab-slug.md; the requested task names its file by number, full name, or filename — read that file with your tools. It is the sole binding authority for this run.
        Cover: scope, files to touch, approach, validation plan, risks.
        Reference files by path. Do not implement anything.`,
  output: draft,
});
