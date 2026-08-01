import { intent, method, procedure, tone, variant, verbosity } from "@ctx-traits/cdk";
import { quickScribe, quickSmart1 } from "../agent.ts";
import { result, taskFiles, taskInput, writtenFiles } from "../data.ts";
import { distillTasksStep } from "../sequence/formatting.ts";
import { writeTasksStep } from "../sequence/writing.ts";

export default variant({
    name: "Plan (Quick)",
    summary: "Distill a described task directly into house-format task files and add them to .internal/tasks/ — no separate grounding pass, no review pass.",
    metadata: { tag: ["task", "plan", "bootstrap", "planning"] },
    behavior: { tone: [tone.Direct, tone.Technical], method: method.EvidenceFirst, verbosity: verbosity.Brief },
    intent: { focus: [intent.focus.Specific, intent.focus.Correctness], avoid: [intent.avoid.SpeculativeClaim, intent.avoid.ScopeCreep] },
    port: writtenFiles,
    procedure: procedure({
        description: "Distill the described task, grounded in the codebase, straight into house-format task files, then add them to .internal/tasks/.",
        sequence: [
            distillTasksStep(quickSmart1, taskInput, taskFiles),
            writeTasksStep(quickScribe, taskFiles, result),
        ],
    }),
});
