import { reviewerRole, workerRole } from "@ctx-traits/agents";

const smart = reviewerRole(
    "smart-1",
    "Strong model: drafts the implementation plan, refines it against the owner's plan-mode annotations, is the sole reviewer in the refinement loop, writes the owner's round briefings, and writes the brief and commit message for the approved work.",
    "Drafting, reviewer, briefing, and brief role.",
);
// No scribe: quick's scribe is a no-tools planner role, and gated's brief must
// be grounded in the actual working tree (git diff/status), so the smart role
// writes it.
const worker = workerRole("worker", "Implements the plan and applies reviewer and owner fixes.");

export const agent = { smart, worker };
