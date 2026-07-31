import { Method, procedure, prompt, sequence, Tone, variant, Verbosity } from "@ctx-traits/cdk";
import { planningAgents } from "../agent.ts";
import { planData } from "../data.ts";
import { TASK_FORMAT_DOCTRINE } from "../resources/task-format.ts";
import { writeTasksStep } from "../sequence/writing.ts";

const { smart1, scribe } = planningAgents(
    "Distills the task directly into house-format task files.", "Distill role.",
    "Writes the task files to .internal/tasks/.", "Write role.",
);
const { taskInput, taskFiles, result, writtenFiles } = planData(false);

export default variant({
    name: "Plan (Quick)",
    summary: "Distill a described task directly into house-format task files and add them to .internal/tasks/ — no separate grounding pass, no review pass.",
    metadata: { tag: ["task", "plan", "bootstrap", "planning"] },
    behavior: { tone: [Tone.direct, Tone.technical], method: Method.evidenceFirst, verbosity: Verbosity.brief },
    intent: { focus: ["specific", "correctness"], avoid: ["speculative-claim", "scope-creep"] },
    procedure: procedure({
        description: "Distill the described task, grounded in the codebase, straight into house-format task files, then add them to .internal/tasks/.",
        input: taskInput, output: writtenFiles,
        sequence: [
            sequence.prompt("distill", {
                title: "Distill into task files (smart-1)", agent: smart1,
                text: prompt.template(
                    `Turn the task below directly into one or more house-format task files for .internal/tasks/ — skip deriving separate grounding notes.
                    Task as described: {task}
                    Read the relevant parts of the repository with your tools to ground each task file in this codebase's concrete files, modules, and validation gates.
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
