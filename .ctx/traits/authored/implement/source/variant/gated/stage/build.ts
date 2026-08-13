import * as cdk from "@ctx-traits/cdk";

import { repoGatesPassed, reviewDiff } from "#trait/shared/data.ts";

import { agent } from "../agent.ts";
import { ownerBriefing, ownerDecision, plan, task, verdict, workSummary } from "../data.ts";

export const produce = cdk.stage({
  agent: agent.worker,
  input: cdk.input.prompt`
    Implement ${task} following the plan ${plan}.
    No reviewer verdict attached means this is round 1: implement the plan in full. On every later round a verdict IS attached, and it redefines your task: execute its blockers' open steps, in order, largest first. The verdict was written against the exact working tree you are in — nothing has changed since — so if it says something is missing, it is missing NOW; re-auditing the tree against the plan re-does, worse, the review that produced the verdict, and spends your round on nothing. Your own work summary from the previous round is also attached: it is your memory of what prior rounds did — trust it plus the verdict's step statuses instead of re-verifying the tree. Before claiming any step was already satisfied, run the check its blocker's done-when names and cite its passing output as evidence; extend the summary with per-step progress as you go.
    A repository gate result may be attached. When its ok field is false, re-run its argv verbatim — that exact command is what decides done-ness, whatever any document or reviewer calls the gate — and repair what it reports. Its tail is the gate's own output and states the actual reason — start there rather than guessing. A tail showing the gate could not run at all (missing recipe, command not found) is a repository misconfiguration you cannot fix from inside the run: report it in the work summary instead of attempting a code change. A gate result whose timed-out field is true is a repo condition — the ceiling is fixed in the trait — never your blocker to fix; report it and stop, do not spend the round chasing it.
    An owner decision may be attached: it is the owner's gate verdict on a previous round's briefing — always "annotated" (an approved verdict would already have ended the loop, and a dismissal halts the run before another round starts) — and its feedback text is the owner's exact words on what must change. The reviewer records an unsatisfied annotation as a typed blocker; read the feedback itself as the authoritative statement of what the owner wants.
    Run this task's own Done-when gates before reporting.
    Return the full work summary: what changed (files), how you validated it, open concerns.`,
  output: workSummary,
  include: [verdict.optional(), workSummary.optional(), repoGatesPassed.optional(), ownerDecision.optional()],
});

export const review = cdk.stage({
  agent: agent.smart,
  input: cdk.input.prompt`
    Review the work for ${task} against the plan ${plan}. Work summary: ${workSummary}.
    ${reviewDiff} lists every file changed this round with its insertion/deletion counts — an index, not the change. Open whatever it points at with your own tools; it omits untracked files. ${repoGatesPassed} is this round's gate verdict; false is grounds for a blocker on its own — UNLESS its timed-out field is true, which is a repo condition (the bound is the repository's own config), never the worker's blocker, and never grounds for a blocker on its own. This gate is the repository's PER-ROUND gate and proves only what it runs — a repo may reserve heavier proof (full integration suites) for its landing gate, which runs once at merge. Never withhold approval for a check this round's gate does not run: judge the work against the task's own Done-when and this verdict, not against a suite that runs later.
    If an owner decision is attached to this frame, it is the owner's "annotated" verdict on a previous round's briefing (an approved one would already have ended the loop; a dismissal halts the run): treat any feedback it carries as an unsatisfied BLOCKER, never advisory, until the work visibly addresses it — record it as a typed blocker like any other, and carry it forward under the same verbatim rules.
    A BLOCKER is a correctness bug, a failing gate this task's Done-when requires, clear over-build, un-abstracted duplication, or an unaddressed owner annotation. Everything else is advisory and never blocks.
    Your own verdict from last round is attached when one exists: this round's verdict EXTENDS it. Carry every open blocker's id and steps forward VERBATIM — same order, same wording; verify each open step with your own tools and flip its status to done only on evidence you confirmed, recording that evidence on the step. Append genuinely new findings as new steps at the end or as new blockers; set recurrence-of on every carried blocker; drop a blocker only when every step is done, and name the drop in the advisory. Never renumber, reword, or re-derive a carried step — a vanished or rewritten step erases the worker's progress and moves the target, which is how loops stall.
    Set status to revise while any blocker remains, approved when none do. Say "not yet" as many rounds as it takes; do not approve to end the loop, and do not invent a blocker to extend it. Approving does not end the loop by itself: it summons the owner, whose gate is the final word.
    Escalation: set escalation to needs-owner if and only if the run as a whole cannot reach an approvable state (the task file is a placeholder, lacks a falsifiable Done-when, names a prerequisite task not landed in this tree, or is marked superseded/cancelled) — never merely because one blocker is outside this round's reach. Record the one owner action that would clear it in escalation-reason; otherwise set escalation to none and leave escalation-reason empty.`,
  output: verdict,
  include: [verdict.optional(), ownerDecision.optional()],
});

export const briefing = cdk.stage({
  agent: agent.smart,
  input: cdk.input.prompt`
    The reviewer has approved this round's work for ${task} with the repository gate green, and the owner is about to be summoned to confirm it ships. Plan: ${plan}. Work summary: ${workSummary}. Reviewer verdict: ${verdict}.
    Inspect the working tree with your tools (git diff, git status, the changed files) so the briefing reflects what actually changed, not just what the summary claims.
    Write a short markdown briefing for the owner, to be read cold — the owner has not seen the working tree: what changed and why, what the review checked and found (including advisory notes worth the owner's attention), and what the owner is being asked to confirm.
    Return only the briefing markdown.`,
  output: ownerBriefing,
});
