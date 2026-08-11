import { LEFTOVER_DOCTRINE, planAmendmentSchema, reviewerVerdict, reviewVerdictSchema, REVIEW_VERDICT_DOCTRINE, SCOPE_SPLIT_DOCTRINE, SMART_VARIANT_DOCTRINE } from "@ctx-traits/agents";
import { condition, effect, flow, input, operation, port, procedure, schema, slot, step, variant } from "@ctx-traits/cdk";
import { FAMILY_BEHAVIOR, FAMILY_INTENT } from "../core.ts";
import {
    clerk,
    commitOutput,
    commitReport,
    declareTaskBoard,
    deriveParkReportStep,
    draft,
    familyCommitTail,
    gateAndDiffEvidence,
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

const verdictSchema = schema.object("review-verdict-smart", schema.extend(reviewVerdictSchema, {
    amendments: schema.field(schema.list(planAmendmentSchema), {
        required: false,
        description:
            "Typed amendments to the agreed draft under smart authority, present when the draft and the observed work genuinely contradicted. Each becomes the binding contract going forward; an unrecorded departure is still a plan-fidelity gap, not an amendment.",
    }),
}));
const verdict1 = slot({ id: "review-verdict-1", schema: verdictSchema, description: "smart-1's refinement verdict for the current work state." });
const verdict2 = slot({ id: "review-verdict-2", schema: verdictSchema, description: "smart-2's refinement verdict for the current work state." });

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

const researchStepText = input.prompt`
        Before drafting for ${task}, research prior art: how comparable tasks in this codebase's history solved a similar problem, and any external precedent worth grounding the draft in. Use web-capable tools if available.
        Keep this to one pass — the loop budget is unchanged from the default flow; smart means a better-informed draft, not more rounds.
        Return your research notes: what you found, its relevance, and anything that should constrain the coming draft.`;

// implement-smart-only: the research-informed draft produce text (single
// consumer — not promoted to the shared kit or family.ts).
const smartDraftText = input.prompt(
    `Create an implementation draft for {task}, informed by your research {researchNotes}.
    Work from the task contract {taskBrief} — a verbatim copy of the task file, your scope contract. Do not re-read the board.
    FIRST verify the contract is implementable: it states a falsifiable Done-when (not a one-line placeholder or a file marked "detail pending"), it is not marked superseded or cancelled, every prerequisite task it names is already landed in this tree, and the scope honestly fits one run. If any of these fail, open the draft with "CONTRACT PROBLEM:" naming exactly what is missing or conflicting — reviewers escalate on that evidence in round 1 instead of round 10 — then still draft whatever subset is honestly implementable.
    ${SCOPE_SPLIT_DOCTRINE}
    After the SCOPE SPLIT, the draft must cover: scope (exactly what the task asks, nothing more), files to touch, approach, validation plan (the repo's standard gates), and risks.
    Explicitly call out where existing code should be REUSED or a shared abstraction EXTRACTED rather than re-implemented or copied beside. Reference repo files by path; do not inline documents. Do not implement anything.
    ${ONE_TURN_DISCIPLINE}`,
    { task, researchNotes, taskBrief },
);

const planReviewText = input.prompt(
    `Review the implementation draft {draft} for {task} against the task contract {taskBrief}.
            A BLOCKER is a draft that: omits an agent-doable checklist item or Done-when of the task, proposes an approach that cannot satisfy the task contract, is missing a required SCOPE SPLIT classification, or scopes in work the task does not ask for. Everything else — style, phrasing, alternate valid approaches — is advisory.
            Return the typed verdict (status: approved when no blocker remains, revise otherwise; blockers: the list that justifies revise).
            ${ONE_TURN_DISCIPLINE}`,
    { draft, task, taskBrief },
);

// implement-smart-only: the amendment-conflict extension to the shared
// produce prompt and its own return contract (the produce step returns
// `draft` + `work-summary`, not just `work-summary`) — everything else is
// the shared paragraph text.
const SMART_AMENDMENT_FIX_INSTRUCTION =
    "When a verdict carries amendments, those become the new binding contract: rewrite the draft to incorporate them exactly, then implement to the amended draft. If verdict1 and verdict2 propose amendments that genuinely conflict with each other, do NOT adjudicate between them yourself: leave the draft unamended on the conflicting point, implement neither side of the conflict, and report the conflict verbatim (both proposed amendments and why they contradict) as an open item in the work summary — the conflict must remain unresolved and blocking until the reviewers reconcile it in a later round, never silently decided here.";
const SMART_PRODUCE_RETURN_CONTRACT =
    'Return exactly two outputs: draft (the complete draft text, amended by every non-conflicting amendment, unchanged on any conflicting point) and work-summary (what changed, files, how it was validated, open concerns, a "Leftovers proposed" section (explicit list or explicit none) — including any unresolved amendment conflict, named explicitly so the next review round blocks on it, and — once a verdict has ever been attached — a "Blocker dispositions" section as in a plain build round).';

const smartProduceText = input.prompt(
    `Produce the work for {task} against the draft {draft}.
            If no reviewer verdict is attached to this frame, this is round 1: implement the draft in full. If a reviewer verdict IS attached, this is a fix round: fix every BLOCKER it names that falls within this task's stated scope and Done-when — those are mandatory to merge; a blocker demanding a gate or deliverable that belongs to a later task is out of scope, note it as a follow-up and do not expand this task to satisfy it; address advisory notes only when the fix is cheap and safe. Treat any blocker with recurrence-of set as proof your prior fix treated a symptom: do the root fix that establishes the required-fix invariant — extending or widening the previously patched check is not an acceptable resolution — even when that is larger than fixing the specific cases named. Where multiple reviewers conflict, prefer correctness and the task contract. ${SMART_AMENDMENT_FIX_INSTRUCTION}
            A leftovers list from this round's most recent review, if any, is attached as context: carry every still-valid entry into your revised "Leftovers proposed" section verbatim — never re-copy from memory. If your fixes invalidate one of those entries' evidence, say so explicitly instead of silently dropping or silently keeping it. Any new leftover your fixes surfaced is proposed separately, in the same section, for the next review round to adjudicate.
            A repository gate result from this round's most recent evidence, if any, is attached as context. When its ok field is false, re-run its argv verbatim — that exact command is what decides done-ness, whatever any document or reviewer calls the gate — and use what it reports Its tail is the gate's own output and states the actual reason — start there rather than guessing. A tail showing the gate could not run at all (missing recipe, command not found) is a repository misconfiguration you cannot fix from inside the run: report it in the work summary instead of attempting a code change. A gate result whose timed-out field is true is a repo condition — the ceiling is fixed in the trait — never your blocker to fix; report it and stop, do not spend the round chasing it.
            Respect the task contract {taskBrief}.
            You owe 100% of the draft's agent-doable pile. For each owner-only item in the draft's SCOPE SPLIT, produce the named substitute evidence (run the tests, dry runs, or static checks it names) — an owner-only classification is never license to skip work a shell can do. If you hit a wall the draft did not classify, flag it in your work summary as a proposed owner-item (item, reason class, why no in-run effort suffices, the substitute evidence you produced) and expect reviewers to challenge it.
            ${TASK_WRITE_SCOPE_RIDER}
            Run the gates named in this task's own Done-when before reporting — not whole-project gates that later tasks will satisfy.
            Propose leftovers explicitly: end the work summary with a "Leftovers proposed:" section listing any legitimate follow-on work (what — one sentence —, reason — needs-unlanded or needs-human —, needs, evidence it does not block this result standing alone, done-when), or state explicitly that none exist. Reviewers adjudicate these, not you.
            ${SMART_PRODUCE_RETURN_CONTRACT}
            ${ONE_TURN_DISCIPLINE}`,
    { task, draft, taskBrief },
);

const review1Text = input.prompt(
    `Review the produced work for {task} against the draft {draft}. Current work summary: {workSummary}.
            {reviewDiff} lists every file changed this round with its insertion/deletion counts — an index of where the work landed, never the work itself. Open whatever it points at with your own tools; it omits untracked files. {repoGatesPassed} is this round's repository gate verdict (build, clippy, fmt, CDK typecheck); a false verdict is grounds for a blocker even if the diff otherwise looks correct. When it failed it carries a tail of the gate's own output: that tail IS the reason — cite it, and never attribute the failure to a cause it does not state. If the tail shows the gate could not run at all (a missing recipe, a command not found), that is a misconfigured repository, not a defect in the work, and it is not the worker's blocker to clear: the argv is fixed in the trait and no amount of implementation changes it. A gate whose timed-out field is true is a repo condition — the bound is the repository's own config, and the recorded reason says whether it went silent or ran past its backstop — never grounds for a blocker on its own; the loop stops on it before another round is spent. This gate is the repository's PER-ROUND gate and proves only what it runs — a repo may reserve heavier proof (full integration suites) for its landing gate, which runs once at merge. Never withhold approval for a check this round's gate does not run: judge the work against the task's own Done-when and this verdict, not against a suite that runs later.
            ${SMART_VARIANT_DOCTRINE}
            ${CROSS_REVIEWER_CONFLICT_BLOCKER}
            ${REVIEW_VERDICT_DOCTRINE}
            ${LEFTOVER_DOCTRINE}
            ${ONE_TURN_DISCIPLINE}`,
    { task, draft, workSummary, reviewDiff, repoGatesPassed, taskBrief },
);

const review2Text = input.prompt(
    `Independently review the produced work for {task} against the draft {draft}. Current work summary: {workSummary}.
            {reviewDiff} lists every file changed this round with its insertion/deletion counts — an index of where the work landed, never the work itself. Open whatever it points at with your own tools; it omits untracked files. {repoGatesPassed} is this round's repository gate verdict (build, clippy, fmt, CDK typecheck); a false verdict is grounds for a blocker even if the diff otherwise looks correct. When it failed it carries a tail of the gate's own output: that tail IS the reason — cite it, and never attribute the failure to a cause it does not state. If the tail shows the gate could not run at all (a missing recipe, a command not found), that is a misconfigured repository, not a defect in the work, and it is not the worker's blocker to clear: the argv is fixed in the trait and no amount of implementation changes it. A gate whose timed-out field is true is a repo condition — the bound is the repository's own config, and the recorded reason says whether it went silent or ran past its backstop — never grounds for a blocker on its own; the loop stops on it before another round is spent. This gate is the repository's PER-ROUND gate and proves only what it runs — a repo may reserve heavier proof (full integration suites) for its landing gate, which runs once at merge. Never withhold approval for a check this round's gate does not run: judge the work against the task's own Done-when and this verdict, not against a suite that runs later.
            ${SMART_VARIANT_DOCTRINE}
            ${CROSS_REVIEWER_CONFLICT_BLOCKER}
            ${REVIEW_VERDICT_DOCTRINE}
            The first reviewer's leftovers this same round: {leftovers}. Independently reapply both questions to every entry: keeping one co-signs it into your own output leftovers list, rejecting one requires a BLOCKER instead — never silently drop or silently keep an entry.
            ${LEFTOVER_DOCTRINE}
            ${ONE_TURN_DISCIPLINE}`,
    { task, draft, workSummary, reviewDiff, repoGatesPassed, taskBrief, leftovers },
);

const procedureBody = procedure.from(
    {
        description:
            "Implement one task from the task board end to end with a research-informed draft: research, extract its contract, draft the approach, implement it, refine against two independent reviewers with typed amendments, then summarize and commit.",
    },
    () => {
        taskExtractionStep(clerk, taskBoard);
        // DISABLED 2026-08-01 (owner): feasibility gate + stall handoff are parked until polished — re-enable by restoring these lines.
        // feasibilityStep(smart1);

        smart1.prompt("Research prior art (smart-1)", { id: "research", input: researchStepText, output: researchNotes });

        const planningVerdict = slot({
            id: "planning-verdict",
            schema: reviewerVerdict,
            description: 'Reviewer verdict for the "planning" produce-review round.',
        });

        flow.loop("Planning", (loop) => {
            loop.maxIterations(2, { onExhausted: "continue" });

            smart1.prompt("Planning Produce", {
                input: smartDraftText,
                output: draft,
                include: [planningVerdict.optional(), draft.optional()],
            });

            smart2.prompt("Planning Review", {
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
                input: smartProduceText,
                output: [draft, workSummary],
                include: [
                    verdict1.optional(),
                    verdict2.optional(),
                    draft.optional(),
                    workSummary.optional(),
                    leftovers.optional(),
                    repoGatesPassed.optional(),
                ],
            });

            gateAndDiffEvidence({ gatePassed: repoGatesPassed, diff: reviewDiff });

            smart1.prompt("Building Review 1", {
                input: review1Text,
                output: [verdict1, leftovers],
                include: [verdict1.optional()],
            });
            smart2.prompt("Building Review 2", {
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
    name: "Implement (Smart)",
    summary:
        "Research-informed dogfood implementation procedure: research prior art, extract the task contract from the task board, draft the approach, implement it, refine against two independent reviewers who may amend the draft, and commit.",
    metadata: { tag: ["dogfood", "implementation", "review", "multi-agent"] },
    behavior: FAMILY_BEHAVIOR,
    intent: FAMILY_INTENT,
    resource: [taskBoard],
    signal: [gateTimedOut],
    port: [commitReport, leftoversPort, parkReportPort],
    procedure: procedureBody,
});
