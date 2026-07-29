import { reviewerRole, scribeRole, workerRole } from "@ctx-traits/agents";
import type { AgentHandle } from "@ctx-traits/cdk";

export function smart1Role(description: string): AgentHandle {
    return reviewerRole("smart-1", description, "Survey, design, reviewer.");
}

export function smart2Role(description: string): AgentHandle {
    return reviewerRole("smart-2", description, "Independent reviewer.");
}

export const worker = workerRole("worker", "Implements the agreed refactor design and applies reviewer fixes.");
export const scribe = scribeRole("scribe", "Writes the refactor commit message from the agreed design and survey record");
