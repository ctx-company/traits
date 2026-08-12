import {
  deviationReportSchema,
  LEFTOVER_DOCTRINE,
  reviewerVerdict,
  reviewVerdictSchema,
  REVIEW_VERDICT_DOCTRINE,
  SCOPE_SPLIT_DOCTRINE,
  STRICT_VARIANT_DOCTRINE,
} from "@ctx-traits/agents";
import { condition, effect, flow, input, operation, procedure, schema, slot, step, variant } from "@ctx-traits/cdk";
import { FAMILY_BEHAVIOR, STRICT_INTENT } from "../core.ts";
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
  parkReport,
  parkReportPort,
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

const smart1 = smart1Role(
  "Strong model: drafts the implementation plan, and reviews the work, including every recorded deviation, in the refinement loop.",
);
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

const verdict1 = slot({
  id: "review-verdict-1",
  schema: reviewVerdictSchema,
  description: "smart-1's refinement verdict for the current work state.",
});
const verdict2 = slot({
  id: "review-verdict-2",
  schema: reviewVerdictSchema,
  description: "smart-2's refinement verdict for the current work state.",
});

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

// implement-strict-only: the produce-first merged verbatim-execution prompt
// (round 1: implement the draft verbatim; a fix round: fix blockers while
// staying verbatim, or STOP rather than actually depart). Not promoted to
// the shared kit or family.ts — no second consumer wants verbatim-or-STOP
// semantics.
const strictProduceText = input.prompt(
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

const review1Text = input.prompt(
  `Review the produced work for {task} against the draft {draft}. Current work summary: {workSummary}.
            {reviewDiff} lists every file changed this round with its insertion/deletion counts — an index of where the work landed, never the work itself. Open whatever it points at with your own tools; it omits untracked files. {repoGatesPassed} is this round's repository gate verdict (build, clippy, fmt, CDK typecheck); a false verdict is grounds for a blocker even if the diff otherwise looks correct. When it failed it carries a tail of the gate's own output: that tail IS the reason — cite it, and never attribute the failure to a cause it does not state. If the tail shows the gate could not run at all (a missing recipe, a command not found), that is a misconfigured repository, not a defect in the work, and it is not the worker's blocker to clear: the argv is fixed in the trait and no amount of implementation changes it. A gate whose timed-out field is true is a repo condition — the bound is the repository's own config, and the recorded reason says whether it went silent or ran past its backstop — never grounds for a blocker on its own; the loop stops on it before another round is spent. This gate is the repository's PER-ROUND gate and proves only what it runs — a repo may reserve heavier proof (full integration suites) for its landing gate, which runs once at merge. Never withhold approval for a check this round's gate does not run: judge the work against the task's own Done-when and this verdict, not against a suite that runs later. Recorded deviations: {deviationReport}. Verify every recorded deviation is a genuinely rejected-but-considered proposal — the actual implementation still matches the draft verbatim on that point — not a departure that was actually taken; silent adaptation not recorded here is itself a BLOCKER.
            ${STRICT_VARIANT_DOCTRINE}
            ${STRICT_DEVIATION_BLOCKER}
            ${REVIEW_VERDICT_DOCTRINE}
            ${LEFTOVER_DOCTRINE}
            ${ONE_TURN_DISCIPLINE}`,
  { task, draft, workSummary, reviewDiff, repoGatesPassed, deviationReport, taskBrief },
);

const review2Text = input.prompt(
  `Independently review the produced work for {task} against the draft {draft}. Current work summary: {workSummary}.
            {reviewDiff} lists every file changed this round with its insertion/deletion counts — an index of where the work landed, never the work itself. Open whatever it points at with your own tools; it omits untracked files. {repoGatesPassed} is this round's repository gate verdict (build, clippy, fmt, CDK typecheck); a false verdict is grounds for a blocker even if the diff otherwise looks correct. When it failed it carries a tail of the gate's own output: that tail IS the reason — cite it, and never attribute the failure to a cause it does not state. If the tail shows the gate could not run at all (a missing recipe, a command not found), that is a misconfigured repository, not a defect in the work, and it is not the worker's blocker to clear: the argv is fixed in the trait and no amount of implementation changes it. A gate whose timed-out field is true is a repo condition — the bound is the repository's own config, and the recorded reason says whether it went silent or ran past its backstop — never grounds for a blocker on its own; the loop stops on it before another round is spent. This gate is the repository's PER-ROUND gate and proves only what it runs — a repo may reserve heavier proof (full integration suites) for its landing gate, which runs once at merge. Never withhold approval for a check this round's gate does not run: judge the work against the task's own Done-when and this verdict, not against a suite that runs later. Recorded deviations: {deviationReport}. Verify every recorded deviation is a genuinely rejected-but-considered proposal — the actual implementation still matches the draft verbatim on that point — not a departure that was actually taken; silent adaptation not recorded here is itself a BLOCKER.
            ${STRICT_VARIANT_DOCTRINE}
            ${STRICT_DEVIATION_BLOCKER}
            ${REVIEW_VERDICT_DOCTRINE}
            The first reviewer's leftovers this same round: {leftovers}. Independently reapply both questions to every entry: keeping one co-signs it into your own output leftovers list, rejecting one requires a BLOCKER instead — never silently drop or silently keep an entry.
            ${LEFTOVER_DOCTRINE}
            ${ONE_TURN_DISCIPLINE}`,
  { task, draft, workSummary, reviewDiff, repoGatesPassed, deviationReport, taskBrief, leftovers },
);

const procedureBody = procedure.from(
  {
    description:
      "Implement one task from the task board end to end with verbatim draft execution: extract its contract, draft the approach, implement it exactly as written with a typed deviation record, refine against two independent reviewers, commit — blocking rather than adapting when the draft is unsatisfiable.",
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
        input: draftText,
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
        input: strictProduceText,
        output: [workSummary, deviationReport],
        include: [
          verdict1.optional(),
          verdict2.optional(),
          workSummary.optional(),
          deviationReport.optional(),
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

      flow.until(
        condition.all([
          condition.all([condition.equals(verdict1.status, "approved"), condition.equals(verdict2.status, "approved")]),
          condition.equals(repoGatesPassed.ok, true),
          condition.equals(sealed, true),
          condition.count(leftovers).where("needs", []).equals(0),
          condition.any([condition.count(leftovers).equals(0), condition.iterationAtLeast(2)]),
        ]),
      );
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
  name: "Implement (Strict)",
  summary:
    "Verbatim dogfood implementation procedure: extract the task contract from the task board, draft the approach, implement the draft exactly as written, refine against two independent reviewers with a typed deviation record, and commit only once both approve.",
  metadata: { tag: ["dogfood", "implementation", "review", "multi-agent"] },
  behavior: FAMILY_BEHAVIOR,
  intent: STRICT_INTENT,
  resource: [taskBoard],
  signal: [gateTimedOut],
  port: [commitReport, leftoversPort, parkReportPort],
  procedure: procedureBody,
});
