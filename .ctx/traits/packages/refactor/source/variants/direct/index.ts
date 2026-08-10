import { defineVariant, input, intent, step, useIntent } from "@ctx-traits/cdk";

import { directPlanner, directWorker } from "./agent.ts";
import { directAnnotationsSlot, directChecklist, directWorkReport } from "./data.ts";

const checklistText = input.prompt`Turn the captured annotations ${directAnnotationsSlot} into a checklist.`;
const implementText = input.prompt`Implement every item in the checklist ${directChecklist}.`;

export default function () {
    defineVariant("direct", {
        name: "Refactor (Direct)",
        summary: "Turns human annotations into directly implemented changes — no review loop, no commit.",
        metadata: { tag: ["first-party", "refactoring", "annotations"] },
        procedure: "Collect human annotations interactively, plan them as a checklist, implement every item, and report.",
    });
    useIntent({
        require: [
            intent.require.AnnotationFidelity,
            intent.require.PreserveScope,
        ],
        avoid: [intent.avoid.ScopeCreep, intent.avoid.OverEngineering],
    });

    step.command("Collect annotations (ctx-annotate)", {
        id: "annotate",
        cmd: "ctx-annotate",
        timeoutMs: 3_600_000,
        output: directAnnotationsSlot,
    });
    directPlanner.prompt("Build the checklist", { id: "checklist", input: checklistText, output: directChecklist });
    directWorker.prompt("Implement the checklist", { id: "implement", input: implementText, output: directWorkReport });

    return { workReport: directWorkReport };
}
