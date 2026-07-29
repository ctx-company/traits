import { reviewerRole, scribeRole, workerRole } from "@ctx-traits/agents";

const smart = reviewerRole(
    "smart-1",
    "Strong model: drafts the implementation plan, and is the sole reviewer in the refinement loop.",
    "Drafting and reviewer role.",
);
const worker = workerRole("worker", "Implements the draft and applies reviewer fixes.");
const scribe = scribeRole("scribe", "Writes the commit message for the completed phase from the execution plan");

export const agent = { smart, worker, scribe };
