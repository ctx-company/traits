import { reviewerRole, workerRole } from "@ctx-traits/agents";
import { agent } from "@ctx-traits/cdk";

export function buildAgents({ benchmarkRefactor }: { benchmarkRefactor: boolean }) {
    // benchmark-refactor's own quick-authority reviewer/worker pair (reused
    // from `@ctx-traits/agents`, the same roles/doctrine `refactor-quick`
    // uses) instead of the generic proposer/summarizer roles.
    const brSmart1 = benchmarkRefactor
        ? reviewerRole(
            "smart-1",
            "Scopes one bounded benchmark-improvement attempt, turns it into a concrete worker draft, and reviews the implemented candidate once.",
            "Scope-and-reviewer role.",
        )
        : undefined;
    const brWorker = benchmarkRefactor
        ? workerRole("worker", "Implements smart-1's draft inside the isolated worktree.")
        : undefined;

    const worker = agent({
        id: "worker",
        description:
            "Prepares the isolated workbench and applies one bounded experiment proposal without deciding whether its result is kept.",
        summary: "Workbench preparation and candidate-application role.",
        system:
            "Work only inside the prepared run worktree. Never report or choose the metric, best value, keep decision, or reset target; those are runtime-owned.",
    });
    const proposer = agent({
        id: "proposer",
        description:
            "Proposes one small experiment from the objective, current best metric, and typed measurement history in a fresh per-frame session.",
        summary: "Fresh bounded-experiment proposal role.",
        system:
            "Propose one falsifiable, minimal change. Do not claim a metric result, a keep decision, or access to prior conversational context.",
    });
    const summarizer = agent({
        id: "summarizer",
        description:
            "Reports a typed abort when preflight or baseline gating prevents deterministic experiment execution.",
        summary: "Typed abort-reporting role.",
        system:
            "Copy the supplied failure evidence exactly. Never invent measurements, decisions, or a successful outcome.",
    });

    return { worker, proposer, summarizer, brSmart1, brWorker };
}

export type AutoResearchAgents = ReturnType<typeof buildAgents>;
