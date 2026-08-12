import { reviewerRole, scribeRole, workerRole } from "@ctx-traits/agents";

export const smart1 = reviewerRole("smart-1", "Surveys, frames, designs, and reviews.");

export const smart2 = reviewerRole("smart-2", "Independent reviewer.");

export const worker = workerRole("worker", "Implements the agreed refactor design and applies reviewer fixes.");

export const scribe = scribeRole("scribe", "Writes the commit message from the agreed design and survey record");
