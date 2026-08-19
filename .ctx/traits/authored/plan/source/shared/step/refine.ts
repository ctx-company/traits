import { input } from "@ctx-traits/cdk";
import type { AgentHandle } from "@ctx-traits/cdk";
import { grounding, taskInput } from "../data.ts";

export function task(agent: AgentHandle) {
  return agent.prompt("Ground & refine the task in the codebase", { id: "refine-task", input: input.prompt`
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
            Do not implement anything, and write nothing to disk in this step — not task files, not notes. A later step writes the board from the reviewed plan; any file created here is out-of-plan and will not be adopted.`, output: grounding });
}
