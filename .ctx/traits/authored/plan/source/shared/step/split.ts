import type { AgentHandle } from "@ctx-traits/cdk";
import { condition, flow, input, signal } from "@ctx-traits/cdk";

import { doneCriteria, durationTarget, grounding, slicePlan, taskInput, workItems } from "../data.ts";

/**
 * How many slices the plan may hold — the deterministic gate below AND the
 * write step's for-each `limit` bound both read this one constant, so
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
 * frames execute. Keys are SYMBOLIC (KEY1, KEY1.2, ...): the model never
 * does numbering arithmetic; the renumber step assigns real board numbers
 * from the live board once the plan is done. A slice with an empty tasks
 * list is one standalone bare task — shape follows the work, a charter
 * exists only when a genuine umbrella does. Gated like research's stream
 * planning: an out-of-range slice count replans (bounded), it never
 * reaches the write phase.
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
            Describe every slice and child as a dependency-ordered outcome at module/area and validation-gate scope. Include applicable constraints from the grounding notes, but defer exact symbols, signatures, edits, and edit sequencing to architect.
            SHAPE FOLLOWS THE WORK — three equal shapes, none preferred: a slice with an EMPTY tasks list is one standalone bare task on the board; several such slices are several independent peer tasks; a slice with children becomes a charter. A charter is legal only when a genuine umbrella exists — a shared stop condition no single child owns. Never split into children as packaging: one coherent piece of work is ONE task.
            Keys are symbolic: the first slice is "KEY1", the next "KEY2", and so on; children are "KEY1.1", "KEY1.2", ... in dependency order (ordinals continue ".10", ".11", ... — never zero-padded). Never invent numeric board keys — a final mechanical step assigns real numbers from the live board.
            Plan at most eight slices — consolidate related work items into one slice rather than exceeding the cap. Every work item MUST appear in at least one slice's covers; a slice may own several related items. Size every task SYMMETRICALLY to roughly ${durationTarget} of focused agent work: split anything materially larger, merge anything materially smaller into a sibling, and never plan a task whose whole body is running gates, tests, or checks — gates ride the [[checks]] of the task that changed the code. Before returning, judge the implied total (task count times ${durationTarget}) against the described work: cover every work item with the FEWEST tasks that stay within the target. Order slices and children so each depends only on earlier keys, and record real dependencies explicitly in each task's depends-on.
            Do not implement anything, and write nothing to disk in this step — not task files, not notes; return only the plan. A later step writes the board from it; any file created here is out-of-plan and will not be adopted.`,
      output: slicePlan,
      include: [slicePlan.optional()],
    });

    loop.until(sliceCountValid);
  });
}
