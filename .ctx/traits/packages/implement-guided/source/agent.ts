import { agent as cdkAgent } from "@ctx-traits/cdk";

const smart = cdkAgent("smart", {
    description:
        "Strong model: drafts the implementation from the annotations, reviews the work each round, and explains the finished diff hunk by hunk.",
    summary: "Drafting, review, and explanation role.",
});
const worker = cdkAgent("worker", {
    description: "Implements the draft and applies reviewer fixes each round.",
    summary: "Implementation role.",
});
const scribe = cdkAgent("scribe", {
    description: "Writes the commit message for the approved work; staging, committing and pushing run as command steps.",
    summary: "Commit-message role.",
});

export const agent = { smart, worker, scribe };
