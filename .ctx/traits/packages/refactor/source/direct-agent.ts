import { agent } from "@ctx-traits/cdk";

export const directPlanner = agent("smart-1", {
    description:
        "Strong model: turns the captured annotations into a concrete, ordered checklist of changes.",
    summary: "Checklist-planning role.",
});
export const directWorker = agent("worker", {
    description: "Implements every checklist item and validates the result.",
    summary: "Implementation role.",
});
