import * as cdk from "@ctx-traits/cdk";

import { agent } from "../agent.ts";
import { draft, plan, planDecision, task } from "../data.ts";
import { taskBoard } from "../resource.ts";

export const draftStage = cdk.stage({
  agent: agent.smart,
  input: cdk.input.prompt`
        Create an implementation draft for ${task} from its file on the task board ${taskBoard}. Task files are named NNNN-kebab-slug.md; the requested task names its file by number, full name, or filename — read that file with your tools. It is the sole binding authority for this run.
        Cover: scope, files to touch, approach, validation plan, risks.
        Reference files by path. Do not implement anything. The owner will annotate this draft next — write it to be read cold.`,
  output: draft,
});

export const refine = cdk.stage({
  agent: agent.smart,
  input: cdk.input.prompt`
        Refine the draft ${draft} into the final plan, using plannotator's plan-mode verdict ${planDecision}.
        If hookSpecificOutput.decision.behavior is "approve", the refined plan may be the draft unchanged.
        If it is "deny", hookSpecificOutput.decision.message is the owner's annotation — every point it raises must be visibly and provably addressed in the refined plan (not merely acknowledged): change the affected section, or state explicitly why the draft already covers it.
        Reference files by path; do not inline documents. Do not implement anything yet.`,
  output: plan,
});
