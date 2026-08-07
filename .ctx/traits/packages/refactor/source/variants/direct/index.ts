import { input, intent, procedure, sequence, variant } from "@ctx-traits/cdk";

import { directPlanner, directWorker } from "./agent.ts";
import { directAnnotationsSlot, directChecklist, directWorkReport } from "./data.ts";

const directAnnotationStep = sequence.command("annotate", {
    title: "Collect annotations (ctx-annotate)",
    cmd: "ctx-annotate",
    timeoutMs: 3_600_000,
    output: directAnnotationsSlot,
});
const directChecklistStep = sequence.prompt("checklist", {
    title: "Build the checklist",
    agent: directPlanner,
    text: input.prompt`Turn the captured annotations ${directAnnotationsSlot} into a checklist.`,
    output: directChecklist,
});
const directImplementationStep = sequence.prompt("implement", {
    title: "Implement the checklist",
    agent: directWorker,
    text: input.prompt`Implement every item in the checklist ${directChecklist}.`,
    output: directWorkReport,
});

export default variant({
    name: "Refactor (Direct)",
    summary: "Turns human annotations into directly implemented changes — no review loop, no commit.",
    metadata: { tag: ["first-party", "refactoring", "annotations"] },
    intent: {
        require: [
            intent.require.AnnotationFidelity,
            intent.require.PreserveScope,
        ],
        avoid: [intent.avoid.ScopeCreep, intent.avoid.OverEngineering],
    },
    port: directWorkReport,
    procedure: procedure({
        description: "Collect human annotations interactively, plan them as a checklist, implement every item, and report.",
        sequence: [directAnnotationStep, directChecklistStep, directImplementationStep],
    }),
});
