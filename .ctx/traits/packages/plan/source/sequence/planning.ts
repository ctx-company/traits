import type { AgentHandle, PortHandle, SlotHandle } from "@ctx-traits/cdk";
import { prompt, sequence } from "@ctx-traits/cdk";
import { TASK_FORMAT_DOCTRINE } from "../resources/task-format.ts";

export function refineTaskStep(agent: AgentHandle, taskInput: PortHandle, grounding: SlotHandle) {
    return sequence.prompt("refine-task", {
        title: "Ground & refine the task in the codebase",
        agent,
        text: prompt.text`
            Task is described as: ${taskInput}, ground it in the codebase & refine it.
            Read the relevant parts of the repository with your tools:
                - identify the concrete files, modules, and patterns the task touches
                - the build/test/lint commands (the repo's validation gates, with exact invocations)
                - architectural invariants and requirements
                - dependencies or rules that need to be respected
            Produce the grounding notes:
                - short project/context overview,
                - rules and invariants an implementer and reviewer must honor,
                - validation gates (exact commands)
                - constraints specific to this task.
            This is the context the task files carry, not the step-by-step plan.
            Do not implement anything.`,
        input: taskInput,
        output: grounding,
    });
}

export function splitTasksStep(agent: AgentHandle, taskInput: PortHandle, grounding: SlotHandle, taskFiles: SlotHandle) {
    return sequence.prompt("split", {
        title: "Split the work into task files",
        agent,
        text: prompt.template(
            `Task format doctrine: ${TASK_FORMAT_DOCTRINE}
            The work, as described: {task}
            Grounding notes: {grounding}
            Break the refined work into small, ordered task files.
            Fold the grounding each task needs — its files, gates, invariants, constraints — into that task's own body and Watch section, so every file stands alone.
            Do not implement anything.`,
            { task: taskInput, grounding },
        ),
        input: [taskInput, grounding],
        output: taskFiles,
    });
}
