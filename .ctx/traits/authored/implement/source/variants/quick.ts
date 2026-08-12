// implement-quick: draft, then grind worker -> reviewer until approved, then
// commit. Ten rounds; exhausting them without approval blocks the run and
// commits nothing.
//
// DELIBERATELY LEAN (2026-07-27). This variant is called quick and now reads
// like it: no recurrence breaker, no earned-exit carry/min-rounds, no
// leftovers adjudication, no spliced doctrine blocks. Every one of those was
// a knob that interacted badly with the others — the measured failures were
// a breaker killing a converging loop, an earned-exit floor stacked on top
// of a fix loop, and prompts long enough that the actual instruction was
// buried. The loop IS the mechanism: the reviewer says "not yet" and the
// worker goes again, until it is satisfied or the budget is gone. The
// park-report projection (0047) is the one addition since: a deterministic,
// zero-prompt `project` step, never a spliced doctrine block, so it stays
// lean by this same rule.
//
// Functional authoring (0109): ported from the 0108 pilot
// (variants/quick-functional.ts, now retired) with the F2/F3 gaps it
// recorded resolved — the seven baseline hand-chosen ids restored via the
// registrars' `id:` override, and the park-report triad restored via
// `step.project`.
import { condition, effect, flow, input, procedure, step, variant } from "@ctx-traits/cdk";

import { FAMILY_BEHAVIOR, QUICK_INTENT } from "../core.ts";
import {
  deriveParkReportStep,
  familyCommitTail,
  gateAndDiffEvidence,
  gateTimedOut,
  gateTimedOutAbortIf,
  repoGatesPassed,
  reviewDiff,
} from "../sequence/family.ts";
import { agent } from "./quick/agent.ts";
import * as port from "./quick/port.ts";
import * as resource from "./quick/resource.ts";
import * as slot from "./quick/slot.ts";

const quickProduceText = input.prompt`
    Implement ${port.task} following the draft ${slot.draft}.
    No reviewer verdict attached means this is round 1: implement the draft in full. On every later round a verdict IS attached, and it redefines your task: execute its blockers' open steps, in order, largest first. The verdict was written against the exact working tree you are in — nothing has changed since — so if it says something is missing, it is missing NOW; re-auditing the tree against the draft re-does, worse, the review that produced the verdict, and spends your round on nothing. Your own work summary from the previous round is also attached: it is your memory of what prior rounds did — trust it plus the verdict's step statuses instead of re-verifying the tree. Before claiming any step was already satisfied, run the check its blocker's done-when names and cite its passing output as evidence; extend the summary with per-step progress as you go.
    A repository gate result may be attached. When its ok field is false, re-run its argv verbatim — that exact command is what decides done-ness, whatever any document or reviewer calls the gate — and repair what it reports. Its tail is the gate's own output and states the actual reason — start there rather than guessing. A tail showing the gate could not run at all (missing recipe, command not found) is a repository misconfiguration you cannot fix from inside the run: report it in the work summary instead of attempting a code change. A gate result whose timed-out field is true is a repo condition — the ceiling is fixed in the trait — never your blocker to fix; report it and stop, do not spend the round chasing it.
    Run this task's own Done-when gates before reporting.
    Return the full work summary: what changed (files), how you validated it, open concerns.`;

