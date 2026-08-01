import { port, resource, schema, slot } from "@ctx-traits/cdk";
import type { AutoResearchVariantFlags } from "./config.ts";
import type { AutoResearchSchemas } from "./schemas.ts";

const SUMMARY_REPLAY_DIGEST = "sha256:007749f093bf5cbc5d2dcca8eab4dd123bdd729f7de36243c9f424842ba5df56";

export function buildSlots(schemas: AutoResearchSchemas, { benchmarkRefactor }: AutoResearchVariantFlags) {
    const { readiness, gitRefSchema, experimentResult, proposalSchema, decisionStatus, roundStatus, reviewStatusSchema, reviewVerdictSchema, summarySchema } =
        schemas;

    const objective = benchmarkRefactor
        ? undefined
        : port.input.text({
            id: "objective",
            description: "The bounded optimization objective for this experiment run.",
        });
    const metricField = benchmarkRefactor
        ? undefined
        : port.input.text({
            id: "metric-field",
            description: "Human-readable meaning and unit normalized into experiment-result.metric.",
        });
    const maxExperiments = benchmarkRefactor
        ? undefined
        : port.input.of(schema.integer(), {
            id: "max-experiments",
            description: "Positive runtime-resolved bound on candidate experiments.",
        });
    const experimentCommand = benchmarkRefactor
        ? undefined
        : port.input.of(schema.list(schema.text()), {
            id: "experiment-command",
            description: "No-shell argv executed for baseline and candidate measurements.",
        });

    // benchmark-refactor's own inputs: caller-selected code area, a no-shell
    // benchmark command emitting typed `{ status, metric }` JSON, the
    // lower-is-better target, a non-negative noise floor, and P385's bounded
    // `max-rounds` shape (the expected stop condition is the time limit, not
    // this ceiling).
    const target = benchmarkRefactor
        ? port.input.text({
            id: "target",
            description: "Module, file path, or code area to optimize the benchmark over.",
        })
        : undefined;
    const benchmarkCommand = benchmarkRefactor
        ? port.input.of(schema.list(schema.text()), {
            id: "benchmark-command",
            description: 'No-shell argv executed for baseline and candidate measurements; must emit JSON matching { status: "ok" | "error", metric: number }.',
        })
        : undefined;
    const improvementTarget = benchmarkRefactor
        ? port.input.of(schema.number(), {
            id: "improvement-target",
            description: "Lower-is-better metric target; the run completes immediately once the baseline or any kept candidate reaches it.",
        })
        : undefined;
    const noiseThreshold = benchmarkRefactor
        ? port.input.of(schema.number(), {
            id: "noise-threshold",
            description: "Non-negative minimum improvement margin below which a lower metric is treated as noise, not a real improvement.",
        })
        : undefined;
    const maxRounds = benchmarkRefactor
        ? port.input.of(schema.integer(), {
            id: "max-rounds",
            description: "Bounded round ceiling (P385 shape); the expected stop condition is the time limit, not this bound.",
        })
        : undefined;
    const timeLimitSeconds = benchmarkRefactor
        ? port.input.of(schema.integer(), {
            id: "time-limit-seconds",
            description: "Soft wall-clock budget in cumulative active-drive seconds; the run completes with reason budget-reached once reached.",
        })
        : undefined;
    const maxWallTime = benchmarkRefactor
        ? undefined
        : port.input.of(schema.integer(), {
            id: "max-wall-time-ms",
            optional: true,
            description: "Optional maximum accepted experiment-command wall time per candidate; the cap is skipped when omitted, and discards a candidate that supplies no wall-time-ms measurement.",
        });
    const maxTokenEstimate = benchmarkRefactor
        ? undefined
        : port.input.of(schema.integer(), {
            id: "max-token-estimate",
            optional: true,
            description: "Optional maximum accepted static non-billing context estimate for the candidate; the cap is skipped when omitted, and discards a candidate that supplies no token-estimate measurement.",
        });
    const maxCost = benchmarkRefactor
        ? undefined
        : port.input.of(schema.integer(), {
            id: "max-cost-microusd",
            optional: true,
            description: "Optional maximum accepted experiment-command provider cost; the cap is skipped when omitted, and discards a candidate that supplies no cost-microusd measurement.",
        });
    const maxDelta = benchmarkRefactor
        ? undefined
        : port.input.of(schema.integer(), {
            id: "max-delta-lines",
            optional: true,
            description: "Optional maximum accepted changed lines relative to the current best Git ref; the cap is skipped when omitted, and discards a candidate that supplies no delta-lines measurement.",
        });
    const summaryReplay = resource.file("auto-research-summary-replay", {
        path: "resources/derive-summary.mjs",
        digest: SUMMARY_REPLAY_DIGEST,
        hint: "Digest-pinned replay validator that recomputes and re-asserts the terminal summary from runtime-owned counters and ledgers.",
        trigger: "on-demand",
    });

    const readinessSlot = slot({
        id: "readiness",
        schema: readiness,
        description: "Latest typed worktree readiness result.",
    });
    const bestRef = slot({
        id: "best-ref",
        schema: schema.text(),
        description: "Immutable current-best commit SHA, advanced only after a kept commit succeeds.",
    });
    const capturedRef = slot({
        id: "captured-ref",
        schema: gitRefSchema,
        description: "Latest immutable commit SHA captured by a trusted Git command.",
    });
    const commandReceipt = slot.text({
        id: "command-receipt",
        description: "Latest deterministic Git command's raw stdout.",
    });
    const baselineResult = slot({
        id: "baseline-result",
        schema: experimentResult,
        description: "Trusted command measurement that seeds best state.",
    });
    const candidateResult = slot({
        id: "candidate-result",
        schema: experimentResult,
        description: "Current trusted candidate measurement read by the declared keep guard.",
    });
    const bestMetric = slot.number({
        id: "best-metric",
        description: "Loop-carried lower-is-better metric projected only from trusted command output.",
    });
    const history = slot.array(experimentResult, {
        id: "history",
        description: "Baseline and candidate command measurements in deterministic ledger order.",
    });
    const experimentCount = slot({
        id: "experiment-count",
        schema: schema.integer(),
        description: "Candidate experiments recorded by keep/discard arms.",
    });
    const keptCount = slot({
        id: "kept-count",
        schema: schema.integer(),
        description: "Candidate experiments selected by the declared keep guard.",
    });
    const decisions = slot.array(decisionStatus, {
        id: "decisions",
        description: "Baseline marker and runtime-owned keep/discard dispositions in execution order.",
    });
    const proposal = slot({
        id: "proposal",
        schema: proposalSchema,
        description: "Current fresh experiment proposal.",
    });
    const applyReceipt = slot.text({
        id: "apply-receipt",
        description: "Worker receipt for the bounded candidate mutation.",
    });
    const summary = slot({
        id: "summary",
        schema: summarySchema,
        description: "Terminal typed experiment report.",
    });
    const summaryPort = port.output.of(summarySchema, {
        id: "summary",
        title: "Auto-research Summary",
        description: "Typed baseline, experiment measurements, kept count, and final best metric.",
        value: summary,
        format: "json",
    });

    // --- benchmark-refactor's own round shape: smart-1 scopes an attempt,
    // turns it into a worker draft, the worker implements it, smart-1 gives
    // exactly one review pass. An approved candidate is measured and kept
    // only when it clears the noise-threshold margin; anything else (a
    // measured non-improvement, or an unapproved review) reverts. Every arm's
    // final step is the one atomic write that both records the round AND
    // flips `round-complete` to "complete", so the runtime's after-each-step
    // guard evaluation can never observe a mid-commit/mid-reset state as done.
    const scopeSlot = benchmarkRefactor
        ? slot.text({
            id: "scope",
            description: "smart-1's bounded scope for one benchmark-improvement attempt, derived from the target, current best metric, and measurement history.",
        })
        : undefined;
    const draftSlot = benchmarkRefactor
        ? slot.text({
            id: "draft",
            description: "smart-1's concrete worker draft turning the scope into an actionable implementation brief.",
        })
        : undefined;
    const implementReceipt = benchmarkRefactor
        ? slot.text({
            id: "implement-receipt",
            description: "Worker's report of the implemented candidate.",
        })
        : undefined;
    const reviewVerdict = benchmarkRefactor
        ? slot({
            id: "review-verdict",
            schema: reviewVerdictSchema!,
            description: "smart-1's single review verdict for the implemented candidate.",
        })
        : undefined;
    const marginResult = benchmarkRefactor
        ? slot.boolean({
            id: "margin-result",
            description: "Whether the candidate's metric improves on best by more than the noise threshold.",
        })
        : undefined;
    const roundComplete = benchmarkRefactor
        ? slot({
            id: "round-complete",
            schema: roundStatus!,
            description: "\"open\" at round start, \"complete\" only in the final atomic record step of the round's arm.",
        })
        : undefined;
    const reviews = benchmarkRefactor
        ? slot.array(reviewStatusSchema!, {
            id: "reviews",
            description: "Quick-authority review verdict status for every round that reached review, in execution order.",
        })
        : undefined;

    return {
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
        readinessSlot,
        bestRef,
        capturedRef,
        commandReceipt,
        baselineResult,
        candidateResult,
        bestMetric,
        history,
        experimentCount,
        keptCount,
        decisions,
        proposal,
        applyReceipt,
        summary,
        summaryPort,
        scopeSlot,
        draftSlot,
        implementReceipt,
        reviewVerdict,
        marginResult,
        roundComplete,
        reviews,
    };
}

export type AutoResearchSlots = ReturnType<typeof buildSlots>;
