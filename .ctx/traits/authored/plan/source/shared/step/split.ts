import type { AgentHandle } from "@ctx-traits/cdk";
import { condition, flow, input, signal } from "@ctx-traits/cdk";

import { doneCriteria, grounding, slicePlan, taskInput, workItems } from "../data.ts";

/**
 * How many slices the plan may hold — the deterministic gate below AND the
 * write step's for-each `limit` bound both read this one constant, so
 * the two can never drift: an over-long plan is caught here as a cheap
 * replanning round, never downstream as a hard `max-items-exceeded` run
 * failure (which is exactly how the first two MVP-planning runs died: split
 * produced 8 slices against a for-each bound of 6, with no gate between).
 */
export const MAX_SLICES = 1;

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
            Capture the described work as EXACTLY ONE slice with an EMPTY tasks list — one parent task (owner ruling 2026-09-01: plan delivers a single parent; decomposition belongs to architect, which splits by its own judgment after grounding in code).
            The work, as described: ${taskInput}
            The source's work items: ${workItems}
            The source's done criteria: ${doneCriteria}
            Grounding notes: ${grounding}
            Describe every slice and child as a dependency-ordered outcome at module/area and validation-gate scope. Include applicable constraints from the grounding notes, but defer exact symbols, signatures, edits, and edit sequencing to architect.
            The one slice is the whole described work: its covers list every work item, its intent carries the shared stop condition, and its tasks list stays EMPTY — never pre-decompose, never emit children, never a second slice. The parent states outcome and acceptance shape at module/area and validation-gate scope; architect later judges whether it becomes a charter with children or stays the implementable unit.
            The slice's key is symbolic: "KEY1". Never invent numeric board keys — a final mechanical step assigns the real number from the live board.
            Record real dependencies on EXISTING board keys in depends-on. Never trim scope to a time budget: the parent may be larger than one run — architect owns decomposition and making it swift.
            Do not implement anything, and write nothing to disk in this step — not task files, not notes; return only the plan. A later step writes the board from it; any file created here is out-of-plan and will not be adopted.`,
      output: slicePlan,
      include: [slicePlan.optional()],
    });

    loop.until(sliceCountValid);
  });
}
