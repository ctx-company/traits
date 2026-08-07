// Dogfood entry point for the implement family: extract, draft, build,
// refine, commit — the same spine as implement-default, with its own richer
// review rubric and the recurrence breaker's typed field wired through.
import { DEFAULT_VARIANT_DOCTRINE, LEFTOVER_DOCTRINE, reviewerVerdict, reviewVerdictSchema, REVIEW_VERDICT_DOCTRINE, SCOPE_SPLIT_DOCTRINE } from "@ctx-traits/agents";
import { condition, effect, flow, input, operation, port, procedure, resource, schema, slot, step, variant } from "@ctx-traits/cdk";
import { FAMILY_BEHAVIOR, FAMILY_INTENT } from "../core.ts";
import {
    clerk,
    commitOutput,
    commitReport,
    declareTaskBoard,
    deriveParkReportStep,
    draft,
    familyCommitTail,
    gateTimedOut,
    gateTimedOutAbortIf,
    leftovers,
    leftoversPort,
    ONE_TURN_DISCIPLINE,
    repoGatesPassed,
    reviewDiff,
    scribe,
    smart1Role,
    smart2Role,
    task,
    taskBrief,
    taskExtractionStep,
    TASK_WRITE_SCOPE_RIDER,
    worker,
    workSummary,
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

const verdictSchema = schema.object("review-verdict-default", schema.extend(reviewVerdictSchema, {
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
}));
const verdict1 = slot({ id: "review-verdict-1", schema: verdictSchema, description: "smart-1's refinement verdict for the current work state." });
const verdict2 = slot({ id: "review-verdict-2", schema: verdictSchema, description: "smart-2's refinement verdict for the current work state." });

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

const REVIEW_EXTRA_INSTRUCTION =
    `Report "recurrence-rounds" only when a carried blocker has the same still-open step statuses with zero delta from the prior verdict: 1 for a first-raised blocker, 0 when blockers is empty, and reset to 1 whenever progress changes a carried step status.`;

const draftText = input.prompt(
    `Create an implementation draft for {task}.
            Work from the task contract {taskBrief} — a verbatim copy of the task file, your scope contract. Do not re-read the board.
            FIRST verify the contract is implementable: it states a falsifiable Done-when (not a one-line placeholder or a file marked "detail pending"), it is not marked superseded or cancelled, every prerequisite task it names is already landed in this tree, and the scope honestly fits one run. If any of these fail, open the draft with "CONTRACT PROBLEM:" naming exactly what is missing or conflicting — reviewers escalate on that evidence in round 1 instead of round 10 — then still draft whatever subset is honestly implementable.
            ${SCOPE_SPLIT_DOCTRINE}
            After the SCOPE SPLIT, the draft must cover: scope (exactly what the task asks, nothing more), files to touch, approach, validation plan (the repo's standard gates), and risks.
            Explicitly call out where existing code should be REUSED or a shared abstraction EXTRACTED rather than re-implemented or copied beside. Reference repo files by path; do not inline documents. Do not implement anything.
            ${ONE_TURN_DISCIPLINE}`,
    { task, taskBrief },
);

const planReviewText = input.prompt(
    `Review the implementation draft {draft} for {task} against the task contract {taskBrief}.
            A BLOCKER is a draft that: omits an agent-doable checklist item or Done-when of the task, proposes an approach that cannot satisfy the task contract, is missing a required SCOPE SPLIT classification, or scopes in work the task does not ask for. Everything else — style, phrasing, alternate valid approaches — is advisory.
            Return the typed verdict (status: approved when no blocker remains, revise otherwise; blockers: the list that justifies revise).
            ${ONE_TURN_DISCIPLINE}`,
    { draft, task, taskBrief },
);

const produceText = input.prompt(
    `Produce the work for {task} against the draft {draft}.
            If no reviewer verdict is attached to this frame, this is round 1: implement the draft in full. If a reviewer verdict IS attached, this is a fix round: fix every BLOCKER it names that falls within this task's stated scope and Done-when — those are mandatory to merge; a blocker demanding a gate or deliverable that belongs to a later task is out of scope, note it as a follow-up and do not expand this task to satisfy it; address advisory notes only when the fix is cheap and safe. Treat any blocker with recurrence-of set as proof your prior fix treated a symptom: do the root fix that establishes the required-fix invariant — extending or widening the previously patched check is not an acceptable resolution — even when that is larger than fixing the specific cases named. Where multiple reviewers conflict, prefer correctness and the task contract.
            A leftovers list from this round's most recent review, if any, is attached as context: carry every still-valid entry into your revised "Leftovers proposed" section verbatim — never re-copy from memory. If your fixes invalidate one of those entries' evidence, say so explicitly instead of silently dropping or silently keeping it. Any new leftover your fixes surfaced is proposed separately, in the same section, for the next review round to adjudicate.
            A repository gate result from this round's most recent evidence, if any, is attached as context. When its ok field is false, re-run its argv verbatim — that exact command is what decides done-ness, whatever any document or reviewer calls the gate — and use what it reports Its tail is the gate's own output and states the actual reason — start there rather than guessing. A tail showing the gate could not run at all (missing recipe, command not found) is a repository misconfiguration you cannot fix from inside the run: report it in the work summary instead of attempting a code change. A gate result whose timed-out field is true is a repo condition — the ceiling is fixed in the trait — never your blocker to fix; report it and stop, do not spend the round chasing it.
            Respect the task contract {taskBrief}.
            You owe 100% of the draft's agent-doable pile. For each owner-only item in the draft's SCOPE SPLIT, produce the named substitute evidence (run the tests, dry runs, or static checks it names) — an owner-only classification is never license to skip work a shell can do. If you hit a wall the draft did not classify, flag it in your work summary as a proposed owner-item (item, reason class, why no in-run effort suffices, the substitute evidence you produced) and expect reviewers to challenge it.
            ${TASK_WRITE_SCOPE_RIDER}
            Run the gates named in this task's own Done-when before reporting — not whole-project gates that later tasks will satisfy.
            Propose leftovers explicitly: end the work summary with a "Leftovers proposed:" section listing any legitimate follow-on work (what — one sentence —, reason — needs-unlanded or needs-human —, needs, evidence it does not block this result standing alone, done-when), or state explicitly that none exist. Reviewers adjudicate these, not you.
            Return the full updated work summary: what changed (files), what was reused or abstracted, how it was validated, substitute evidence per owner-only item, open concerns, a "Leftovers proposed" section, and — once a verdict has ever been attached — a "Blocker dispositions" section listing every blocker id from that verdict with its done-when quoted verbatim, what changed, where, and how it was verified (or the out-of-scope justification); for a blocker with recurrence-of set, open its disposition with one paragraph stating the structural design you implemented.
            ${ONE_TURN_DISCIPLINE}`,
    { task, draft, taskBrief },
);

const review1Text = input.prompt(
    `Review the produced work for {task} against the draft {draft}. Current work summary: {workSummary}.
            {reviewDiff} lists every file changed this round with its insertion/deletion counts — an index of where the work landed, never the work itself. Open whatever it points at with your own tools; it omits untracked files. {repoGatesPassed} is this round's repository gate verdict (build, clippy, fmt, CDK typecheck); a false verdict is grounds for a blocker even if the diff otherwise looks correct. When it failed it carries a tail of the gate's own output: that tail IS the reason — cite it, and never attribute the failure to a cause it does not state. If the tail shows the gate could not run at all (a missing recipe, a command not found), that is a misconfigured repository, not a defect in the work, and it is not the worker's blocker to clear: the argv is fixed in the trait and no amount of implementation changes it. A gate whose timed-out field is true is a repo condition — the bound is the repository's own config, and the recorded reason says whether it went silent or ran past its backstop — never grounds for a blocker on its own; the loop stops on it before another round is spent. This gate is the repository's PER-ROUND gate and proves only what it runs — a repo may reserve heavier proof (full integration suites) for its landing gate, which runs once at merge. Never withhold approval for a check this round's gate does not run: judge the work against the task's own Done-when and this verdict, not against a suite that runs later. Score the work against the review rubric {reviewRubric}: report scope, correctness, and gates-ran, each as a mandatory first finding plus any further findings, every entry citing a concrete repo-relative file and line; a failed dimension is also recorded as a typed BLOCKER, never left as evidence-only.
            ${DEFAULT_VARIANT_DOCTRINE}
            ${REVIEW_EXTRA_INSTRUCTION}
            ${REVIEW_VERDICT_DOCTRINE}
            ${LEFTOVER_DOCTRINE}
            ${ONE_TURN_DISCIPLINE}`,
    { task, draft, workSummary, reviewDiff, repoGatesPassed, reviewRubric, taskBrief },
);

const review2Text = input.prompt(
    `Independently review the produced work for {task} against the draft {draft}. Current work summary: {workSummary}.
            {reviewDiff} lists every file changed this round with its insertion/deletion counts — an index of where the work landed, never the work itself. Open whatever it points at with your own tools; it omits untracked files. {repoGatesPassed} is this round's repository gate verdict (build, clippy, fmt, CDK typecheck); a false verdict is grounds for a blocker even if the diff otherwise looks correct. When it failed it carries a tail of the gate's own output: that tail IS the reason — cite it, and never attribute the failure to a cause it does not state. If the tail shows the gate could not run at all (a missing recipe, a command not found), that is a misconfigured repository, not a defect in the work, and it is not the worker's blocker to clear: the argv is fixed in the trait and no amount of implementation changes it. A gate whose timed-out field is true is a repo condition — the bound is the repository's own config, and the recorded reason says whether it went silent or ran past its backstop — never grounds for a blocker on its own; the loop stops on it before another round is spent. This gate is the repository's PER-ROUND gate and proves only what it runs — a repo may reserve heavier proof (full integration suites) for its landing gate, which runs once at merge. Never withhold approval for a check this round's gate does not run: judge the work against the task's own Done-when and this verdict, not against a suite that runs later. Score the work against the review rubric {reviewRubric}: report scope, correctness, and gates-ran, each as a mandatory first finding plus any further findings, every entry citing a concrete repo-relative file and line; a failed dimension is also recorded as a typed BLOCKER, never left as evidence-only.
            ${DEFAULT_VARIANT_DOCTRINE}
            ${REVIEW_EXTRA_INSTRUCTION}
            ${REVIEW_VERDICT_DOCTRINE}
            The first reviewer's leftovers this same round: {leftovers}. Independently reapply both questions to every entry: keeping one co-signs it into your own output leftovers list, rejecting one requires a BLOCKER instead — never silently drop or silently keep an entry.
            ${LEFTOVER_DOCTRINE}
            ${ONE_TURN_DISCIPLINE}`,
    { task, draft, workSummary, reviewDiff, repoGatesPassed, reviewRubric, taskBrief, leftovers },
);

const procedureBody = procedure.from(
    {
        description:
            "Implement one task from the task board end to end: extract its contract, draft the approach, implement it, refine against two independent reviewers until both approve, then summarize and commit — favoring the minimal, reuse-first implementation.",
    },
    () => {
        taskExtractionStep(clerk, taskBoard);
        // DISABLED 2026-08-01 (owner): feasibility gate + stall handoff are parked until polished — re-enable by restoring these lines.
        // feasibilityStep(smart1);

        const planningVerdict = slot({
            id: "planning-verdict",
            schema: reviewerVerdict,
            description: 'Reviewer verdict for the "planning" produce-review round.',
        });

        flow.loop("Planning", (loop) => {
            loop.maxIterations(2, { onExhausted: "continue" });

            smart1.prompt("Planning Produce", {
                id: "planning-produce",
                input: draftText,
                output: draft,
                include: [planningVerdict.optional(), draft.optional()],
            });

            smart2.prompt("Planning Review", {
                id: "planning-review",
                input: planReviewText,
                output: planningVerdict,
                include: [planningVerdict.optional()],
            });

            flow.until(condition.equals(planningVerdict.status, "approved"));
        });

        const sealed = slot.boolean("building-sealed");

        flow.loop("Building", (loop) => {
            loop.maxIterations(5, { onExhausted: "abort" });

            step.project("Building Unseal", { projections: [{ source: operation.literal(false), destination: sealed }] });

            worker.prompt("Building Produce", {
                id: "building-produce",
                input: produceText,
                output: workSummary,
                include: [verdict1.optional(), verdict2.optional(), workSummary.optional(), leftovers.optional(), repoGatesPassed.optional()],
            });

            step.check("Run the repository gate chain", { id: "repo-gates", argv: ["just", "test"], output: repoGatesPassed });
            step.command("Capture the changed-file inventory", {
                id: "capture-diff",
                argv: ["git", "diff", "--stat", "--", ".", ":(exclude).agents/runs"],
                output: reviewDiff,
            });

            smart1.prompt("Building Review 1", {
                id: "building-review-1",
                input: review1Text,
                output: [verdict1, leftovers],
                include: [verdict1.optional()],
            });
            smart2.prompt("Building Review 2", {
                id: "building-review-2",
                input: review2Text,
                output: [verdict2, leftovers],
                include: [verdict2.optional()],
            });

            deriveParkReportStep([verdict1, verdict2], { parkReportSlot: parkReport });

            step.project("Building Seal", { projections: [{ source: operation.literal(true), destination: sealed }] });

            flow.when("Gate Timed Out", gateTimedOutAbortIf, flow.Abort);
            effect.onAbort(gateTimedOut);

            flow.until(condition.all([
                condition.all([
                    condition.equals(verdict1.status, "approved"),
                    condition.equals(verdict2.status, "approved"),
                ]),
                condition.equals(repoGatesPassed.ok, true),
                condition.equals(sealed, true),
                condition.count(leftovers).where("needs", []).equals(0),
                condition.any([condition.count(leftovers).equals(0), condition.iterationAtLeast(2)]),
            ]));
        });

        const scribeText = input.prompt(
            `The build loop for {task} has ended in dual approval and the work is being committed. The final reviewer verdicts are {verdict1} and {verdict2} — the sole authority on what was implemented (P414: this step is unreachable while either status is still revise; that case parks instead, with a typed park report, and never reaches here).
            Write a concise commit message for the completed work based only on the task contract {taskBrief} and those verdicts: a short subject line naming the task, then a one-paragraph summary of what the verdicts certify implemented — never work belonging to other tasks.
            Return exactly that message as your output; the runtime injects it into the git commit step.
            Do not run any git commands and do not write any files for the message; staging and committing happen in later runtime steps.
            Do NOT record task status anywhere. ${TASK_WRITE_SCOPE_RIDER}
            Compose the tail of the commit message from the verdicts, including only the sections the verdicts actually support — never write a "none" placeholder for a section with no entries. If a verdict carries owner-items, add "Owner items:" quoting each entry (item, reason class, close-out command or decision). If a verdict carries remaining (cross-task seam citations only), add "Cross-task seams:" quoting it verbatim. The final adjudicated leftovers are attached as context: when non-empty, add a "Leftovers:" section reproducing every entry's typed fields (what, reason, needs, evidence, done-when) verbatim — never paraphrased, never invented; omit the section entirely when the list is empty. A non-empty leftovers list never affects approval or is written as if it were a blocker.`,
            { task, verdict1, verdict2, taskBrief },
        );

        familyCommitTail({
            id: "shipping",
            scribe: { agent: scribe, text: scribeText },
            receipt: commitOutput,
        });
    },
);

export default variant({
    name: "Implement Phase",
    summary:
        "Dogfood implementation procedure: extract the task contract from the task board, draft the approach, implement it, then a doubly-reviewed bounded refinement loop — then summarize and commit.",
    metadata: {
        tag: ["dogfood", "implementation", "review", "multi-agent"],
    },
    behavior: FAMILY_BEHAVIOR,
    intent: FAMILY_INTENT,
    resource: [taskBoard, reviewRubric],
    signal: [gateTimedOut],
    port: [commitReport, leftoversPort, parkReportPort],
    procedure: procedureBody,
});
