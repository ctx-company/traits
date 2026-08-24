import { TASK_CHECK_DOCTRINE } from "@ctx-traits/agents";
import * as cdk from "@ctx-traits/cdk";

import { smart } from "../agent.ts";
import { criticVerdict, grounding, receipt, target, taskSnapshot } from "../data.ts";

export const inPlace = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt(
    `Rewrite only {target} in place from its draft plan to a ready execution plan. You may edit only target.file: do not implement, create, rename, or modify any other file. Preserve its filename, key, and raised date. Keep relations unchanged unless code reading proves a dependency is wrong; if changed, explain the proof in relations-correction. Change status from draft to ready and add no execution-plan header.
     Use grounding {grounding} and checksum context {task-snapshot}. The content, scope, and validation must name every touched repo-relative file; every edited or new symbol with its exact signature; every removed symbol; strict edit order; reuse points; explicit exclusions; and a runnable existing command for every Done-when. [[checks]] must follow this shared doctrine exactly:\n${TASK_CHECK_DOCTRINE}\nPrior critic verdict, when present: {critic-verdict}. Fix its blockers only.`,
    { target, grounding, "task-snapshot": taskSnapshot, "critic-verdict": criticVerdict.optional() },
  ),
  output: cdk.output.prompt`Return the typed receipt for this one rewrite, recording draft to ready and any permitted relations correction: ${receipt}`,
});
