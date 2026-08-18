import * as agents from "@ctx-traits/agents";

export const smart1 = agents.reviewerRole("smart-1", "Surveys, frames, designs, and reviews.");

export const smart2 = agents.reviewerRole("smart-2", "Independent reviewer.");

export const worker = agents.workerRole("worker", "Implements the agreed refactor design and applies reviewer fixes.");

export const scribe = agents.scribeRole("scribe", "Writes the commit message from the agreed design and survey record");
