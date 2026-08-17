// Cross-variant agent roles. The worker implements; smart-1 reviews (and
// smart-2 independently cross-reviews in complex); the scribe only writes
// the commit message.
import { reviewerRole, scribeRole, workerRole } from "@ctx-traits/agents";

export const worker = workerRole("worker", "Implements the task and applies reviewer fixes.");
export const smart1 = reviewerRole(
  "smart-1",
  "Strong model: reviews the implemented state each refinement round.",
  "Reviewer role.",
);
export const smart2 = reviewerRole(
  "smart-2",
  "Independent strong model: cross-reviews the implemented state each round, separately from smart-1.",
  "Second-reviewer role.",
);
export const scribe = scribeRole("scribe", "Writes the commit message for the completed task from the work summary");
