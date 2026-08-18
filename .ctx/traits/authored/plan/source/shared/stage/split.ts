import type { AgentHandle } from "@ctx-traits/cdk";
import { condition, flow, input, signal } from "@ctx-traits/cdk";

import { doneCriteria, grounding, nextKey, slicePlan, taskInput, workItems } from "../data.ts";

/**
 * How many slices the plan may hold — the deterministic gate below AND the
 * write stage's for-each `maxItems` bound both read this one constant, so
 * the two can never drift: an over-long plan is caught here as a cheap
 * replanning round, never downstream as a hard `max-items-exceeded` run
 * failure (which is exactly how the first two MVP-planning runs died: split
 * produced 8 slices against a for-each bound of 6, with no gate between).
 */
export const MAX_SLICES = 8;

const sliceCountValid = condition.any(
  Array.from({ length: MAX_SLICES }, (_, i) => condition.count(slicePlan).equals(i + 1)),
);

/**
 * Decompose the work into the typed slice plan — the index the per-slice
 * frames execute. Final board keys are assigned HERE, once, from the derived
 * next-free key: the plan is the single reviewable place where numbering
 * arithmetic happens, and every later step treats keys as fixed facts.
 * Gated like research's stream planning: an out-of-range slice count
 * replans (bounded), it never reaches the write phase.
 */
export function slices(agent: AgentHandle) {
  flow.loop("Planning", (loop) => {
    loop.maxIterations(3, { onExhausted: signal.Abort });

    agent.prompt("Split the work into slices", {
      id: "split",
      input: input.prompt`
            Break the described work into dependency-ordered slices with child tasks, as the typed plan.
            The work, as described: ${taskInput}
            The source's work items: ${workItems}
            The source's done criteria: ${doneCriteria}
            Grounding notes: ${grounding}
            Assign final board keys now, starting at ${nextKey}: the first slice's charter takes that key, the next slice the following number, and so on; children are "<charter-key>.1", "<charter-key>.2", ... in dependency order. Keys are final — no placeholders.
            Plan at most eight slices — consolidate related work items into one slice rather than exceeding the cap. Every work item MUST appear in at least one slice's covers; a slice may own several related items. Size every child task to roughly 10-15 minutes of focused agent work; a slice with more than nine children is too big — split the slice. Order slices and children so each depends only on earlier keys, and record real dependencies explicitly in each task's depends-on.
            Do not implement anything, and write nothing to disk in this step — not task files, not notes; return only the plan. A later step writes the board from it; any file created here is out-of-plan and will not be adopted.`,
      output: slicePlan,
      include: [slicePlan.optional()],
    });

    flow.until(sliceCountValid);
  });
}
