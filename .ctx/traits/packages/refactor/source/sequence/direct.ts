import { prompt, sequence } from "@ctx-traits/cdk";
import { directPlanner, directWorker } from "../direct-agent.ts";
import { directAnnotationsSlot, directChecklist, directWorkReport } from "../direct-data.ts";

export const directAnnotationStep = sequence.command("annotate", {
    title: "Collect annotations (ctx-annotate)",
    cmd: "ctx-annotate",
    timeoutMs: 3_600_000,
    output: directAnnotationsSlot,
});
export const directChecklistStep = sequence.prompt("checklist", {
    title: "Build the checklist",
    agent: directPlanner,
    text: prompt.text`Turn the captured annotations ${directAnnotationsSlot} into a checklist.`,
    output: directChecklist,
});
export const directImplementationStep = sequence.prompt("implement", {
    title: "Implement the checklist",
    agent: directWorker,
    text: prompt.text`Implement every item in the checklist ${directChecklist}.`,
    output: directWorkReport,
});
