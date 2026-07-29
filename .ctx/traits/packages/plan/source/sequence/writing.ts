import type { AgentHandle, SlotHandle } from "@ctx-traits/cdk";
import { prompt, sequence } from "@ctx-traits/cdk";

export function writePlanStep(agent: AgentHandle, executionPlan: SlotHandle, result: SlotHandle) {
    return sequence.prompt("write", {
        title: "Write EXECUTION_PLAN.md (scribe)", agent,
        text: prompt.text`
                    Write the execution plan group below to .plans/EXECUTION_PLAN.md, at the repo root.
                    Execution plan group: ${executionPlan}
                    Create the .plans directory if missing. Keep the group faithful — do not add or drop material.
                    Do NOT overwrite existing content: if the file already exists, APPEND the group as a new group at the end. If the file does not exist, create it with this content. Do not write .docs/PRODUCT.md.
                    Return one line naming the path you wrote.`,
        output: result, input: [executionPlan],
    });
}

export function writeContractAndPlanStep(agent: AgentHandle, productContext: SlotHandle, executionPlan: SlotHandle, result: SlotHandle) {
    return sequence.prompt("write", {
        title: "Write PRODUCT.md + EXECUTION_PLAN.md (scribe)", agent,
        text: prompt.text`
                    Write the two files implement-phase needs, at the repo root.
                    Product contract (for .docs/PRODUCT.md): ${productContext}
                    Execution plan group (for .plans/EXECUTION_PLAN.md): ${executionPlan}
                    Create the .docs and .plans directories if missing. Write the product contract to .docs/PRODUCT.md and the plan group to .plans/EXECUTION_PLAN.md, keeping each faithful — do not add or drop material.
                    Do NOT overwrite existing content: if a file already exists, APPEND (add the plan as a new group at the end of .plans/EXECUTION_PLAN.md; add the contract as a clearly-headed new section in .docs/PRODUCT.md). If a file does not exist, create it with that content.
                    Return one line naming both paths you wrote.`,
        output: result, input: [productContext, executionPlan],
    });
}
