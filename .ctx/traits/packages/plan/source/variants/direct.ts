import { method, procedure, prompt, sequence, tone, variant, verbosity } from "@ctx-traits/cdk";
import { planningAgents } from "../agent.ts";
import { planData } from "../data.ts";
import { TASK_FORMAT_DOCTRINE } from "../resources/task-format.ts";
import { writeTasksStep } from "../sequence/writing.ts";

const { smart1, scribe } = planningAgents(
    "Formats the task text into one well-formed task file, near-verbatim.", "Format role.",
    "Writes the task file to .internal/tasks/.", "Write role.",
);
const { taskInput, taskFiles, result, writtenFiles } = planData(false);

export default variant({
    name: "Plan (Direct)",
    summary: "Format a described task into one well-formed house-format task file with wording kept near-verbatim, then add it to .internal/tasks/ — no refinement, no invention, the fastest path to just implement.",
    metadata: { tag: ["task", "plan", "bootstrap", "planning"] },
    behavior: { tone: [tone.direct, tone.technical], method: method.evidenceFirst, verbosity: verbosity.brief },
    intent: { focus: ["specific", "correctness"], avoid: ["speculative-claim", "scope-creep"] },
    procedure: procedure({
        description: "Format the described task into one well-formed house-format task file with wording kept near-verbatim, then add it to .internal/tasks/.",
        input: taskInput, output: writtenFiles,
        sequence: [
            sequence.prompt("format", {
                title: "Format into a task file (smart-1)", agent: smart1,
                text: prompt.template(
                    `Format the task below into exactly one well-formed task file for .internal/tasks/.
                    Task as described: {task}
                    Keep the task's own wording near-verbatim. Do NOT refine, invent, or add requirements the task does not state; do not read the codebase or ground it in anything beyond the task text itself.
                    ${TASK_FORMAT_DOCTRINE}
                    Do not implement anything.`,
                    { task: taskInput },
                ),
                output: taskFiles, input: [taskInput],
            }),
            writeTasksStep(scribe, taskFiles, result),
        ],
    }),
});
