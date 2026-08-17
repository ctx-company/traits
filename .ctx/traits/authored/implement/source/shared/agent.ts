import * as agents from "@ctx-traits/agents";

export const worker = agents.workerRole("worker", "Implementation Work");

export const smart = agents.reviewerRole("smart", "Plan Drafting and Review");

export const scribe = agents.scribeRole("scribe", "Commit & Summary Writing");
