import { Method, procedure, prompt, sequence, Tone, variant, Verbosity } from "@ctx-traits/cdk";
import { planningAgents } from "../agent.ts";
import { planData } from "../data.ts";
import { PLAN_FORMAT_DOCTRINE } from "../sequence/formatting.ts";
import { writePlanStep } from "../sequence/writing.ts";

const { smart1, scribe } = planningAgents(
    "Distills the task directly into a house-format execution-plan group.", "Distill role.",
    "Writes the execution plan to .plans/EXECUTION_PLAN.md.", "Write role.",
);
const { taskInput, executionPlan, result, writtenFiles } = planData(false);

export default variant({
    name: "Plan (Quick)",
    summary: "Distill a described task directly into a house-format execution-plan group and append it to .plans/EXECUTION_PLAN.md — no product contract, no review pass.",
    metadata: { tag: ["task", "plan", "bootstrap", "planning"] },
    behavior: { tone: [Tone.direct, Tone.technical], method: Method.evidenceFirst, verbosity: Verbosity.brief },
    intent: { focus: ["specific", "correctness"], avoid: ["speculative-claim", "scope-creep"] },
    procedure: procedure({
        description: "Distill the described task, grounded in the codebase, straight into a house-format execution-plan group, then append it to .plans/EXECUTION_PLAN.md.",
        input: taskInput, output: writtenFiles,
        sequence: [
            sequence.prompt("distill", {
                title: "Distill into a plan group (smart-1)", agent: smart1,
                text: prompt.template(
                    `Turn the task below directly into a phased execution-plan group for .plans/EXECUTION_PLAN.md — skip deriving a separate product contract.
                    Task as described: {task}
                    Read the relevant parts of the repository with your tools to ground each phase in this codebase's concrete files, modules, and validation gates.
                    ${PLAN_FORMAT_DOCTRINE}
                    Do not implement anything.`,
                    { task: taskInput },
                ),
                output: executionPlan, input: [taskInput],
            }),
            writePlanStep(scribe, executionPlan, result),
        ],
    }),
});
