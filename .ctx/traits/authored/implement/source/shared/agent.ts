// Cross-variant agent roles every implement variant's build loop shares
// unchanged.
import { clerkRole, reviewerRole, scribeRole, workerRole } from "@ctx-traits/agents";
import type { AgentHandle } from "@ctx-traits/cdk";

export function smart1Role(description: string): AgentHandle {
  return reviewerRole("smart-1", description, "Drafting and first-reviewer role.");
}
export function smart2Role(description: string): AgentHandle {
  return reviewerRole("smart-2", description, "Second-reviewer role.");
}
export const worker = workerRole("worker", "Implements the draft and applies reviewer fixes.");
export const scribe = scribeRole("scribe", "Writes the commit message for the completed task from the task contract");
export const clerk = clerkRole(
  "clerk",
  "Fast extraction model: copies the task file out of the task board verbatim, so no later step re-reads the board.",
);
