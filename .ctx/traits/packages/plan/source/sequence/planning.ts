import type { AgentHandle, PortHandle, SlotHandle } from "@ctx-traits/cdk";
import { prompt, sequence } from "@ctx-traits/cdk";
import { PLAN_FORMAT_DOCTRINE } from "./formatting.ts";

export function refineTaskStep(agent: AgentHandle, taskInput: PortHandle, productContext: SlotHandle) {
    return sequence.prompt("refine", {
        title: "Refine + derive the product contract (smart-1)",
        agent,
        text: prompt.text`
                    Refine the task below and ground it in THIS codebase, then derive the product contract for it.
                    Task as described: ${taskInput}
                    Read the relevant parts of the repository with your tools: identify the concrete files, modules, and patterns the task touches; the build/test/lint commands (the repo's validation gates, with exact invocations); architectural invariants; and any dependency, unsafe-code, or stability rules the work must respect.
                    Produce the PRODUCT CONTRACT — the content for .docs/PRODUCT.md: a short project/context overview, the house rules and invariants an implementer and reviewer must honor, the validation gates (exact commands), and the constraints specific to this task. This is the rules-and-context document later steps hold the work to; it is NOT the step-by-step plan. Do not implement anything.`,
        output: productContext,
        input: [taskInput],
    });
}

export function splitPlanStep(agent: AgentHandle, taskInput: PortHandle, productContext: SlotHandle, executionPlan: SlotHandle) {
    return sequence.prompt("split", {
        title: "Split into phases (smart-1)",
        agent,
        text: prompt.template(
            `Break the refined task into small, ordered implementation phases for .plans/EXECUTION_PLAN.md.
                    Product contract: {contract}
                    ${PLAN_FORMAT_DOCTRINE}
                    Ground each phase in the codebase specifics from the contract. Do not implement anything.`,
            { contract: productContext },
        ),
        output: executionPlan,
        input: [taskInput, productContext],
    });
}
