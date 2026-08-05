import { agent as cdkAgent } from "@ctx-traits/cdk";

const smart1 = cdkAgent("smart-1", {
    description:
        "Strong model: drafts the plan, refines it against plan-mode annotations, reviews the implemented work each round, and writes the owner's round briefing.",
    summary: "Drafting, plan refinement, review, and briefing role.",
});
const smart2 = cdkAgent("smart-2", {
    description:
        "Strong model: writes the tracked brief and the commit message for the approved work, grounded in the work summary, the final verdict, and the working tree.",
    summary: "Brief and commit-message role.",
});
const worker = cdkAgent("worker", {
    description: "Implements the plan and applies reviewer/owner fixes each round.",
    summary: "Implementation role.",
});

export const agent = { smart1, smart2, worker };
