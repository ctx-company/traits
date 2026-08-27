import { TASK_CHECK_DOCTRINE } from "@ctx-traits/agents";
import * as cdk from "@ctx-traits/cdk";

import { smart } from "../agent.ts";
import { criticVerdict, grounding, receipt, target, taskSnapshot } from "../data.ts";

export const inPlace = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt(
    `Rewrite only {target} in place from its draft plan to a ready execution plan. You may edit only target.file: do not implement, create, rename, or modify any other file. Preserve its filename, key, and raised date. Keep relations unchanged unless code reading proves a dependency is wrong; if changed, explain the proof in relations-correction. Change status from draft to ready and add no execution-plan header.
     Use grounding {grounding} and checksum context {task-snapshot}. The plan separates goals from approach, and must say so in its own text. GOALS are explicit and binding: each Done-when states an observable outcome with a runnable existing command that decides it, plus explicit exclusions. APPROACH is the suggested route, never a boundary: expected files, symbols and signatures, edit order, commands to run, and proof mechanics are the architect's best guidance — stated precisely where the grounding supports them — and the run may deviate from any of it whenever a goal requires it, reporting deviations in the work summary rather than treating them as violations or escalating them. Never pin internal proof mechanics (exact fixtures, matchers, or literal expected output) beyond what the grounding evidences as already occurring: state what must be proven and let the run choose how to observe it. Reuse points stay named. [[checks]] must follow this shared doctrine exactly:\n${TASK_CHECK_DOCTRINE}\nPrior critic verdict, when present: {critic-verdict}. Fix its blockers only.`,
    { target, grounding, "task-snapshot": taskSnapshot, "critic-verdict": criticVerdict.optional() },
  ),
  output: cdk.output.prompt`Return the typed receipt for this one rewrite, recording draft to ready and any permitted relations correction: ${receipt}`,
});
