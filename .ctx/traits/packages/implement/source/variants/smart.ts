import { blockerSchema, commitTail, guardedProduction, planAmendmentSchema, SCOPE_SPLIT_DOCTRINE, SMART_VARIANT_DOCTRINE } from "@ctx-traits/agents";
import { condition, intent, method, port, procedure, prompt, schema, sequence, slot, variant, tone, verbosity } from "@ctx-traits/cdk";
import {
    buildPlanReviewText,
    buildProducePrompt,
    buildScribeText,
    captureDiffStep,
    clerk,
    commitOutput,
    commitReport,
    declareTaskBoard,
    deriveParkReportStep,
    draft,
    gateTimedOut,
    gateTimedOutStopIf,
    leftovers,
    leftoversPort,
    ONE_TURN_DISCIPLINE,
    ownerItemSchema,
    promptText,
    repoGatesPassed,
    repoGatesStep,
    reviewSeat,
    scribe,
    smart1Role,
    smart2Role,
    task,
    taskBrief,
    taskExtractionStep,
    verdictSchemaFor,
    verdictSlot,
    worker,
    workSummary,
} from "../sequence/family.ts";

// The repo-root task-board resource is declared directly here (via the
// shared factory), not referenced through a trait `dependency`: a
// dependency-vendored root="repo" resource loses the on-demand audit
// exemption a package's own direct declaration gets, forcing a full
// hidden-content scan of the board on every check. The factory keeps the
// declaration itself single-sourced even though each package instantiates
// its own resource node.
const taskBoard = declareTaskBoard();

const smart1 = smart1Role(
    "Strong model with research tools: researches prior art before drafting, and reviews the work in the build loop.",
);
const smart2 = smart2Role("Independent strong review model: reviews the built work each build round, separately from smart-1.");

const CROSS_REVIEWER_CONFLICT_BLOCKER =
    "Treat a work summary that reports an unresolved cross-reviewer amendment conflict as a BLOCKER in your verdict: the conflict stays open until both reviewers reconcile it in a later round, and status must not be approved while it remains.";

const researchNotes = slot.text({
    id: "research-notes",
    description:
        "Prior art and precedent gathered before the draft: how comparable tasks or the house's own history handled this problem, and any external guidance worth grounding the draft in.",
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
        "Typed park record for an unapproved run (P414): the wall citation (if any), the exact blockers, and escalation state. Present in the run's persisted output-port evidence only when the build loop exhausted without approval — the run parks and no commit is created. A dispatch-time preflight refuses a sibling task that explicitly cites the same wall-id while this stands unforced.",
    optional: true,
    value: parkReport,
    format: ["structured", "table"],
});

// implement-smart-only: the research-informed draft produce text (single
// consumer — not promoted to the shared kit or family shared.ts).
const smartDraftText = promptText(
    `Create an implementation draft for {task}, informed by your research {researchNotes}.
    Work from the task contract {taskBrief} — a verbatim copy of the task file, your scope contract. Do not re-read the board.
    FIRST verify the contract is implementable: it states a falsifiable Done-when (not a one-line placeholder or a file marked "detail pending"), it is not marked superseded or cancelled, every prerequisite task it names is already landed in this tree, and the scope honestly fits one run. If any of these fail, open the draft with "CONTRACT PROBLEM:" naming exactly what is missing or conflicting — reviewers escalate on that evidence in round 1 instead of round 10 — then still draft whatever subset is honestly implementable.
    ${SCOPE_SPLIT_DOCTRINE}
    After the SCOPE SPLIT, the draft must cover: scope (exactly what the task asks, nothing more), files to touch, approach, validation plan (the repo's standard gates), and risks.
    Explicitly call out where existing code should be REUSED or a shared abstraction EXTRACTED rather than re-implemented or copied beside. Reference repo files by path; do not inline documents. Do not implement anything.
    ${ONE_TURN_DISCIPLINE}`,
    { task, researchNotes, taskBrief },
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
        Before drafting for ${task}, research prior art: how comparable tasks in this codebase's history solved a similar problem, and any external precedent worth grounding the draft in. Use web-capable tools if available.
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
        reviewSeat(smart1, 1, SMART_VARIANT_DOCTRINE, verdict1, { extraInstruction: CROSS_REVIEWER_CONFLICT_BLOCKER }),
        reviewSeat(smart2, 2, SMART_VARIANT_DOCTRINE, verdict2, { extraInstruction: CROSS_REVIEWER_CONFLICT_BLOCKER }),
    ],
    evidence: [repoGatesStep, captureDiffStep],
    afterReview: deriveParkReportStep([verdict1, verdict2], { parkReportSlot: parkReport }),
    alsoRequire: condition.equals(repoGatesPassed.ok, true),
    carry: leftovers,
    minRounds: 3,
    rounds: 5,
    onExhausted: "block",
    // 0047 mechanism 4: a true timed-out gate is a repo condition no worker
    // round can fix — stop here rather than grinding toward a doomed park.
    stopIf: gateTimedOutStopIf,
    onStop: gateTimedOut,
});

export default variant({
    name: "Implement (Smart)",
    summary:
        "Research-informed dogfood implementation procedure: research prior art, extract the task contract from the task board, draft the approach, implement it, refine against two independent reviewers who may amend the draft, and commit.",
    metadata: { tag: ["dogfood", "implementation", "review", "multi-agent"] },
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
    resource: [taskBoard],
    signal: [gateTimedOut],
    port: [commitReport, leftoversPort, parkReportPort],
    procedure: procedure({
        description:
            "Implement one task from the task board end to end with a research-informed draft: research, extract its contract, draft the approach, implement it, refine against two independent reviewers with typed amendments, then summarize and commit.",
        sequence: [
            taskExtractionStep(clerk, taskBoard),
            // DISABLED 2026-08-01 (owner): feasibility gate + stall handoff are parked until polished — re-enable by restoring these lines.
            // feasibilityStep(smart1),
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
