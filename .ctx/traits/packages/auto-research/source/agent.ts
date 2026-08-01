import { reviewerRole, workerRole } from "@ctx-traits/agents";
import { agent } from "@ctx-traits/cdk";
import type { AgentHandle } from "@ctx-traits/cdk";

export interface AutoResearchAgents {
    readonly worker?: AgentHandle;
    readonly proposer?: AgentHandle;
    readonly summarizer?: AgentHandle;
    readonly brSmart1?: AgentHandle;
    readonly brWorker?: AgentHandle;
}

export const worker = agent({
    id: "worker",
    description: "Prepares the isolated workbench and applies one bounded experiment proposal without deciding whether its result is kept.",
    summary: "Workbench preparation and candidate-application role.",
    system: "Work only inside the prepared run worktree. Never report or choose the metric, best value, keep decision, or reset target; those are runtime-owned.",
});
export const proposer = agent({
    id: "proposer",
    description: "Proposes one small experiment from the objective, current best metric, and typed measurement history in a fresh per-frame session.",
    summary: "Fresh bounded-experiment proposal role.",
    system: "Propose one falsifiable, minimal change. Do not claim a metric result, a keep decision, or access to prior conversational context.",
});
export const summarizer = agent({
    id: "summarizer",
    description: "Reports a typed abort when preflight or baseline gating prevents deterministic experiment execution.",
    summary: "Typed abort-reporting role.",
    system: "Copy the supplied failure evidence exactly. Never invent measurements, decisions, or a successful outcome.",
});
export const benchmarkReviewer = reviewerRole("smart-1", "Scopes one bounded benchmark-improvement attempt, turns it into a concrete worker draft, and reviews the implemented candidate once.", "Scope-and-reviewer role.");
export const benchmarkWorker = workerRole("worker", "Implements smart-1's draft inside the isolated worktree.");
