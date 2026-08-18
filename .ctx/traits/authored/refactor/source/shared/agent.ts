import * as agents from "@ctx-traits/agents";

export const smart = agents.reviewerRole("smart", "Surveys, frames, designs, and reviews.");

export const worker = agents.workerRole("worker", "Implements the agreed refactor design.");

export const scribe = agents.scribeRole("scribe", "Writes the commit message and summary.");
