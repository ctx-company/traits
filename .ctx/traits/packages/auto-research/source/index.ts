import { blockerSchema } from "@ctx-traits/agents";
import { intent, method, procedure, trait } from "@ctx-traits/cdk";
import { buildAgents } from "./agents.ts";
import { buildSchemas } from "./schemas.ts";
import { buildSlots } from "./slots.ts";
import { buildSteps } from "./steps.ts";

export type AutoResearchVariant = "generic" | "benchmark-refactor";

export function autoResearch(variant: AutoResearchVariant) {
    const benchmarkRefactor = variant === "benchmark-refactor";
    const traitId = benchmarkRefactor ? "benchmark-refactor" : "auto-research";

    const schemas = buildSchemas({ benchmarkRefactor });
    const agents = buildAgents({ benchmarkRefactor });
    const slots = buildSlots(schemas, { benchmarkRefactor });
    const sequence = buildSteps(agents, slots, { benchmarkRefactor });

    const {
        readinessStatus,
        readiness,
        gitRefSchema,
        resultStatus,
        experimentResult,
        proposalSchema,
        summaryStatus,
        decisionStatus,
        roundStatus,
        reviewStatusSchema,
        reviewVerdictSchema,
        summaryReason,
        summaryRow,
        summarySchema,
    } = schemas;
    const { worker, proposer, summarizer, brSmart1, brWorker } = agents;
    const {
        objective,
        experimentCommand,
        metricField,
        maxExperiments,
        target,
        benchmarkCommand,
        improvementTarget,
        noiseThreshold,
        maxRounds,
        timeLimitSeconds,
        maxWallTime,
        maxTokenEstimate,
        maxCost,
        maxDelta,
        summaryReplay,
        summaryPort,
    } = slots;

    const inputs = benchmarkRefactor
        ? [target!, benchmarkCommand!, improvementTarget!, noiseThreshold!, maxRounds!, timeLimitSeconds!]
        : [objective!, metricField!, maxExperiments!, experimentCommand!, maxWallTime!, maxTokenEstimate!, maxCost!, maxDelta!];

    return trait(traitId, {
        version: "0.1.0",
        name: benchmarkRefactor ? "Benchmark Refactor" : "Auto Research",
        summary: benchmarkRefactor
            ? "Optimizes a lower-is-better benchmark over a caller-selected code area, keeping only command-measured improvements that clear a noise threshold."
            : "Runs bounded experiments whose keep/discard decision is a deterministic guard over typed command output, with optional wall-time, token, cost, and delta-line caps.",
        metadata: {
            tag: benchmarkRefactor
                ? ["auto-research", "benchmark", "refactor", "provenance"]
                : ["auto-research", "experiments", "optimization", "provenance"],
        },
        intent: {
            require: benchmarkRefactor
                ? [intent.require.reviewBeforeFinal, intent.Correctness]
                : [intent.Correctness, intent.GatesGreenBeforeCommit],
        },
        behavior: {
            method: [method.EvidenceFirst],
        },
        agent: benchmarkRefactor ? [brSmart1!, brWorker!] : [worker, proposer, summarizer],
        schema: [
            ...(benchmarkRefactor ? [blockerSchema, reviewStatusSchema!, reviewVerdictSchema!, roundStatus!] : []),
            readinessStatus,
            readiness,
            gitRefSchema,
            resultStatus,
            experimentResult,
            ...(benchmarkRefactor ? [] : [proposalSchema]),
            summaryStatus,
            decisionStatus,
            ...(benchmarkRefactor ? [summaryReason!] : []),
            summaryRow,
            summarySchema,
        ],
        ...(benchmarkRefactor ? {} : { resource: [summaryReplay!], "schema-version": "0.3" }),
        procedure: procedure({
            description: benchmarkRefactor
                ? "Verify an isolated worktree, seed a trusted baseline benchmark, run a bounded scope/draft/implement/review round loop, and deterministically keep only benchmark-measured improvements beyond the noise threshold."
                : "Verify an isolated worktree, seed a trusted baseline, run a dynamically bounded fresh-proposal experiment loop, and deterministically keep only command-measured improvements.",
            worktreeRequired: true,
            input: inputs,
            output: summaryPort,
            sequence,
        }),
    });
}

export default autoResearch("generic");
