import type { AgentHandle } from "@ctx-traits/cdk";
import { input } from "@ctx-traits/cdk";

import { doneCriteria, grounding, nextKey, slicePlan, taskInput, workItems } from "../data.ts";

/**
 * Decompose the work into the typed slice plan — the index the per-slice
 * frames execute. Final board keys are assigned HERE, once, from the derived
 * next-free key: the plan is the single reviewable place where numbering
 * arithmetic happens, and every later step treats keys as fixed facts.
 */
export function slices(agent: AgentHandle) {
  return agent.prompt("Split the work into slices", {
    id: "split",
    input: input.prompt`
            Break the described work into dependency-ordered slices with child tasks, as the typed plan.
            The work, as described: ${taskInput}
            The source's work items: ${workItems}
            The source's done criteria: ${doneCriteria}
            Grounding notes: ${grounding}
            Assign final board keys now, starting at ${nextKey}: the first slice's charter takes that key, the next slice the following number, and so on; children are "<charter-key>.1", "<charter-key>.2", ... in dependency order. Keys are final — no placeholders.
            Every work item MUST appear in at least one slice's covers; a slice may own several related items. Size every child task to roughly 10-15 minutes of focused agent work; a slice with more than nine children is too big — split the slice. Order slices and children so each depends only on earlier keys, and record real dependencies explicitly in each task's depends-on.
            Do not implement anything and do not write any files yet — return only the plan.`,
    output: slicePlan,
  });
}
