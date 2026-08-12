import { reviewerRole, scribeRole, workerRole } from "@ctx-traits/agents";

export const smart = reviewerRole(
  "smart-1",
  "Strong model: drafts the implementation plan, and is the sole reviewer in the refinement loop.",
  "Drafting and reviewer role.",
);
export const worker = workerRole("worker", "Implements the draft and applies reviewer fixes.");
export const scribe = scribeRole("scribe", "Writes the commit message for the completed task from the task contract");
