// Dogfood entry point for the implement family: a thin package-identity
// wrapper over the shared family procedure (P363), plus the review rubric
// and the recurrence breaker. Never add a second copy of the
// extraction/draft/implement/refinement-loop/commit sequence here — that
// machinery lives once in ../sequence/family.ts.
import { blockerSchema, DEFAULT_VARIANT_DOCTRINE } from "@ctx-traits/agents";
import { condition, intent, method, port, procedure, resource, schema, signal, slot, variant, tone, verbosity } from "@ctx-traits/cdk";
import {
    clerk,
    commitReport,
    declareTaskBoard,
    familyProcedure,
    leftoversPort,
    ownerItemSchema,
    scribe,
    smart1Role,
    smart2Role,
    task,
    verdictSchemaFor,
    verdictSlot,
    worker,
} from "../sequence/family.ts";

const smart1 = smart1Role("Strong model: drafts the implementation plan, and reviews the work in the refinement loop.");
const smart2 = smart2Role("Independent strong review model: reviews the implemented work each refinement pass, separately from smart-1.");

// The repo-root task-board resource is declared directly here (via the
// shared factory), not referenced through a trait dependency: see
// variants/smart.ts for why a dependency-vendored root="repo" resource
// loses the on-demand hidden-content audit exemption a package's own
// direct declaration gets.
const taskBoard = declareTaskBoard();

// implement-phase-only: the richer review rubric (P276) is this package's
// own on-demand resource, threaded optionally into the shared review
// builders — sibling implement variants that never pass one stay
// byte-equivalent.
const reviewRubric = resource({
    id: "review-rubric",
    path: "resources/review-rubric.md",
    hint: "The three required review dimensions (scope, correctness, gates-ran), their evidence expectations, and the blocker/status relationship; agents read this file with their own tools.",
    trigger: "on-demand",
});

const reviewFindingSchema = schema.object(
    "review-finding",
    {
        finding: schema.field(schema.text(), { description: "What was checked and what was found, in the reviewer's own terms." }),
        file: schema.field(schema.text(), { description: "Repo-relative path this finding cites." }),
        line: schema.field(schema.integer(), { description: "Line number in that file this finding cites." }),
    },
    { description: "One review-rubric finding: a concrete, source-anchored observation citing a repo-relative file and line." },
);

// A bare `schema.list(reviewFindingSchema)` accepts `[]`, which would let a
// dimension go unchecked despite being `required` — `required` only enforces
// field presence, not list non-emptiness. Splitting each dimension into one
// mandatory `first` finding plus an optional `rest` list makes at least one
// source-anchored finding structurally unavoidable.
function dimensionFindingsSchema(id: string, dimension: string) {
    return schema.object(
        id,
        {
            first: schema.field(reviewFindingSchema, { description: `The mandatory first ${dimension} finding.` }),
            rest: schema.field(schema.list(reviewFindingSchema), { description: `Any further ${dimension} findings beyond the mandatory first one.` }),
        },
        { description: `${dimension}-dimension findings, with at least one finding required.` },
    );
}

// P334: the low end of the contract's "2-3" — a blocker still open after two
// consecutive raisings has already had one root-fix attempt fail its own
// done-when, which is the exact signal the recurrence breaker exists to
// catch.
const RECURRENCE_BREAKER_ROUNDS = 2;

const verdictSchema = verdictSchemaFor("default", {
    scope: schema.field(dimensionFindingsSchema("scope-findings", "scope"), {
        description: "Scope-dimension findings: does the diff implement exactly what the task contract asks, source-anchored.",
    }),
    correctness: schema.field(dimensionFindingsSchema("correctness-findings", "correctness"), {
        description: "Correctness-dimension findings: is the implemented behavior actually right, source-anchored.",
    }),
    "gates-ran": schema.field(dimensionFindingsSchema("gates-ran-findings", "gates-ran"), {
        description: "Gates-ran-dimension findings: which of this task's Done-when gates actually ran, and their result, source-anchored.",
    }),
    "recurrence-rounds": schema.field(schema.integer(), {
        description:
            "The number of consecutive rounds the longest-standing still-open blocker has now been raised: 1 for a first-raised blocker, 0 when blockers is empty. Derived from the same prior-round comparison that sets a blocker's recurrence-of.",
    }),
});
const verdict1 = verdictSlot("review-verdict-1", "smart-1", verdictSchema);
const verdict2 = verdictSlot("review-verdict-2", "smart-2", verdictSchema);

// P334: the recurrence breaker's own terminal signal, distinct from
// `refinement-exhausted`/`max-iterations-exhausted`, so a park caused by a
// twice-unresolved blocker reads as its own named reason in the ledger, the
// receipt, and `run-status`.
const recurringBlockerUnresolved = signal({
    id: "recurring-blocker-unresolved",
    description: "Either reviewer's verdict still recurs the same unresolved blocker after the breaker's round threshold; the loop stops here rather than spending the rest of its budget.",
});

