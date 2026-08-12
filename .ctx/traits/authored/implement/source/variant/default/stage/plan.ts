import { SCOPE_SPLIT_DOCTRINE } from "@ctx-traits/agents";
import * as cdk from "@ctx-traits/cdk";

import { draft, ONE_TURN_DISCIPLINE, task, taskBrief } from "#trait/shared/data.ts";

import { smart1, smart2 } from "../agent.ts";
import { planningVerdict } from "../data.ts";

export const produce = cdk.stage({
  agent: smart1,
  input: cdk.input.prompt(
    `Create an implementation draft for {task}.
            Work from the task contract {taskBrief} — a verbatim copy of the task file, your scope contract. Do not re-read the board.
            FIRST verify the contract is implementable: it states a falsifiable Done-when (not a one-line placeholder or a file marked "detail pending"), it is not marked superseded or cancelled, every prerequisite task it names is already landed in this tree, and the scope honestly fits one run. If any of these fail, open the draft with "CONTRACT PROBLEM:" naming exactly what is missing or conflicting — reviewers escalate on that evidence in round 1 instead of round 10 — then still draft whatever subset is honestly implementable.
            ${SCOPE_SPLIT_DOCTRINE}
            After the SCOPE SPLIT, the draft must cover: scope (exactly what the task asks, nothing more), files to touch, approach, validation plan (the repo's standard gates), and risks.
            Explicitly call out where existing code should be REUSED or a shared abstraction EXTRACTED rather than re-implemented or copied beside. Reference repo files by path; do not inline documents. Do not implement anything.
            ${ONE_TURN_DISCIPLINE}`,
    { task, taskBrief },
  ),
  output: draft,
  include: [planningVerdict.optional(), draft.optional()],
});

export const review = cdk.stage({
  agent: smart2,
  input: cdk.input.prompt(
    `Review the implementation draft {draft} for {task} against the task contract {taskBrief}.
            A BLOCKER is a draft that: omits an agent-doable checklist item or Done-when of the task, proposes an approach that cannot satisfy the task contract, is missing a required SCOPE SPLIT classification, or scopes in work the task does not ask for. Everything else — style, phrasing, alternate valid approaches — is advisory.
            Return the typed verdict (status: approved when no blocker remains, revise otherwise; blockers: the list that justifies revise).
            ${ONE_TURN_DISCIPLINE}`,
    { draft, task, taskBrief },
  ),
  output: planningVerdict,
  include: [planningVerdict.optional()],
});
