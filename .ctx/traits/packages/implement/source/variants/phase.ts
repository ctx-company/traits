// Dogfood entry point for the implement family: a thin package-identity
// wrapper over the shared family procedure (P363), plus the review rubric
// and the recurrence breaker. Never add a second copy of the
// extraction/draft/implement/refinement-loop/commit sequence here — that
// machinery lives once in ../sequence/family.ts.
import { blockerSchema, DEFAULT_VARIANT_DOCTRINE } from "@ctx-traits/agents";
import { condition, intent, method, operation, port, procedure, prompt, resource, schema, sequence, signal, slot, variant, tone, verbosity } from "@ctx-traits/cdk";
import {
    clerk,
    commitReport,
    declareTaskBoard,
    familyProcedure,
    gateTimedOut,
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
            "The number of consecutive rounds the longest-standing still-open blocker has had exactly the same still-open step statuses with zero delta from the prior verdict: 1 for a first-raised blocker, 0 when blockers is empty. Reset it to 1 when any carried blocker step changes status or the blocker clears; never infer it from prose.",
    }),
    // DISABLED 2026-08-01 (owner): feasibility gate + stall handoff are parked until polished — re-enable by restoring these lines.
    // "stall-question": schema.field(schema.text(), {
    //     description:
    //         `Empty while recurrence-rounds is below ${RECURRENCE_BREAKER_ROUNDS}. At or above that threshold, one concrete, owner-answerable question that names the still-open blocker and the decision or access needed to clear it.`,
    // }),
});
const verdict1 = verdictSlot("review-verdict-1", "smart-1", verdictSchema);
const verdict2 = verdictSlot("review-verdict-2", "smart-2", verdictSchema);

// DISABLED 2026-08-01 (owner): feasibility gate + stall handoff are parked until polished — re-enable by restoring these lines.
// const stallQuestion = slot.text({
//     id: "stall-question",
//     description: "The selected concrete owner question for a zero-delta recurring blocker, projected deterministically from reviewer 1 when both reviewers trip together.",
// });
// const ownerAnswer = slot.text({
//     id: "owner-answer",
//     description: "The owner's answer to the selected phase stall question, attached optionally to the next worker frame.",
// });
// const ownerInputRequired = signal({
//     id: "owner-input-required",
//     description: "A phase reviewer has reached the zero-delta recurrence threshold and requires one owner answer before refinement can resume.",
// });
//
// const recurrenceThreshold1 = condition.all([
//     condition.fieldEquals(verdict1, "status", "revise"),
//     condition.fieldGte(verdict1, "recurrence-rounds", RECURRENCE_BREAKER_ROUNDS),
// ]);
// const recurrenceThreshold2 = condition.all([
//     condition.fieldEquals(verdict2, "status", "revise"),
//     condition.fieldGte(verdict2, "recurrence-rounds", RECURRENCE_BREAKER_ROUNDS),
// ]);
// const recurrenceThreshold = condition.any([
//     recurrenceThreshold1,
//     recurrenceThreshold2,
// ]);
//
// // This runs after park-report projection but before the loop guard. The ask
// // stays inside the matching branch so a persistent signal cannot expose it in
// // an unrelated later round.
// const recurrenceOwnerQuestion = sequence.when("recurrence-owner-question", {
//     if: recurrenceThreshold,
//     then: [
//         sequence.when("recurrence-question-reviewer-1", {
//             if: recurrenceThreshold1,
//             then: [
//                 sequence.project("recurrence-question-project-reviewer-1", {
//                     projections: [{ source: verdict1, field: "stall-question", destination: stallQuestion }],
//                 }),
//             ],
//         }),
//         sequence.when("recurrence-question-reviewer-2", {
//             if: condition.all([condition.not(recurrenceThreshold1), recurrenceThreshold2]),
//             then: [
//                 sequence.project("recurrence-question-project-reviewer-2", {
//                     projections: [{ source: verdict2, field: "stall-question", destination: stallQuestion }],
//                 }),
//             ],
//         }),
//         sequence.ask("recurrence-owner-answer", {
//             prompt: input.prompt`The implementation is stalled on a recurring blocker. Owner answer required: ${stallQuestion}`,
//             when: ownerInputRequired,
//             output: ownerAnswer,
//         }),
//     ],
// });

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
        "Dogfood implementation procedure: extract the task contract from the task board, draft the approach, implement it, then a doubly-reviewed bounded refinement loop — then summarize and commit.",
    metadata: {
        tag: ["dogfood", "implementation", "review", "multi-agent"],
    },
    behavior: {
        tone: [tone.Direct, tone.Technical],
        method: method.EvidenceFirst,
        verbosity: verbosity.Brief,
    },
    intent: {
        require: [
            intent.focus.Correctness,
            intent.require.Robustness,
            intent.require.Pragmatism,
            intent.require.Elegance,
            intent.require.Leanness,
            intent.require.ReuseOverReimplement,
            intent.require.ReviewBeforeFinal,
            intent.require.BoundedRefinement,
        ],
        avoid: [
            intent.avoid.Accretion,
            intent.avoid.OverEngineering,
            intent.avoid.GoldPlating,
            intent.avoid.Duplication,
            intent.avoid.ScopeCreep,
            intent.avoid.UnboundedLoop,
            intent.avoid.RubberStampReview,
        ],
    },
    resource: [taskBoard, reviewRubric],
    signal: [gateTimedOut],
    port: [commitReport, leftoversPort, parkReportPort],
    procedure: procedure({
        description:
            "Implement one task from the task board end to end: extract its contract, draft the approach, implement it, refine against two independent reviewers until both approve, then summarize and commit — favoring the minimal, reuse-first implementation.",
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
            // DISABLED 2026-08-01 (owner): feasibility gate + stall handoff are parked until polished — re-enable by restoring these lines.
            reviewExtraInstruction:
                `Report "recurrence-rounds" only when a carried blocker has the same still-open step statuses with zero delta from the prior verdict: 1 for a first-raised blocker, 0 when blockers is empty, and reset to 1 whenever progress changes a carried step status.`,
        }),
    }),
});
