import { reviewerRole, workerRole } from "@ctx-traits/agents";

// Both variants' worker never claims a metric, best value, keep decision,
// or reset target — those are runtime-owned; the guard is a command/project
// step chain, never a model-authored judgement.
export const worker = workerRole(
    "worker",
    "Prepares the isolated workbench and applies one bounded candidate change without deciding whether its measured result is kept.",
);

export const smart1 = reviewerRole(
    "smart-1",
    "Scopes one bounded benchmark-improvement attempt, turns it into a concrete worker draft, and reviews the implemented candidate once.",
);

// experiment-only seat: a proposer that drafts one fresh bounded experiment
// per round from typed measurement history (never a conversational memory).
// Preflight/baseline aborts are typed deterministic command steps, never an
// agent-authored summary (see shared/stage/summary.ts) — no summarizer seat.
export const proposer = reviewerRole(
    "proposer",
    "Proposes one small, falsifiable experiment from the objective, current best metric, and typed measurement history in a fresh per-frame session. Never claims a metric result, a keep decision, or access to prior conversational context.",
    "Fresh bounded-experiment proposal role.",
);
