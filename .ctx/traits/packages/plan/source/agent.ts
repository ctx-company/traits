import { agent } from "@ctx-traits/cdk";

export function planningAgents(smartDescription: string, smartSummary: string, scribeDescription: string, scribeSummary: string) {
    return {
        smart1: agent.reviewer("smart-1", { description: smartDescription, summary: smartSummary }),
        scribe: agent.planner("scribe", { description: scribeDescription, summary: scribeSummary }),
    };
}
