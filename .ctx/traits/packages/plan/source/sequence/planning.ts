import type { AgentHandle, PortHandle, SlotHandle } from "@ctx-traits/cdk";
import { prompt, sequence } from "@ctx-traits/cdk";
import { PLAN_FORMAT_DOCTRINE } from "../resources/plan-format";

export function refineTaskStep(agent: AgentHandle, taskInput: PortHandle, productContract: SlotHandle) {
    return sequence.prompt("refine-task", {
        title: "Ground & refine the task into product contract",
        agent,
        text: prompt.text`
            Task is described as: ${taskInput}, ground it in the codebase & refine it.
            Read the relevant parts of the repository with your tools:
                - identify the concrete files, modules, and patterns the task touches
                - the build/test/lint commands (the repo's validation gates, with exact invocations)
                - architectural invariants and requirements
                - dependencies or rules that need to be respected
            Produce product contract:
                - short project/context overview,
                - house rules and invariants an implementer and reviewer must honor,
                - validation gates (exact commands)
                - constraints specific to this task.
            This is the rules-and-context document later steps hold the work to, not the step-by-step plan.
            Do not implement anything.`,
        input: taskInput,
        output: productContract,
    });
}

export function splitPlanStep(agent: AgentHandle, taskInput: PortHandle, productContract: SlotHandle, executionPlan: SlotHandle) {
    return sequence.prompt("split", {
        title: "Split the task into implementation phases",
        agent,
        text: prompt.text`
            Plan format doctrine: ${PLAN_FORMAT_DOCTRINE}
            Product contract: ${productContract}
            Break the refined task into small, ordered implementation phases.
            Ground each phase in the codebase specifics from the contract.
            Do not implement anything.`,
        input: [taskInput, productContract],
        output: executionPlan,
    });
}
