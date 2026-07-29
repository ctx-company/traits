import { blockerSchema, commitTail, guardedProduction, planAmendmentSchema, SCOPE_SPLIT_DOCTRINE, SMART_VARIANT_DOCTRINE } from "@ctx-traits/agents";
import { condition, Intent, Method, port, procedure, prompt, schema, sequence, slot, variant, Tone, Verbosity } from "@ctx-traits/cdk";
import {
    buildPlanReviewText,
    buildProducePrompt,
    buildScribeText,
    captureDiffStep,
    clerk,
    commitOutput,
    commitReport,
    declareExecutionPlan,
    declareRuleAuthority,
    deriveParkReportStep,
    draft,
    leftovers,
    leftoversPort,
    ONE_TURN_DISCIPLINE,
    ownerItemSchema,
    phase,
    phaseBrief,
    phaseExtractionStep,
    productBrief,
    productExtractionStep,
    repoGatesPassed,
    repoGatesStep,
    reviewSeat,
    ruleSchema,
    scribe,
    smart1Role,
    smart2Role,
    verdictSchemaFor,
    verdictSlot,
    worker,
    workSummary,
} from "../sequence/family.ts";

// Repo-root resources are declared directly here (via the shared factory),
// not referenced through a trait `dependency` on implement-default: a
// dependency-vendored root="repo" resource loses the on-demand audit
// exemption a package's own direct declaration gets, forcing a full
// hidden-content scan of PRODUCT.md/EXECUTION_PLAN.md on every check. The
// factory keeps the declaration itself single-sourced even though each
// package instantiates its own resource node.
const executionPlan = declareExecutionPlan();
const ruleAuthority = declareRuleAuthority();

const smart1 = smart1Role(
    "Strong model with research tools: researches prior art before drafting, and reviews the work in the build loop.",
);
const smart2 = smart2Role("Independent strong review model: reviews the built work each build round, separately from smart-1.");

const CROSS_REVIEWER_CONFLICT_BLOCKER =
    "Treat a work summary that reports an unresolved cross-reviewer amendment conflict as a BLOCKER in your verdict: the conflict stays open until both reviewers reconcile it in a later round, and status must not be approved while it remains.";

const researchNotes = slot.text({
    id: "research-notes",
    description:
        "Prior art and precedent gathered before the draft: how comparable phases or the house's own history handled this problem, and any external guidance worth grounding the draft in.",
    hint: "Better inputs, not more rounds — this step runs once, before the draft, and is never repeated.",
});

const verdictSchema = verdictSchemaFor("smart", {
    amendments: schema.field(schema.list(planAmendmentSchema), {
        required: false,
        description:
            "Typed amendments to the agreed draft under smart authority, present when the draft and the observed work genuinely contradicted. Each becomes the binding contract going forward; an unrecorded departure is still a plan-fidelity gap, not an amendment.",
    }),
});
const verdict1 = verdictSlot("review-verdict-1", "smart-1", verdictSchema);
const verdict2 = verdictSlot("review-verdict-2", "smart-2", verdictSchema);

