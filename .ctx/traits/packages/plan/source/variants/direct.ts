import { Method, procedure, prompt, sequence, Tone, variant, Verbosity } from "@ctx-traits/cdk";
import { planningAgents } from "../agent.ts";
import { planData } from "../data.ts";
import { PLAN_FORMAT_DOCTRINE } from "../sequence/formatting.ts";
import { writePlanStep } from "../sequence/writing.ts";

const { smart1, scribe } = planningAgents(
    "Formats the task text into one well-formed plan group, near-verbatim.", "Format role.",
    "Writes the execution plan to .plans/EXECUTION_PLAN.md.", "Write role.",
);
const { taskInput, executionPlan, result, writtenFiles } = planData(false);

export default variant({
    name: "Plan (Direct)",
    summary: "Format a described task into one well-formed house-format plan group with wording kept near-verbatim, then append it to .plans/EXECUTION_PLAN.md — no refinement, no invention, the fastest path to just implement.",
    metadata: { tag: ["task", "plan", "bootstrap", "planning"] },
    behavior: { tone: [Tone.direct, Tone.technical], method: Method.evidenceFirst, verbosity: Verbosity.brief },
    intent: { focus: ["specific", "correctness"], avoid: ["speculative-claim", "scope-creep"] },
    procedure: procedure({
        description: "Format the described task into one well-formed house-format plan group with wording kept near-verbatim, then append it to .plans/EXECUTION_PLAN.md.",
        input: taskInput, output: writtenFiles,
        sequence: [
            sequence.prompt("format", {
                title: "Format into a plan group (smart-1)", agent: smart1,
                text: prompt.template(
                    `Format the task below into exactly one well-formed execution-plan group for .plans/EXECUTION_PLAN.md.
                    Task as described: {task}
                    Keep the task's own wording near-verbatim. Do NOT refine, invent, or add requirements the task does not state; do not read the codebase or ground it in anything beyond the task text itself.
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
