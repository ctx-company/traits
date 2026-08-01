import { blockerSchema, commitTail, deviationReportSchema, guardedProduction, STRICT_VARIANT_DOCTRINE } from "@ctx-traits/agents";
import { condition, intent, method, procedure, prompt, schema, slot, variant, tone, verbosity } from "@ctx-traits/cdk";
import {
    buildDraftPromptText,
    buildPlanReviewText,
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
    parkReport,
    parkReportPort,
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
    TASK_WRITE_SCOPE_RIDER,
    verdictSchemaFor,
    verdictSlot,
    worker,
    workSummary,
} from "../sequence/family.ts";

const smart1 = smart1Role("Strong model: drafts the implementation plan, and reviews the work, including every recorded deviation, in the refinement loop.");
const smart2 = smart2Role(
    "Independent strong review model: reviews the implemented work and every recorded deviation each refinement pass, separately from smart-1.",
);

// See variants/smart.ts for why this is a direct declaration (via the
// shared factory) rather than a dependency ref: a dependency-vendored
// root="repo" resource loses the on-demand audit exemption a package's own
// direct declaration gets.
const taskBoard = declareTaskBoard();

const deviationReport = slot({
    id: "deviation-report",
    schema: schema.list(deviationReportSchema),
    description:
        "Every proposed departure from the verbatim draft — including a rejected improvement the worker considered but did not make — recorded instead of silently adapted. Empty when the draft was followed exactly.",
});

const STRICT_DEVIATION_BLOCKER =
    "A BLOCKER also includes an unrecorded deviation from the draft, or any recorded deviation that reflects an actual departure rather than a rejected proposal. Approve ONLY verbatim work where every recorded deviation is a rejected proposal — never a proposal that was taken.";

const verdictSchema = verdictSchemaFor("strict");
const verdict1 = verdictSlot("review-verdict-1", "smart-1", verdictSchema);
const verdict2 = verdictSlot("review-verdict-2", "smart-2", verdictSchema);

// implement-strict-only: the produce-first merged verbatim-execution prompt
// (round 1: implement the draft verbatim; a fix round: fix blockers while
// staying verbatim, or STOP rather than actually depart). Not promoted to
// the shared kit or `implement-default/source/shared.ts` — no second
// consumer wants verbatim-or-STOP semantics.
const strictProduceText = promptText(
    `Implement {task} following the draft {draft} VERBATIM: execute exactly as specified, with no improvisation and no ACTUAL departure from what is written, however minor.
    If no reviewer verdict is attached to this frame, this is round 1. If one IS attached, this is a fix round: fix every BLOCKER it names that falls within this task's stated scope and Done-when while staying strictly verbatim to the draft — never introduce a new ACTUAL departure to satisfy a finding. If a blocker can only be fixed by actually departing from the draft, do not deviate: STOP and say so plainly in the work summary so the loop can exhaust and block rather than silently adapting.
    If the draft is unsatisfiable as written — it contradicts the actual code, omits a step you need, or cannot be executed against reality — STOP and say so plainly in your work summary rather than adapting around it; do not implement a modified version.
    If you notice a place the draft could be improved, do NOT take it: record it as a typed deviation report entry instead — what was planned, what you actually did (which must match the draft verbatim), the rationale for the proposed improvement, and disposition "rejected — implemented verbatim". A deviation report entry never records something you actually did differently from the draft; an actual departure means the draft was unsatisfiable, and that means STOP, not a recorded deviation. Any deviation report entries from a prior round, if attached as context, must be carried forward verbatim alongside any new ones this round.
    Respect the task contract {taskBrief}; do not widen any interface to make a caller compile — fix the caller.
    A leftovers list from this round's most recent review, if any, is attached as context: carry every still-valid entry into your revised "Leftovers proposed" section verbatim — never re-copy from memory. Any new leftover your fixes surfaced is proposed separately, in the same section, for the next review round to adjudicate.
    A repository gate result from this round's most recent evidence, if any, is attached as context. When its ok field is false, re-run its argv verbatim — that exact command is what decides done-ness, whatever any document or reviewer calls the gate — and use what it reports Its tail is the gate's own output and states the actual reason — start there rather than guessing. A tail showing the gate could not run at all (missing recipe, command not found) is a repository misconfiguration you cannot fix from inside the run: report it in the work summary instead of attempting a code change. A gate result whose timed-out field is true is a repo condition — the ceiling is fixed in the trait — never your blocker to fix; report it and stop, do not spend the round chasing it.
    ${TASK_WRITE_SCOPE_RIDER}
    Run the gates named in this task's own Done-when before reporting.
    Propose leftovers explicitly: end the work summary with a "Leftovers proposed" section listing any legitimate follow-on work (what — one sentence —, reason — needs-unlanded or needs-human —, needs, evidence it does not block this result standing alone, done-when), or state explicitly that none exist. Reviewers adjudicate these, not you.
    Return exactly two outputs: work-summary (what changed, files, how it was validated, open concerns, the leftovers-proposed section, and — once a verdict has ever been attached — a "Blocker dispositions" section listing every blocker id with its done-when quoted verbatim, what changed, where, and how it was verified) and deviation-report (every rejected-but-considered improvement recorded above, including any carried forward; an empty list when none arose).
    ${ONE_TURN_DISCIPLINE}`,
    { task, draft, taskBrief },
);