/**
 * This variant's own park-report list, declared locally (not the shared
 * `implement-default/source/shared.ts` `parkReport`/`parkReportPort`)
 * because its verdict schema is EXTENDED with `amendments` — a `project`
 * step can only copy `verdict2`'s whole accepted value onto a destination
 * whose declared schema matches it exactly, so the park-report list must
 * share this variant's own extended `verdictSchema`, not the family base.
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
        "Typed park record for an unapproved run (P414): the wall citation (if any), the exact blockers, and escalation state. Present in the run's persisted output-port evidence only when the build loop exhausted without approval — the run parks and no commit is created. A dispatch-time preflight refuses a sibling phase that explicitly cites the same wall-id while this stands unforced.",
    optional: true,
    value: parkReport,
    format: ["structured", "table"],
});

// implement-smart-only: the research-informed draft produce text (single
// consumer — not promoted to the shared kit or family shared.ts).
const smartDraftText = prompt.template(
    `Create an implementation draft for {phase}, informed by your research {researchNotes}.
    Work from the phase contract {phaseBrief} — a verbatim copy of the execution-plan section, your scope contract — and the product rules {productBrief}. Do not re-read the plan or product files.
    FIRST verify the contract is implementable: it states a falsifiable Done-when (not a one-line placeholder or a group marked "detail pending"), its group is not marked SUPERSEDED or CANCELLED, every prerequisite its Deps names is already landed in this tree, and the scope honestly fits one run. If any of these fail, open the draft with "CONTRACT PROBLEM:" naming exactly what is missing or conflicting — reviewers escalate on that evidence in round 1 instead of round 10 — then still draft whatever subset is honestly implementable.
    ${SCOPE_SPLIT_DOCTRINE}
    After the SCOPE SPLIT, the draft must cover: scope (exactly what the phase asks, nothing more), files to touch, approach, validation plan (the repo's standard gates), and risks.
    Favor the leanest correct approach: the minimal, robust, elegant change that satisfies the phase — no speculative generality, no gold-plating, no defensive armor for states that cannot occur. Explicitly call out where existing code should be REUSED or a shared abstraction EXTRACTED rather than re-implemented or copied beside. Reference repo files by path; do not inline documents. Do not implement anything.
    ${ONE_TURN_DISCIPLINE}`,
    { phase, researchNotes, phaseBrief, productBrief },
);

// implement-smart-only: the amendment-conflict extension to the shared
// produce prompt (`buildProducePrompt`'s `extraFixInstruction`) and its own
// return contract (the produce step returns `draft` + `work-summary`, not
// just `work-summary`) — everything else is the shared paragraph text.
const SMART_AMENDMENT_FIX_INSTRUCTION =
    "When a verdict carries amendments, those become the new binding contract: rewrite the draft to incorporate them exactly, then implement to the amended draft. If verdict1 and verdict2 propose amendments that genuinely conflict with each other, do NOT adjudicate between them yourself: leave the draft unamended on the conflicting point, implement neither side of the conflict, and report the conflict verbatim (both proposed amendments and why they contradict) as an open item in the work summary — the conflict must remain unresolved and blocking until the reviewers reconcile it in a later round, never silently decided here.";
const SMART_PRODUCE_RETURN_CONTRACT =
    'Return exactly two outputs: draft (the complete draft text, amended by every non-conflicting amendment, unchanged on any conflicting point) and work-summary (what changed, files, how it was validated, open concerns, a "Leftovers proposed" section (explicit list or explicit none) — including any unresolved amendment conflict, named explicitly so the next review round blocks on it, and — once a verdict has ever been attached — a "Blocker dispositions" section as in a plain build round).';
const smartProduceText = buildProducePrompt({
    extraFixInstruction: SMART_AMENDMENT_FIX_INSTRUCTION,
    returnContract: SMART_PRODUCE_RETURN_CONTRACT,
});

const researchStep = sequence.prompt("research", {
    title: "Research prior art (smart-1)",
    agent: smart1,
    text: prompt.text`
        Before drafting for ${phase}, research prior art: how comparable phases in this codebase's history solved a similar problem, and any external precedent worth grounding the draft in. Use web-capable tools if available.
        Keep this to one pass — the loop budget is unchanged from the default flow; smart means a better-informed draft, not more rounds.
        Return your research notes: what you found, its relevance, and anything that should constrain the coming draft.`,
    output: researchNotes,
});

const planning = guardedProduction({
    id: "planning",
    produces: draft,
    produce: { agent: smart1, text: smartDraftText },
    review: { agent: smart2, text: buildPlanReviewText() },
    rounds: 2,
    onExhausted: "continue",
});
const building = guardedProduction({
    id: "building",
    produces: [draft, workSummary],
    produce: { agent: worker, text: smartProduceText, optionalInputs: [leftovers, repoGatesPassed] },
    review: [
        reviewSeat(smart1, 1, SMART_VARIANT_DOCTRINE, ruleAuthority, verdict1, { extraInstruction: CROSS_REVIEWER_CONFLICT_BLOCKER }),
        reviewSeat(smart2, 2, SMART_VARIANT_DOCTRINE, ruleAuthority, verdict2, { extraInstruction: CROSS_REVIEWER_CONFLICT_BLOCKER }),
    ],
    evidence: [repoGatesStep, captureDiffStep],
    afterReview: deriveParkReportStep([verdict1, verdict2], { parkReportSlot: parkReport }),
    alsoRequire: condition.equals(repoGatesPassed.ok, true),
    carry: leftovers,
    minRounds: 3,
    rounds: 5,
    onExhausted: "block",
});

export default variant({
    name: "Implement (Smart)",
    summary:
        "Research-informed dogfood implementation procedure: research prior art, extract the phase and product context, draft the approach, implement it, refine against two independent reviewers who may amend the draft, and commit.",
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
            "Implement one execution-plan phase end to end with a research-informed draft: research, extract context, draft the approach, implement it, refine against two independent reviewers with typed amendments, then summarize and commit.",
        input: phase,
        output: [commitReport, leftoversPort, parkReportPort],
        sequence: [
            phaseExtractionStep(clerk, executionPlan),
            productExtractionStep(clerk, ruleAuthority),
            researchStep,
            planning,
            building,
            ...commitTail({
                id: "shipping",
                scribe: { agent: scribe, text: buildScribeText(verdict1, verdict2) },
                summary: workSummary,
                verdict: building.verdict,
                receipt: commitOutput,
            }),
        ],
    }),
});
