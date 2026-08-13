import { input } from "@ctx-traits/cdk";
import type { AgentHandle } from "@ctx-traits/cdk";
import { grounding, taskFiles, taskInput } from "../data.ts";
import { TASK_FORMAT_DOCTRINE } from "../resource.ts";

export function tasks(agent: AgentHandle) {
  return agent.prompt("Split the work into task files", { id: "split", input: input.prompt(`Task format doctrine: ${TASK_FORMAT_DOCTRINE}
            The work, as described: {task}
            Grounding notes: {grounding}
            Break the refined work into small, ordered task files.
            Fold the grounding each task needs — its files, gates, invariants, constraints — into that task's own body and Watch section, so every file stands alone.
            Do not implement anything.`, { task: taskInput, grounding }), output: taskFiles });
}
