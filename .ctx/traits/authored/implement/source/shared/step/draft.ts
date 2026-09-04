import * as cdk from "@ctx-traits/cdk";

import { smart } from "../agent.ts";
import { annotations, draft, planAnswer, task } from "../data.ts";

// The plan's shape (scope, stages, approach, risks) and the stage doctrine
// live in the draft slot's schema descriptions, not here (0281.2). The
// owner's plan-gate corrections (0281.3), when present, are binding input
// to the redraft; the prior draft itself is not re-read — the corrections
// carry the exact plan text they were made on.
export const compose = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    Draft the implementation plan for ${task}, read it and the relevant code with your tools.
    The owner's corrections to the previous draft, when present, are binding: ${planAnswer.optional()}
`,
  output: draft,
});

// The tree lane: the owner's annotations are the task, so the plan is
// drafted from them and the annotated code, not from a task file.
export const composeFromAnnotations = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    Draft the implementation plan for the owner's annotations: ${annotations}.
    Each names a file, its lines, the exact annotated text, and the owner's note; read the annotated code and its surroundings with your tools. The annotations are the task: the plan's goals are cut from them, one observable outcome per thing the owner asked for.
`,
  output: draft,
});
