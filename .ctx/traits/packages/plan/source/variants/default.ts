import { method, procedure, tone, variant, verbosity } from "@ctx-traits/cdk";
import { planningAgents } from "../agent.ts";
import { planData } from "../data.ts";
import { refineTaskStep, splitTasksStep } from "../sequence/planning.ts";
import { writeTasksStep } from "../sequence/writing.ts";

const { smart1, scribe } = planningAgents(
    "Strong model: refines the task, grounds it in the codebase, and splits it into task files.", "Refinement + grounding role.",
    "Writes the task files to .internal/tasks/.", "Bootstrap-writer role.",
);
const { taskInput, grounding, taskFiles, result, writtenFiles } = planData(true);
if (!grounding) throw new Error("default plan requires grounding");

export default variant({
    name: "Plan",
    summary: "Turn a described task into codebase-grounded, house-format task files in .internal/tasks/ — the board the implement family runs from in any repo.",
    metadata: { tag: ["task", "plan", "bootstrap", "planning"] },
    behavior: { tone: [tone.direct, tone.technical], method: method.evidenceFirst, verbosity: verbosity.brief },
    intent: { focus: ["specific", "correctness"], avoid: ["speculative-claim", "scope-creep"] },
    procedure: procedure({
        description: "Refine the described task against the codebase, split it into small task files, and write them to .internal/tasks/ so the implement family can run.",
        input: taskInput, output: writtenFiles,
        sequence: [
            refineTaskStep(smart1, taskInput, grounding),
            splitTasksStep(smart1, taskInput, grounding, taskFiles),
            writeTasksStep(scribe, taskFiles, result),
        ],
    }),
});