const planning = guardedProduction({
    id: "planning",
    produces: draft,
    produce: { agent: smart1, text: buildDraftPromptText() },
    review: { agent: smart2, text: buildPlanReviewText() },
    rounds: 2,
    onExhausted: "continue",
});
const building = guardedProduction({
    id: "building",
    produces: [workSummary, deviationReport],
    produce: {
        agent: worker,
        text: strictProduceText,
        optionalInputs: [leftovers, repoGatesPassed, deviationReport],
    },
    review: [
        reviewSeat(smart1, 1, STRICT_VARIANT_DOCTRINE, verdict1, { deviationReport, extraInstruction: STRICT_DEVIATION_BLOCKER }),
        reviewSeat(smart2, 2, STRICT_VARIANT_DOCTRINE, verdict2, { deviationReport, extraInstruction: STRICT_DEVIATION_BLOCKER }),
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
    name: "Implement (Strict)",
    summary:
        "Verbatim dogfood implementation procedure: extract the task contract from the task board, draft the approach, implement the draft exactly as written, refine against two independent reviewers with a typed deviation record, and commit only once both approve.",
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
            { id: "verbatim-draft-execution", summary: "Execute the agreed draft exactly as written; record every departure instead of silently adapting." },
        ],
        avoid: [
            intent.avoid.accretion,
            intent.avoid.overEngineering,
            intent.avoid.goldPlating,
            intent.avoid.duplication,
            intent.avoid.scopeCreep,
            intent.avoid.unboundedLoop,
            intent.avoid.rubberStampReview,
            { id: "silent-plan-deviation", summary: "Never adapt around an unsatisfiable or contradicted draft without recording and disposing of the deviation." },
        ],
    },
    schema: [blockerSchema, ownerItemSchema],
    resource: [taskBoard],
    signal: [gateTimedOut],
    procedure: procedure({
        description:
            "Implement one task from the task board end to end with verbatim draft execution: extract its contract, draft the approach, implement it exactly as written with a typed deviation record, refine against two independent reviewers, commit — blocking rather than adapting when the draft is unsatisfiable.",
        input: task,
        output: [commitReport, leftoversPort, parkReportPort],
        sequence: [
            taskExtractionStep(clerk, taskBoard),
            // DISABLED 2026-08-01 (owner): feasibility gate + stall handoff are parked until polished — re-enable by restoring these lines.
            // feasibilityStep(smart1),
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
