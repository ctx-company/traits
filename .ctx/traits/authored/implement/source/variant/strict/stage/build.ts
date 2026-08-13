import { LEFTOVER_DOCTRINE, REVIEW_VERDICT_DOCTRINE, STRICT_VARIANT_DOCTRINE } from "@ctx-traits/agents";
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
import { deviationReport, verdict1, verdict2 } from "../data.ts";

const STRICT_DEVIATION_BLOCKER =
  "A BLOCKER also includes an unrecorded deviation from the draft, or any recorded deviation that reflects an actual departure rather than a rejected proposal. Approve ONLY verbatim work where every recorded deviation is a rejected proposal — never a proposal that was taken.";

export const produce = cdk.stage({
  agent: worker,
  input: cdk.input.prompt(
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
  ),
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

export const review1 = cdk.stage({
  agent: smart1,
  input: cdk.input.prompt(
    `Review the produced work for {task} against the draft {draft}. Current work summary: {workSummary}.
            {reviewDiff} lists every file changed this round with its insertion/deletion counts — an index of where the work landed, never the work itself. Open whatever it points at with your own tools; it omits untracked files. {repoGatesPassed} is this round's repository gate verdict (build, clippy, fmt, CDK typecheck); a false verdict is grounds for a blocker even if the diff otherwise looks correct. When it failed it carries a tail of the gate's own output: that tail IS the reason — cite it, and never attribute the failure to a cause it does not state. If the tail shows the gate could not run at all (a missing recipe, a command not found), that is a misconfigured repository, not a defect in the work, and it is not the worker's blocker to clear: the argv is fixed in the trait and no amount of implementation changes it. A gate whose timed-out field is true is a repo condition — the bound is the repository's own config, and the recorded reason says whether it went silent or ran past its backstop — never grounds for a blocker on its own; the loop stops on it before another round is spent. This gate is the repository's PER-ROUND gate and proves only what it runs — a repo may reserve heavier proof (full integration suites) for its landing gate, which runs once at merge. Never withhold approval for a check this round's gate does not run: judge the work against the task's own Done-when and this verdict, not against a suite that runs later. Recorded deviations: {deviationReport}. Verify every recorded deviation is a genuinely rejected-but-considered proposal — the actual implementation still matches the draft verbatim on that point — not a departure that was actually taken; silent adaptation not recorded here is itself a BLOCKER.
            ${STRICT_VARIANT_DOCTRINE}
            ${STRICT_DEVIATION_BLOCKER}
            ${REVIEW_VERDICT_DOCTRINE}
            ${LEFTOVER_DOCTRINE}
            ${ONE_TURN_DISCIPLINE}`,
    { task, draft, workSummary, reviewDiff, repoGatesPassed, deviationReport, taskBrief },
  ),
  output: [verdict1, leftovers],
  include: [verdict1.optional()],
});

export const review2 = cdk.stage({
  agent: smart2,
  input: cdk.input.prompt(
    `Independently review the produced work for {task} against the draft {draft}. Current work summary: {workSummary}.
            {reviewDiff} lists every file changed this round with its insertion/deletion counts — an index of where the work landed, never the work itself. Open whatever it points at with your own tools; it omits untracked files. {repoGatesPassed} is this round's repository gate verdict (build, clippy, fmt, CDK typecheck); a false verdict is grounds for a blocker even if the diff otherwise looks correct. When it failed it carries a tail of the gate's own output: that tail IS the reason — cite it, and never attribute the failure to a cause it does not state. If the tail shows the gate could not run at all (a missing recipe, a command not found), that is a misconfigured repository, not a defect in the work, and it is not the worker's blocker to clear: the argv is fixed in the trait and no amount of implementation changes it. A gate whose timed-out field is true is a repo condition — the bound is the repository's own config, and the recorded reason says whether it went silent or ran past its backstop — never grounds for a blocker on its own; the loop stops on it before another round is spent. This gate is the repository's PER-ROUND gate and proves only what it runs — a repo may reserve heavier proof (full integration suites) for its landing gate, which runs once at merge. Never withhold approval for a check this round's gate does not run: judge the work against the task's own Done-when and this verdict, not against a suite that runs later. Recorded deviations: {deviationReport}. Verify every recorded deviation is a genuinely rejected-but-considered proposal — the actual implementation still matches the draft verbatim on that point — not a departure that was actually taken; silent adaptation not recorded here is itself a BLOCKER.
            ${STRICT_VARIANT_DOCTRINE}
            ${STRICT_DEVIATION_BLOCKER}
            ${REVIEW_VERDICT_DOCTRINE}
            The first reviewer's leftovers this same round: {leftovers}. Independently reapply both questions to every entry: keeping one co-signs it into your own output leftovers list, rejecting one requires a BLOCKER instead — never silently drop or silently keep an entry.
            ${LEFTOVER_DOCTRINE}
            ${ONE_TURN_DISCIPLINE}`,
    { task, draft, workSummary, reviewDiff, repoGatesPassed, deviationReport, taskBrief, leftovers },
  ),
  output: [verdict2, leftovers],
  include: [verdict2.optional()],
});
