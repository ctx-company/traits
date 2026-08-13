import { input } from "@ctx-traits/cdk";
import type { AgentHandle } from "@ctx-traits/cdk";
import { taskFiles, taskInput } from "../data.ts";
import { TASK_FORMAT_DOCTRINE } from "../resource.ts";

export function tasks(agent: AgentHandle) {
  return agent.prompt("Distill into task files (smart-1)", { id: "distill", input: input.prompt(`Turn the task below directly into one or more house-format task files for .internal/tasks/ — skip deriving separate grounding notes.
            Task as described: {task}
            Read the relevant parts of the repository with your tools to ground each task file in this codebase's concrete files, modules, and validation gates.
            ${TASK_FORMAT_DOCTRINE}
            Do not implement anything.`, { task: taskInput }), output: taskFiles });
}
