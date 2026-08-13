import { LEFTOVER_DOCTRINE, REVIEW_VERDICT_DOCTRINE, SMART_VARIANT_DOCTRINE } from "@ctx-traits/agents";
import * as cdk from "@ctx-traits/cdk";

import {
  draft,
  leftovers,
  ONE_TURN_DISCIPLINE,
  repoGatesPassed,
  reviewDiff,
  task,
  taskBrief,
  TASK_WRITE_SCOPE_RIDER,
  workSummary,
} from "#trait/shared/data.ts";

import { smart1, smart2, worker } from "../agent.ts";
import { verdict1, verdict2 } from "../data.ts";

const CROSS_REVIEWER_CONFLICT_BLOCKER =
  "Treat a work summary that reports an unresolved cross-reviewer amendment conflict as a BLOCKER in your verdict: the conflict stays open until both reviewers reconcile it in a later round, and status must not be approved while it remains.";

const SMART_AMENDMENT_FIX_INSTRUCTION =
  "When a verdict carries amendments, those become the new binding contract: rewrite the draft to incorporate them exactly, then implement to the amended draft. If verdict1 and verdict2 propose amendments that genuinely conflict with each other, do NOT adjudicate between them yourself: leave the draft unamended on the conflicting point, implement neither side of the conflict, and report the conflict verbatim (both proposed amendments and why they contradict) as an open item in the work summary — the conflict must remain unresolved and blocking until the reviewers reconcile it in a later round, never silently decided here.";
const SMART_PRODUCE_RETURN_CONTRACT =
  'Return exactly two outputs: draft (the complete draft text, amended by every non-conflicting amendment, unchanged on any conflicting point) and work-summary (what changed, files, how it was validated, open concerns, a "Leftovers proposed" section (explicit list or explicit none) — including any unresolved amendment conflict, named explicitly so the next review round blocks on it, and — once a verdict has ever been attached — a "Blocker dispositions" section as in a plain build round).';

export const produce = cdk.stage({
  agent: worker,
  input: cdk.input.prompt(
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
  ),
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

export const review1 = cdk.stage({
  agent: smart1,
  input: cdk.input.prompt(
    `Review the produced work for {task} against the draft {draft}. Current work summary: {workSummary}.
            {reviewDiff} lists every file changed this round with its insertion/deletion counts — an index of where the work landed, never the work itself. Open whatever it points at with your own tools; it omits untracked files. {repoGatesPassed} is this round's repository gate verdict (build, clippy, fmt, CDK typecheck); a false verdict is grounds for a blocker even if the diff otherwise looks correct. When it failed it carries a tail of the gate's own output: that tail IS the reason — cite it, and never attribute the failure to a cause it does not state. If the tail shows the gate could not run at all (a missing recipe, a command not found), that is a misconfigured repository, not a defect in the work, and it is not the worker's blocker to clear: the argv is fixed in the trait and no amount of implementation changes it. A gate whose timed-out field is true is a repo condition — the bound is the repository's own config, and the recorded reason says whether it went silent or ran past its backstop — never grounds for a blocker on its own; the loop stops on it before another round is spent. This gate is the repository's PER-ROUND gate and proves only what it runs — a repo may reserve heavier proof (full integration suites) for its landing gate, which runs once at merge. Never withhold approval for a check this round's gate does not run: judge the work against the task's own Done-when and this verdict, not against a suite that runs later.
            ${SMART_VARIANT_DOCTRINE}
            ${CROSS_REVIEWER_CONFLICT_BLOCKER}
            ${REVIEW_VERDICT_DOCTRINE}
            ${LEFTOVER_DOCTRINE}
            ${ONE_TURN_DISCIPLINE}`,
    { task, draft, workSummary, reviewDiff, repoGatesPassed, taskBrief },
  ),
  output: [verdict1, leftovers],
  include: [verdict1.optional()],
});

export const review2 = cdk.stage({
  agent: smart2,
  input: cdk.input.prompt(
    `Independently review the produced work for {task} against the draft {draft}. Current work summary: {workSummary}.
            {reviewDiff} lists every file changed this round with its insertion/deletion counts — an index of where the work landed, never the work itself. Open whatever it points at with your own tools; it omits untracked files. {repoGatesPassed} is this round's repository gate verdict (build, clippy, fmt, CDK typecheck); a false verdict is grounds for a blocker even if the diff otherwise looks correct. When it failed it carries a tail of the gate's own output: that tail IS the reason — cite it, and never attribute the failure to a cause it does not state. If the tail shows the gate could not run at all (a missing recipe, a command not found), that is a misconfigured repository, not a defect in the work, and it is not the worker's blocker to clear: the argv is fixed in the trait and no amount of implementation changes it. A gate whose timed-out field is true is a repo condition — the bound is the repository's own config, and the recorded reason says whether it went silent or ran past its backstop — never grounds for a blocker on its own; the loop stops on it before another round is spent. This gate is the repository's PER-ROUND gate and proves only what it runs — a repo may reserve heavier proof (full integration suites) for its landing gate, which runs once at merge. Never withhold approval for a check this round's gate does not run: judge the work against the task's own Done-when and this verdict, not against a suite that runs later.
            ${SMART_VARIANT_DOCTRINE}
            ${CROSS_REVIEWER_CONFLICT_BLOCKER}
            ${REVIEW_VERDICT_DOCTRINE}
            The first reviewer's leftovers this same round: {leftovers}. Independently reapply both questions to every entry: keeping one co-signs it into your own output leftovers list, rejecting one requires a BLOCKER instead — never silently drop or silently keep an entry.
            ${LEFTOVER_DOCTRINE}
            ${ONE_TURN_DISCIPLINE}`,
    { task, draft, workSummary, reviewDiff, repoGatesPassed, taskBrief, leftovers },
  ),
  output: [verdict2, leftovers],
  include: [verdict2.optional()],
});
