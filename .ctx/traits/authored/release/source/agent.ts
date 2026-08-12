import { reviewerRole, scribeRole, workerRole } from "@ctx-traits/agents";

export const worker = workerRole("worker", "Bumps the version files the release manifest declares.");
export const reviewer = reviewerRole(
  "reviewer",
  "Reviews the version bump for correctness and spot-checks the changelog against landed commits, refuting rather than confirming.",
  "Review role.",
);
export const scribe = scribeRole(
  "scribe",
  "Drafts the changelog entry from the commit log and writes the release commit message.",
);
