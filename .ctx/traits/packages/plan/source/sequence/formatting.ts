import type { AgentHandle, SlotHandle } from "@ctx-traits/cdk";
import { input, sequence } from "@ctx-traits/cdk";
import { TASK_FORMAT_DOCTRINE } from "../resource.ts";

export function formatTaskStep(agent: AgentHandle, task: SlotHandle, taskFiles: SlotHandle) {
    return sequence.prompt("format", {
        title: "Format into a task file (smart-1)",
        agent,
        text: input.prompt(
            `Format the task below into exactly one well-formed task file for .internal/tasks/.
            Task as described: {task}
            Keep the task's own wording near-verbatim. Do NOT refine, invent, or add requirements the task does not state; do not read the codebase or ground it in anything beyond the task text itself.
            ${TASK_FORMAT_DOCTRINE}
            Do not implement anything.`,
            { task },
        ),
        output: taskFiles,
        input: [task],
    });
}

export function distillTasksStep(agent: AgentHandle, task: SlotHandle, taskFiles: SlotHandle) {
    return sequence.prompt("distill", {
        title: "Distill into task files (smart-1)",
        agent,
        text: input.prompt(
            `Turn the task below directly into one or more house-format task files for .internal/tasks/ — skip deriving separate grounding notes.
            Task as described: {task}
            Read the relevant parts of the repository with your tools to ground each task file in this codebase's concrete files, modules, and validation gates.
            ${TASK_FORMAT_DOCTRINE}
            Do not implement anything.`,
            { task },
        ),
        output: taskFiles,
        input: [task],
    });
}
