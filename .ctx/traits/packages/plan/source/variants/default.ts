import { intent, method, procedure, tone, variant, verbosity } from "@ctx-traits/cdk";
import { defaultScribe, defaultSmart1 } from "../agent.ts";
import { grounding, result, taskFiles, taskInput, writtenFiles } from "../data.ts";
import { refineTaskStep, splitTasksStep } from "../sequence/planning.ts";
import { writeTasksStep } from "../sequence/writing.ts";

export default variant({
    name: "Plan",
    summary: "Turn a described task into codebase-grounded, house-format task files in .internal/tasks/ — the board the implement family runs from in any repo.",
    metadata: { tag: ["task", "plan", "bootstrap", "planning"] },
    behavior: { tone: [tone.Direct, tone.Technical], method: method.EvidenceFirst, verbosity: verbosity.Brief },
    intent: { focus: [intent.focus.Specific, intent.focus.Correctness], avoid: [intent.avoid.SpeculativeClaim, intent.avoid.ScopeCreep] },
    port: writtenFiles,
    procedure: procedure({
        description: "Refine the described task against the codebase, split it into small task files, and write them to .internal/tasks/ so the implement family can run.",
        sequence: [
            refineTaskStep(defaultSmart1, taskInput, grounding),
            splitTasksStep(defaultSmart1, taskInput, grounding, taskFiles),
            writeTasksStep(defaultScribe, taskFiles, result),
        ],
    }),
});
