import { agent as cdkAgent } from "@ctx-traits/cdk";

const smart1 = cdkAgent("smart-1", {
    description:
        "Strong model: turns the captured annotations into a concrete, ordered checklist of changes.",
    summary: "Checklist-planning role.",
});
const worker = cdkAgent("worker", {
    description: "Implements every checklist item and validates the result.",
    summary: "Implementation role.",
});

export const agent = { smart1, worker };