// P334: mutually exclusive with `tripleGreen` by construction — each
// disjunct conjoins with that reviewer's own `status == "revise"`, which a
// `tripleGreen`-matching (i.e. both-approved) round always falsifies, so the
// runtime can never see both `until` and `stop-if` match in the same
// evaluation (which would otherwise block as `guard-conflict`).
const recurrenceStopIf = condition.any([
    condition.all([
        condition.fieldEquals(verdict1, "status", "revise"),
        condition.fieldGte(verdict1, "recurrence-rounds", RECURRENCE_BREAKER_ROUNDS),
    ]),
    condition.all([
        condition.fieldEquals(verdict2, "status", "revise"),
        condition.fieldGte(verdict2, "recurrence-rounds", RECURRENCE_BREAKER_ROUNDS),
    ]),
]);

/**
 * This variant's own park-report list, declared locally (not the shared
 * `implement-default/source/shared.ts` `parkReport`/`parkReportPort`)
 * because its verdict schema is EXTENDED with the four review-rubric
 * dimensions — a `project` step can only copy `verdict2`'s whole accepted
 * value onto a destination whose declared schema matches it exactly, so the
 * park-report list must share this variant's own extended `verdictSchema`,
 * not the family base.
 */
const parkReport = slot({
    id: "park-report",
    schema: schema.list(verdictSchema),
    description:
        "This round's typed park record (P414): empty when the round's verdict is approved, exactly one entry — the round's own verdict, copied unchanged — when it is revise. Written each round by a deterministic project step, never model-authored, so it can never disagree with the verdict it comes from.",
});
const parkReportPort = port.output.of(schema.list(verdictSchema), {
    id: "park-report",
    title: "Park Report",
    description:
        "Typed park record for an unapproved run (P414): the wall citation (if any), the exact blockers, and escalation state. Present in the run's persisted output-port evidence only when the refinement loop exhausted without approval — the run parks and no commit is created. A dispatch-time preflight refuses a sibling task that explicitly cites the same wall-id while this stands unforced.",
    optional: true,
    value: parkReport,
    format: ["structured", "table"],
});

export default variant({
    name: "Implement Phase",
    summary:
        "Dogfood implementation procedure: extract the task contract from the task board, draft the approach, implement it, then a doubly-reviewed bounded refinement loop tuned for pragmatism, robustness, elegance, correctness, leanness, and reuse — then summarize and commit.",
    metadata: {
        tag: ["dogfood", "implementation", "review", "multi-agent"],
    },
    behavior: {
        tone: [tone.direct, tone.technical],
        method: method.evidenceFirst,
        verbosity: verbosity.brief,
    },
    intent: {
        require: [
            intent.focus.correctness,
            { id: "robustness", summary: "Prefer changes that remain correct under ordinary failure and boundary conditions." },
            { id: "pragmatism", summary: "Choose the smallest practical change that satisfies the task contract." },
            { id: "elegance", summary: "Favor clear, cohesive designs over clever or incidental complexity." },
            intent.require.leanness,
            intent.require.reuseOverReimplement,
            intent.require.reviewBeforeFinal,
            intent.require.boundedRefinement,
        ],
        avoid: [
            intent.avoid.accretion,
            intent.avoid.overEngineering,
            intent.avoid.goldPlating,
            intent.avoid.duplication,
            intent.avoid.scopeCreep,
            intent.avoid.unboundedLoop,
            intent.avoid.rubberStampReview,
        ],
    },
    schema: [blockerSchema, ownerItemSchema],
    resource: [taskBoard, reviewRubric],
    signal: [recurringBlockerUnresolved],
    procedure: procedure({
        description:
            "Implement one task from the task board end to end: extract its contract, draft the approach, implement it, refine against two independent reviewers until both approve, then summarize and commit — favoring the minimal, reuse-first implementation.",
        input: task,
        output: [commitReport, leftoversPort, parkReportPort],
        sequence: familyProcedure({
            clerk,
            smart1,
            smart2,
            worker,
            scribe,
            taskBoard,
            variantDoctrine: DEFAULT_VARIANT_DOCTRINE,
            verdict1,
            verdict2,
            reviewRubric,
            parkReportOutput: parkReport,
            stopIf: recurrenceStopIf,
            onStop: recurringBlockerUnresolved,
            reviewExtraInstruction:
                `Report "recurrence-rounds": the number of consecutive rounds the longest-standing still-open blocker has now been raised (1 for a first-raised blocker, 0 when blockers is empty) — the same comparison that sets a blocker's recurrence-of. The loop terminates early, before the iteration budget, once either reviewer reports a still-revise verdict with recurrence-rounds >= ${RECURRENCE_BREAKER_ROUNDS}.`,
        }),
    }),
});
