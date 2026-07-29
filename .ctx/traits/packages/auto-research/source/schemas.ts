import { blockerSchema } from "@ctx-traits/agents";
import { schema } from "@ctx-traits/cdk";
import type { AutoResearchVariantFlags } from "./config.ts";

export function buildSchemas({ benchmarkRefactor }: AutoResearchVariantFlags) {
    const readinessStatus = schema.enum(
        "auto-research-readiness-status",
        ["ready", "abort"],
        { description: "Whether the isolated workbench is safe to mutate." },
    );
    const readiness = schema.object(
        "auto-research-readiness",
        {
            status: schema.field(readinessStatus, {
                description: "Ready only for a clean, isolated Git worktree.",
            }),
            detail: schema.field(schema.text(), {
                description: "Concise readiness or abort reason.",
            }),
        },
        { description: "Typed preflight result that gates every mutation." },
    );
    const gitRefSchema = schema.object(
        "auto-research-git-ref",
        {
            sha: schema.field(schema.text(), {
                description: "Immutable Git commit SHA captured by the trusted local runtime.",
            }),
        },
        { description: "Typed immutable Git ref captured from the isolated worktree." },
    );
    const resultStatus = schema.enum(
        "auto-research-result-status",
        ["ok", "error"],
        { description: "Whether the trusted experiment command produced a usable measurement." },
    );
    const experimentResult = benchmarkRefactor
        ? schema.object(
            "benchmark-refactor-result",
            {
                status: schema.field(resultStatus, {
                    description: "Ok only when the trusted benchmark command produced a usable measurement.",
                }),
                metric: schema.field(schema.number(), {
                    description: "Primary lower-is-better benchmark metric.",
                }),
            },
            { description: "Typed lower-is-better benchmark measurement from the trusted benchmark command." },
        )
        : schema.object(
            "auto-research-result",
            {
                status: schema.field(resultStatus, {
                    description: "Whether the experiment command produced a usable measurement.",
                }),
                metric: schema.field(schema.number(), {
                    description: "Primary lower-is-better experiment metric.",
                }),
                artifacts: schema.field(schema.list(schema.text()), {
                    description: "Paths, digests, or other evidence emitted by the experiment command.",
                }),
                score: schema.field(schema.number(), {
                    required: false,
                    description: "Optional supplementary passing count reported by the experiment command.",
                }),
                "wall-time-ms": schema.field(schema.integer(), {
                    required: false,
                    description: "Optional wall time spent by the experiment command.",
                }),
                "token-estimate": schema.field(schema.integer(), {
                    required: false,
                    description: "Optional static non-billing context estimate reported by the experiment command.",
                }),
                "cost-microusd": schema.field(schema.integer(), {
                    required: false,
                    description: "Optional provider cost of the experiment command, checked against max-cost-microusd when both are present.",
                }),
                iteration: schema.field(schema.integer(), {
                    required: false,
                    description: "Optional monotonic experiment-command invocation index, with baseline at zero.",
                }),
                "delta-lines": schema.field(schema.integer(), {
                    required: false,
                    description: "Optional changed line count relative to the current best worktree commit.",
                }),
                "canonical-digest": schema.field(schema.text(), {
                    required: false,
                    description: "Optional canonical digest of the evaluated candidate.",
                }),
                "measurement-scope": schema.field(schema.text(), {
                    required: false,
                    description: "Optional explicit boundary of the dimensions measured by the experiment command.",
                }),
            },
            { description: "Typed measurement emitted directly by the trusted experiment command." },
        );
    const proposalSchema = schema.object(
        "auto-research-proposal",
        {
            title: schema.field(schema.text(), {
                description: "Short experiment label.",
            }),
            hypothesis: schema.field(schema.text(), {
                description: "Falsifiable reason this small change may improve the declared metric.",
            }),
            change: schema.field(schema.text(), {
                description: "Exact bounded change for the worker to apply.",
            }),
        },
        { description: "One fresh, bounded experiment proposal." },
    );
    const summaryStatus = schema.enum(
        "auto-research-summary-status",
        ["completed", "aborted"],
        { description: "Terminal experiment outcome." },
    );
    const decisionStatus = benchmarkRefactor
        ? schema.enum(
            "benchmark-refactor-decision-status",
            ["baseline", "kept", "discarded", "review-rejected"],
            { description: "Deterministic disposition of one round." },
        )
        : schema.enum(
            "auto-research-decision-status",
            ["baseline", "kept", "discarded"],
            { description: "Deterministic disposition of one measured row." },
        );
    // Closed round-progress marker: "open" at round start, "complete" only in
    // the final atomic record step of every round arm (keep, discard, or
    // review-rejected). Gates the loop's `until` so the runtime's
    // after-each-step guard evaluation can never terminate mid-commit/reset.
    const roundStatus = benchmarkRefactor
        ? schema.enum(
            "benchmark-refactor-round-status",
            ["open", "complete"],
            { description: "Whether the current round's atomic record step has finished." },
        )
        : undefined;
    const reviewStatusSchema = benchmarkRefactor
        ? schema.enum(
            "benchmark-refactor-review-status",
            ["approved", "revise"],
            { description: "One round's quick-authority review verdict status." },
        )
        : undefined;
    const reviewVerdictSchema = benchmarkRefactor
        ? schema.object(
            "benchmark-refactor-review-verdict",
            {
                status: schema.field(reviewStatusSchema!, {
                    description: "approved when the candidate is behavior-preserving and introduces no new smell; revise otherwise. An unapproved candidate is reverted, never repaired.",
                }),
                blockers: schema.field(schema.list(blockerSchema), {
                    required: false,
                    description: "Blocking defects; present when status is revise.",
                }),
                advisory: schema.field(schema.text(), {
                    required: false,
                    description: "Non-blocking notes. Never affects status.",
                }),
            },
            { description: "Typed single-pass quick-authority review verdict for one implemented candidate." },
        )
        : undefined;
    const summaryReason = benchmarkRefactor
        ? schema.enum(
            "benchmark-refactor-summary-reason",
            ["target-reached", "budget-reached", "round-limit-reached", "aborted"],
            { description: "Which declared stop condition ended the run. budget-reached is partial success, not a failure. aborted is set only alongside status aborted." },
        )
        : undefined;
    const summaryRow = benchmarkRefactor
        ? schema.object(
            "benchmark-refactor-summary-row",
            {
                measurement: schema.field(experimentResult, {
                    required: false,
                    description: "The trusted benchmark measurement; absent for a review-rejected round, which is never measured.",
                }),
                decision: schema.field(decisionStatus, {
                    description: "Baseline marker, keep/discard decision, or review-rejected disposition.",
                }),
            },
            { description: "One round and its deterministic disposition." },
        )
        : schema.object(
            "auto-research-summary-row",
            {
                measurement: schema.field(experimentResult, {
                    description: "The exact trusted command measurement.",
                }),
                decision: schema.field(decisionStatus, {
                    description: "Baseline marker or keep/discard decision replayed from the declared guards.",
                }),
            },
            { description: "One measured experiment and its deterministic disposition." },
        );
    const summarySchema = benchmarkRefactor
        ? schema.object(
            "benchmark-refactor-summary",
            {
                status: schema.field(summaryStatus, {
                    description: "Completed once the target is reached, the time limit is hit, or the round budget is exhausted. budget-reached is partial success, not blocked or failed.",
                }),
                reason: schema.field(summaryReason!, {
                    description: "Which declared stop condition ended the run.",
                }),
                rounds: schema.field(schema.integer(), {
                    description: "Rounds recorded after the baseline measurement.",
                }),
                kept: schema.field(schema.integer(), {
                    description: "Candidates selected by the declared status/metric/noise-margin guard.",
                }),
                best: schema.field(schema.number(), {
                    required: false,
                    description: "Final lower-is-better best metric when a baseline was available.",
                }),
                rows: schema.field(schema.list(summaryRow), {
                    description: "Baseline followed by every round and its deterministic disposition.",
                }),
                reviews: schema.field(schema.list(reviewStatusSchema!), {
                    description: "Quick-authority review verdict status for every round that reached review.",
                }),
                detail: schema.field(schema.text(), {
                    description: "Concise terminal explanation, including any abort reason or measurement limitations.",
                }),
            },
            { description: "Typed terminal summary backed by benchmark measurements and runtime guard evidence." },
        )
        : schema.object(
            "auto-research-summary",
            {
                status: schema.field(summaryStatus, {
                    description: "Completed after the declared experiment budget, or aborted by preflight/baseline gating.",
                }),
                experiments: schema.field(schema.integer(), {
                    description: "Candidate experiments recorded after the baseline measurement.",
                }),
                kept: schema.field(schema.integer(), {
                    description: "Candidates selected by the declared status-and-metric guard.",
                }),
                best: schema.field(schema.number(), {
                    required: false,
                    description: "Final lower-is-better best metric when a baseline was available.",
                }),
                rows: schema.field(schema.list(summaryRow), {
                    description: "Baseline followed by every unchanged trusted measurement and deterministic disposition.",
                }),
                detail: schema.field(schema.text(), {
                    description: "Concise terminal explanation, including any abort reason or measurement limitations.",
                }),
            },
            { description: "Typed terminal summary backed by command measurements and runtime guard evidence." },
        );

    return {
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
    };
}

export type AutoResearchSchemas = ReturnType<typeof buildSchemas>;
