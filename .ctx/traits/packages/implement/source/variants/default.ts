import { blockerSchema, DEFAULT_VARIANT_DOCTRINE } from "@ctx-traits/agents";
import { Intent, Method, procedure, variant, Tone, Verbosity } from "@ctx-traits/cdk";
import {
    clerk,
    commitReport,
    declareExecutionPlan,
    declareRuleAuthority,
    familyProcedure,
    leftoversPort,
    ownerItemSchema,
    parkReportPort,
    phase,
    ruleSchema,
    scribe,
    smart1Role,
    smart2Role,
    verdictSchemaFor,
    verdictSlot,
    worker,
} from "../sequence/family.ts";

const smart1 = smart1Role("Strong model: drafts the implementation plan, and reviews the work in the refinement loop.");
const smart2 = smart2Role("Independent strong review model: reviews the implemented work each refinement pass, separately from smart-1.");

const executionPlan = declareExecutionPlan();
const ruleAuthority = declareRuleAuthority();

const verdictSchema = verdictSchemaFor("default");
const verdict1 = verdictSlot("review-verdict-1", "smart-1", verdictSchema);
const verdict2 = verdictSlot("review-verdict-2", "smart-2", verdictSchema);

export default variant({
    name: "Implement (Default)",
    summary:
        "Surveyed dogfood implementation procedure: extract the phase and product context, draft the approach, implement it, then a doubly-reviewed bounded refinement loop tuned for pragmatism, robustness, elegance, correctness, leanness, and reuse — then summarize and commit.",
    metadata: { tag: ["dogfood", "implementation", "review", "multi-agent"] },
    behavior: {
        tone: [Tone.direct, Tone.technical],
        method: Method.evidenceFirst,
        verbosity: Verbosity.brief,
    },
    intent: {
        require: [
            Intent.focus.correctness,
            { id: "robustness", summary: "Prefer changes that remain correct under ordinary failure and boundary conditions." },
            { id: "pragmatism", summary: "Choose the smallest practical change that satisfies the phase contract." },
            { id: "elegance", summary: "Favor clear, cohesive designs over clever or incidental complexity." },
            Intent.require.leanness,
            Intent.require.reuseOverReimplement,
            Intent.require.reviewBeforeFinal,
            Intent.require.boundedRefinement,
        ],
        avoid: [
            Intent.avoid.accretion,
            Intent.avoid.overEngineering,
            Intent.avoid.goldPlating,
            Intent.avoid.duplication,
            Intent.avoid.scopeCreep,
            Intent.avoid.unboundedLoop,
            Intent.avoid.rubberStampReview,
        ],
    },
    schema: [blockerSchema, ownerItemSchema, ruleSchema],
    resource: [executionPlan, ruleAuthority],
    procedure: procedure({
        description:
            "Implement one execution-plan phase end to end: extract context, draft the approach, implement it, refine against two independent reviewers until both approve, then summarize and commit — favoring the minimal, reuse-first implementation.",
        input: phase,
        output: [commitReport, leftoversPort, parkReportPort],
        sequence: familyProcedure({
            clerk,
            smart1,
            smart2,
            worker,
            scribe,
            executionPlan,
            ruleAuthority,
            variantDoctrine: DEFAULT_VARIANT_DOCTRINE,
            verdict1,
            verdict2,
        }),
    }),
});
