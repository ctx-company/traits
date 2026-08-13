import * as agents from "@ctx-traits/agents";

export const worker = agents.workerRole("worker", "Bumps the version files the release manifest declares.");

export const reviewer = agents.reviewerRole("reviewer", "Reviews the version bump and spot-checks the changelog.");

export const scribe = agents.scribeRole("scribe", "Drafts the changelog entry and the release commit message.");
