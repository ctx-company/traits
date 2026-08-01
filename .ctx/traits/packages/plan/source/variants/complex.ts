import { intent, method, procedure, tone, variant, verbosity } from "@ctx-traits/cdk";
import { complexScribe, complexSmart1 } from "../agent.ts";
import { critique, grounding, result, taskFiles, taskInput, writtenFiles } from "../data.ts";
import { critiqueTasksStep, reviseTasksStep } from "../sequence/critique.ts";
import { refineTaskStep, splitTasksStep } from "../sequence/planning.ts";
import { writeTasksStep } from "../sequence/writing.ts";

export default variant({
    name: "Plan (Complex)",
    summary: "Turn a described task into codebase-grounded, house-format task files, then independently critique and revise them once before writing to .internal/tasks/.",
    metadata: { tag: ["task", "plan", "bootstrap", "planning", "review"] },
    behavior: { tone: [tone.Direct, tone.Technical], method: method.EvidenceFirst, verbosity: verbosity.Brief },
    intent: { focus: [intent.focus.Specific, intent.focus.Correctness], avoid: [intent.avoid.SpeculativeClaim, intent.avoid.ScopeCreep] },
    port: writtenFiles,
    procedure: procedure({
        description: "Refine the described task against the codebase, split it into small task files, independently critique them, apply one bounded revise pass, and write them to .internal/tasks/ so the implement family can run.",
        sequence: [
            refineTaskStep(complexSmart1, taskInput, grounding),
            splitTasksStep(complexSmart1, taskInput, grounding, taskFiles),
            critiqueTasksStep(complexSmart1, grounding, taskFiles, critique),
            reviseTasksStep(complexScribe, critique, taskFiles),
            writeTasksStep(complexScribe, taskFiles, result),
        ],
    }),
});
