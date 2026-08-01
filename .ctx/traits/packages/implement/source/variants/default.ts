import { blockerSchema, DEFAULT_VARIANT_DOCTRINE } from "@ctx-traits/agents";
import { intent, method, procedure, variant, tone, verbosity } from "@ctx-traits/cdk";
import {
    clerk,
    commitReport,
    declareTaskBoard,
    familyProcedure,
    gateTimedOut,
    leftoversPort,
    ownerItemSchema,
    parkReportPort,
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

const taskBoard = declareTaskBoard();

const verdictSchema = verdictSchemaFor("default");
const verdict1 = verdictSlot("review-verdict-1", "smart-1", verdictSchema);
const verdict2 = verdictSlot("review-verdict-2", "smart-2", verdictSchema);

export default variant({
    name: "Implement (Default)",
    summary:
        "Surveyed dogfood implementation procedure: extract the task contract from the task board, draft the approach, implement it, then a doubly-reviewed bounded refinement loop tuned for pragmatism, robustness, elegance, correctness, leanness, and reuse — then summarize and commit.",
    metadata: { tag: ["dogfood", "implementation", "review", "multi-agent"] },
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
    resource: [taskBoard],
    signal: [gateTimedOut],
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
        }),
    }),
});