const quickReviewText = input.prompt`
    Review the work for ${port.task} against the draft ${slot.draft}. Work summary: ${slot.workSummary}.
    ${reviewDiff} lists every file changed this round with its insertion/deletion counts — an index, not the change. Open whatever it points at with your own tools; it omits untracked files. ${repoGatesPassed} is this round's gate verdict; false is grounds for a blocker on its own — UNLESS its timed-out field is true, which is a repo condition (the bound is the repository's own config), never the worker's blocker, and never grounds for a blocker on its own. This gate is the repository's PER-ROUND gate and proves only what it runs — a repo may reserve heavier proof (full integration suites) for its landing gate, which runs once at merge. Never withhold approval for a check this round's gate does not run: judge the work against the task's own Done-when and this verdict, not against a suite that runs later.
    A BLOCKER is a correctness bug, a failing gate this task's Done-when requires, clear over-build, or un-abstracted duplication. Everything else is advisory and never blocks.
    Your own verdict from last round is attached when one exists: this round's verdict EXTENDS it. Carry every open blocker's id and steps forward VERBATIM — same order, same wording; verify each open step with your own tools and flip its status to done only on evidence you confirmed, recording that evidence on the step. Append genuinely new findings as new steps at the end or as new blockers; set recurrence-of on every carried blocker; drop a blocker only when every step is done, and name the drop in the advisory. Never renumber, reword, or re-derive a carried step — a vanished or rewritten step erases the worker's progress and moves the target, which is how loops stall.
    Set status to revise while any blocker remains, approved when none do. Say "not yet" as many rounds as it takes; do not approve to end the loop, and do not invent a blocker to extend it.
    Escalation: set escalation to needs-owner if and only if the run as a whole cannot reach an approvable state (the task file is a placeholder, lacks a falsifiable Done-when, names a prerequisite task not landed in this tree, or is marked superseded/cancelled) — never merely because one blocker is outside this round's reach. Record the one owner action that would clear it in escalation-reason; otherwise set escalation to none and leave escalation-reason empty.`;

const quickScribeText = input.prompt`The review has ended approved and the work is being committed.
    Write a concise commit message from the draft ${slot.draft} and the verdict ${slot.verdict}: a short subject line naming the change, then one paragraph on what was implemented and how it was validated.
    Return exactly that message. Do not run git commands and do not write files.`;

/** 0098: the loop body, parameterized by an optional commit gate — extracted so `guarded` reuses it instead of copying the loop. */
export function quickProcedure(gate?: { prefix: string; title?: string; timeoutMs?: number }) {
  return procedure.from(
    {
      description:
        "Implement one task from the task board: draft it from the task file, implement it, and repeat worker-then-review until the reviewer approves — then commit.",
    },
    () => {
      agent.smart.prompt("Draft the work", {
        input: input.prompt`
        Create an implementation draft for ${port.task} from its file on the task board ${resource.taskBoard}. Task files are named NNNN-kebab-slug.md; the requested task names its file by number, full name, or filename — read that file with your tools. It is the sole binding authority for this run.
        Cover: scope, files to touch, approach, validation plan, risks.
        Reference files by path. Do not implement anything.`,
        output: slot.draft,
      });

      flow.loop("Building", (loop) => {
        loop.maxIterations(10, { onExhausted: "abort" });

        agent.worker.prompt("Building Produce", {
          input: quickProduceText,
          output: slot.workSummary,
          include: [slot.verdict.optional(), slot.workSummary.optional(), repoGatesPassed.optional()],
        });

        gateAndDiffEvidence({ gatePassed: repoGatesPassed, diff: reviewDiff });

        agent.smart.prompt("Building Review", {
          input: quickReviewText,
          output: slot.verdict,
          include: [slot.verdict.optional()],
        });

        deriveParkReportStep(slot.verdict, { parkReportSlot: slot.parkReport });

        flow.when("Gate Timed Out", gateTimedOutAbortIf, flow.Abort);
        effect.onAbort(gateTimedOut);

        flow.until(
          condition.all([
            condition.equals(slot.verdict.status, "approved"),
            condition.equals(repoGatesPassed.ok, true),
          ]),
        );
      });

      familyCommitTail({
        id: "shipping",
        scribe: { agent: agent.scribe, text: quickScribeText },
        receipt: slot.commitOutput,
        gate,
      });
    },
  );
}

export default variant({
  name: "Implement (Quick)",
  summary:
    "Quick dogfood implementation procedure: draft the approach from the plan, implement it, and grind a single reviewer loop until the work is approved — then commit.",
  metadata: { tag: ["dogfood", "implementation", "review", "lean"] },
  behavior: FAMILY_BEHAVIOR,
  intent: QUICK_INTENT,
  resource: [resource.taskBoard],
  signal: [gateTimedOut],
  port: [port.commitReport, port.parkReportPort],
  procedure: quickProcedure(),
});
