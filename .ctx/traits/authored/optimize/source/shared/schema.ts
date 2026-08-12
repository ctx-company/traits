import { blockerSchema } from "@ctx-traits/agents";
import type { SchemaHandle } from "@ctx-traits/cdk";
import { schema } from "@ctx-traits/cdk";

export const readinessStatus = schema.enum(
    "optimize-readiness-status",
    ["ready", "abort"],
    { description: "Whether the isolated workbench is safe to mutate." },
);
export const readiness = schema.object(
    "optimize-readiness",
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

export const gitRefSchema = schema.object(
    "optimize-git-ref",
    {
        sha: schema.field(schema.text(), {
            description: "Immutable Git commit SHA captured by the trusted local runtime.",
        }),
    },
    { description: "Typed immutable Git ref captured from the isolated worktree." },
);

export const runResultStatus = schema.enum(
    "optimize-run-result-status",
    ["ok", "error"],
    { description: "Whether one caller-supplied command run produced a usable measurement." },
);
export const runResult = schema.object(
    "optimize-run-result",
    {
        status: schema.field(runResultStatus, {
            description: "Whether this single run produced a usable measurement.",
        }),
        metric: schema.field(schema.number(), {
            description: "Primary lower-is-better metric reported by this single run.",
        }),
    },
    { description: "Typed measurement from one invocation of the caller-supplied command." },
);

export const resultStatus = schema.enum(
    "optimize-result-status",
    ["ok", "error"],
    { description: "Ok only when every run in the aggregate produced a usable measurement." },
);
export const aggregateResult = schema.object(
    "optimize-aggregate-result",
    {
        status: schema.field(resultStatus, {
            description: "Ok only when every one of the benchmark-runs invocations reported status ok.",
        }),
        metric: schema.field(schema.number(), {
            description: "Median lower-is-better metric across the benchmark-runs invocations — an actually-measured value, never a fabricated average.",
        }),
        runs: schema.field(schema.list(schema.number()), {
            description: "Every run's metric, in invocation order, so a kept/discarded decision is auditable.",
        }),
        "delta-lines": schema.field(schema.integer(), {
            required: false,
            description: "Optional changed line count relative to the current best worktree commit.",
        }),
    },
    { description: "Deterministic N-run aggregate measurement feeding the keep/discard guard." },
);

export const proposalSchema = schema.object(
    "optimize-proposal",
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

export const summaryStatus = schema.enum(
    "optimize-summary-status",
    ["completed", "aborted"],
    { description: "Terminal run outcome." },
);
export const decisionStatus = schema.enum(
    "optimize-decision-status",
    ["baseline", "kept", "discarded", "review-rejected"],
    { description: "Deterministic disposition of one round. review-rejected is benchmark-only — experiment rounds never reach it." },
);
export const summaryReason = schema.enum(
    "optimize-summary-reason",
    ["target-reached", "iteration-limit-reached", "aborted"],
    { description: "Which declared stop condition ended the run. aborted is set only alongside status aborted." },
);
export const summaryRow = schema.object(
    "optimize-summary-row",
    {
        measurement: schema.field(aggregateResult, {
            required: false,
            description: "The trusted N-run aggregate measurement; absent for a review-rejected round, which is never measured.",
        }),
        decision: schema.field(decisionStatus, {
            description: "Baseline marker, keep/discard decision, or review-rejected disposition.",
        }),
    },
    { description: "One round and its deterministic disposition." },
);
export const summarySchema = schema.object(
    "optimize-summary",
    {
        status: schema.field(summaryStatus, {
            description: "Completed once the target is reached or the iteration cap is exhausted. aborted only from preflight/baseline gating.",
        }),
        reason: schema.field(summaryReason, {
            description: "Which declared stop condition ended the run.",
        }),
        rounds: schema.field(schema.integer(), {
            description: "Rounds recorded after the baseline measurement.",
        }),
        kept: schema.field(schema.integer(), {
            description: "Candidates selected by the declared keep guard.",
        }),
        best: schema.field(schema.number(), {
            required: false,
            description: "Final lower-is-better best metric when a baseline was available.",
        }),
        rows: schema.field(schema.list(summaryRow), {
            description: "Baseline followed by every round and its deterministic disposition.",
        }),
        detail: schema.field(schema.text(), {
            description: "Concise terminal explanation, including any abort reason or measurement limitations.",
        }),
    },
    { description: "Typed terminal summary backed by command measurements and runtime guard evidence." },
);

export const reviewStatusSchema = schema.enum(
    "optimize-benchmark-review-status",
    ["approved", "revise"],
    { description: "One round's review verdict status." },
);

export function reviewVerdictSchema(): SchemaHandle {
    return schema.object(
        "optimize-review-verdict",
        {
            status: schema.field(reviewStatusSchema, {
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
        { description: "Typed single-pass review verdict for one implemented candidate. Review approval only routes into measurement — keep/discard is decided strictly by the measured-aggregate condition chain." },
    );
}
