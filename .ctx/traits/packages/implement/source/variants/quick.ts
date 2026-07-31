// implement-quick: draft, then grind worker -> reviewer until approved, then
// commit. Ten rounds; exhausting them without approval blocks the run and
// commits nothing.
//
// DELIBERATELY LEAN (2026-07-27). This variant is called quick and now reads
// like it: no recurrence breaker, no earned-exit carry/min-rounds, no
// leftovers adjudication, no park-report projection, no spliced doctrine
// blocks. Every one of those was a knob that interacted badly with the
// others — the measured failures were a breaker killing a converging loop,
// an earned-exit floor stacked on top of a fix loop, and prompts long enough
// that the actual instruction was buried. The loop IS the mechanism: the
// reviewer says "not yet" and the worker goes again, until it is satisfied or
// the budget is gone.
import { commitTail, guardedProduction } from "@ctx-traits/agents";
import { condition, intent, method, procedure, prompt, sequence, variant, tone, verbosity } from "@ctx-traits/cdk";

import { captureDiffStep, repoGatesPassed, repoGatesStep, reviewDiff } from "../sequence/family.ts";
import { agent } from "./quick/agent.ts";
import * as port from "./quick/port.ts";
import * as resource from "./quick/resource.ts";
import * as slot from "./quick/slot.ts";

const draftStep = sequence.prompt("draft-writing", {
    title: "Draft the work",
    agent: agent.smart,
    text: prompt.text`
        Create an implementation draft for ${port.task} from its file on the task board ${resource.taskBoard}. Task files are named NNNN-kebab-slug.md; the requested task names its file by number, full name, or filename — read that file with your tools. It is the sole binding authority for this run.
        Cover: scope, files to touch, approach, validation plan, risks.
        Take the leanest correct approach. Reuse what exists rather than re-implementing beside it. Reference files by path. Do not implement anything.`,
    output: slot.draft,
});

const quickProduceText = prompt.template(
    `Implement {task} following the draft {draft}.
    No reviewer verdict attached means this is round 1: implement the draft in full. On every later round a verdict IS attached, and it redefines your task: execute its blockers' open steps, in order, largest first. The verdict was written against the exact working tree you are in — nothing has changed since — so if it says something is missing, it is missing NOW; re-auditing the tree against the draft re-does, worse, the review that produced the verdict, and spends your round on nothing. Your own work summary from the previous round is also attached: it is your memory of what prior rounds did — trust it plus the verdict's step statuses instead of re-verifying the tree. Before claiming any step was already satisfied, run the check its blocker's done-when names and cite its passing output as evidence; extend the summary with per-step progress as you go.
    A repository gate result may be attached. When its ok field is false, re-run its argv verbatim — that exact command is what decides done-ness, whatever any document or reviewer calls the gate — and repair what it reports. Its tail is the gate's own output and states the actual reason — start there rather than guessing. A tail showing the gate could not run at all (missing recipe, command not found) is a repository misconfiguration you cannot fix from inside the run: report it in the work summary instead of attempting a code change.
    Run this task's own Done-when gates before reporting.
    Return the full work summary: what changed (files), how you validated it, open concerns.`,
    { task: port.task, draft: slot.draft },
);

const quickReviewText = prompt.template(
    `Review the work for {task} against the draft {draft}. Work summary: {workSummary}.
    {reviewDiff} lists every file changed this round with its insertion/deletion counts — an index, not the change. Open whatever it points at with your own tools; it omits untracked files. {repoGatesPassed} is this round's gate verdict; false is grounds for a blocker on its own.
    A BLOCKER is a correctness bug, a failing gate this task's Done-when requires, clear over-build, or un-abstracted duplication. Everything else is advisory and never blocks.
    Your own verdict from last round is attached when one exists: this round's verdict EXTENDS it. Carry every open blocker's id and steps forward VERBATIM — same order, same wording; verify each open step with your own tools and flip its status to done only on evidence you confirmed, recording that evidence on the step. Append genuinely new findings as new steps at the end or as new blockers; set recurrence-of on every carried blocker; drop a blocker only when every step is done, and name the drop in the advisory. Never renumber, reword, or re-derive a carried step — a vanished or rewritten step erases the worker's progress and moves the target, which is how loops stall.
    Set status to revise while any blocker remains, approved when none do. Say "not yet" as many rounds as it takes; do not approve to end the loop, and do not invent a blocker to extend it.`,
    {
        task: port.task,
        draft: slot.draft,
        workSummary: slot.workSummary,
        reviewDiff,
        repoGatesPassed,
    },
);

const building = guardedProduction({
    id: "building",
    produces: slot.workSummary,
    produce: {
        agent: agent.worker,
        text: quickProduceText,
        optionalInputs: [repoGatesPassed],
    },
    review: {
        agent: agent.smart,
        verdictSlot: slot.verdict,
        text: quickReviewText,
        // The reviewer reads its OWN previous verdict, attached as frame
        // context (never interpolated — core refuses an optional ref a prompt
        // requires). Without this the verdict slot is write-only from the
        // reviewer's side: every round replaces it, so a blocker raised in
        // round N is invisible in round N+1 and vanishes unfixed. Measured on
        // run-3d5ef0a1ff, where `implement-fold-not-landed` was raised once
        // and never seen again.
        extraInputs: [slot.verdict.optional()],
    },
    evidence: [repoGatesStep, captureDiffStep],
    alsoRequire: condition.equals(repoGatesPassed.ok, true),
    rounds: 10,
    // Ten rounds without an approving verdict is a blocked run, not a commit:
    // the scribe and the git tail are unreachable, and the run parks.
    onExhausted: "block",
});

const quickScribeText = prompt.template(
    `The review has ended approved and the work is being committed.
    Write a concise commit message from the draft {draft} and the verdict {verdict}: a short subject line naming the change, then one paragraph on what was implemented and how it was validated.
    Return exactly that message. Do not run git commands and do not write files.`,
    { draft: slot.draft, verdict: slot.verdict },
);

export default variant({
    name: "Implement (Quick)",
    summary:
        "Quick dogfood implementation procedure: draft the approach from the plan, implement it, and grind a single reviewer loop until the work is approved — then commit.",
    metadata: { tag: ["dogfood", "implementation", "review", "lean"] },
    behavior: {
        tone: [tone.direct, tone.technical],
        method: method.evidenceFirst,
        verbosity: verbosity.brief,
    },
    intent: {
        require: [
            intent.focus.correctness,
            intent.require.leanness,
            intent.require.reuseOverReimplement,
            intent.require.reviewBeforeFinal,
        ],
        avoid: [
            intent.avoid.overEngineering,
            intent.avoid.scopeCreep,
            intent.avoid.rubberStampReview,
        ],
    },
    resource: [resource.taskBoard],
    procedure: procedure({
        description:
            "Implement one task from the task board: draft it from the task file, implement it, and repeat worker-then-review until the reviewer approves — then commit.",
        input: port.task,
        output: port.commitReport,
        sequence: [
            draftStep,
            building,
            ...commitTail({
                id: "shipping",
                scribe: { agent: agent.scribe, text: quickScribeText },
                summary: slot.workSummary,
                verdict: building.verdict,
                receipt: slot.commitOutput,
            }),
        ],
    }),
});
