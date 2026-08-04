import { agent as cdkAgent } from "@ctx-traits/cdk";

const smart = cdkAgent("smart", {
    description:
        "Strong model: drafts the plan, refines it against plan-mode annotations, and reviews the implemented work each round against the plan and the owner's annotations.",
    summary: "Drafting, plan refinement, and implement-review role.",
});
const worker = cdkAgent("worker", {
    description: "Implements the plan and applies reviewer/owner fixes each round.",
    summary: "Implementation role.",
});
const scribe = cdkAgent("scribe", {
    description: "Writes the tracked brief and the commit message for the approved work; staging and committing run as command steps.",
    summary: "Brief and commit-message role.",
});

export const agent = { smart, worker, scribe };
