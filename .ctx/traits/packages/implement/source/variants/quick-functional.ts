// PILOT (0108): functional-layer port of implement:quick, using carried-over
// features only (nothing 0105 added). NOT wired into source/index.ts or
// package.toml — this file exists to build to a scratch canonical and diff
// against generated/quick/index.toml, never to replace the object-layer
// original (source/variants/quick.ts stays authoritative until 0109).
//
// Prompt template literals below are copied byte-for-byte from quick.ts
// (0046 ruling: templates don't dedent). Slot/port/resource/agent
// declarations are imported and reused from ./quick/* and ../sequence/
// family.ts, never redeclared.
//
// Known gaps vs the object-layer original, recorded as 0102 amendments
// (see .internal/tasks/0102-the-functional-authoring-layer-the-contract.md):
//   F2 — no explicit step-id override on step.*/agent.prompt: mintId(title)
//        always applies, so seven baseline steps whose id != kebab(title)
//        (draft-writing, repo-gates, capture-diff, shipping-status,
//        shipping-message, shipping-stage, shipping-commit) cannot be
//        reproduced byte-identically here.
//   F3 — no step.project registrar: park-report-clear/park-report-record/
//        park-report-append (kind = "project"/"branch" wrapping a project)
//        cannot be authored functionally at all and are omitted below.
import { condition, effect, flow, input, intent, method, procedure, slot as slotOf, step, tone, variant, verbosity } from "@ctx-traits/cdk";

import { gateTimedOut, gateTimedOutAbortIf, repoGatesPassed, reviewDiff } from "../sequence/family.ts";
import { agent } from "./quick/agent.ts";
import * as port from "./quick/port.ts";
import * as resource from "./quick/resource.ts";
import * as slot from "./quick/slot.ts";

// Not in ./quick/slot.ts: the object-layer original invents these inline via
// `commitTail()` (packages/agents/src/process.ts) rather than declaring them
// alongside the variant's other slots. commitTail() itself cannot be called
// here (0106: no way to lift a pre-built SequenceHandle[] into a functional
// scope), so the tail is hand-expanded below and these three slots are
// declared with commitTail's own id/description/hint, verbatim.
const shippingStatus = slotOf.text({
    id: "shipping-status",
    description: "Working-tree status captured immediately before the commit tail: git status --porcelain output verbatim.",
    hint: "Empty exactly when the tree is clean; the commit tail is skipped entirely in that case, so no commit is attempted and the scribe never runs.",
});
const shippingMessage = slotOf.text({
    id: "shipping-message",
    description: "The commit message the scribe writes for the completed work.",
});
const shippingStageOutput = slotOf.text({
    id: "shipping-stage-output",
    description: "Verbatim output of the git-add staging command.",
});

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

const procedureBody = procedure.from(
    {
        description:
            "Implement one task from the task board: draft it from the task file, implement it, and repeat worker-then-review until the reviewer approves — then commit.",
    },
    () => {
        // F2: baseline id "draft-writing" != kebab("Draft the work"); no id override on agent.prompt.
        agent.smart.prompt("Draft the work", {
            input: input.prompt`
        Create an implementation draft for ${port.task} from its file on the task board ${resource.taskBoard}. Task files are named NNNN-kebab-slug.md; the requested task names its file by number, full name, or filename — read that file with your tools. It is the sole binding authority for this run.
        Cover: scope, files to touch, approach, validation plan, risks.
        Reference files by path. Do not implement anything.`,
            output: slot.draft,
        });

        flow.loop("Building", (loop) => {
            loop.maxIterations(10, { onExhausted: "abort" });
            effect.onAbort(gateTimedOut);

            agent.worker.prompt("Building Produce", {
                input: quickProduceText,
                output: slot.workSummary,
                include: [slot.verdict.optional(), slot.workSummary.optional(), repoGatesPassed.optional()],
            });

            // F2: baseline id "repo-gates" != kebab("Run the repository gate chain").
            step.check("Run the repository gate chain", {
                argv: ["just", "test"],
                output: repoGatesPassed,
            });

            // F2: baseline id "capture-diff" != kebab("Capture the changed-file inventory").
            step.command("Capture the changed-file inventory", {
                argv: ["git", "diff", "--stat", "--", ".", ":(exclude).agents/runs"],
                output: reviewDiff,
            });

            agent.smart.prompt("Building Review", {
                input: quickReviewText,
                output: slot.verdict,
                include: [slot.verdict.optional()],
            });

            // F3 GAP — cannot express here: no step.project registrar exists in the
            // functional layer (registrars.ts exposes only step.command/step.check).
            // The baseline's park-report-clear / park-report-record / park-report-append
            // (kind = "project", deriveParkReportStep in ../sequence/family.ts) are
            // omitted; slot:park-report is declared (via port.parkReportPort below) but
            // never written by this ported variant. Recorded as an 0102 amendment.

            flow.when("Gate Timed Out", gateTimedOutAbortIf, flow.Abort);

            flow.until(condition.all([
                condition.equals(slot.verdict.status, "approved"),
                condition.equals(repoGatesPassed.ok, true),
            ]));
        });

        // F2: baseline id "shipping-status" != kebab("Check working tree status").
        step.command("Check working tree status", {
            argv: ["git", "status", "--porcelain"],
            output: shippingStatus,
        });

        flow.when("Shipping Maybe Commit", condition.not(condition.equals(shippingStatus, "")), () => {
            // F2: baseline id "shipping-message" != kebab("Write the commit message (scribe)").
            agent.scribe.prompt("Write the commit message (scribe)", {
                input: quickScribeText,
                output: shippingMessage,
            });

            // F2: baseline id "shipping-stage" != kebab("Stage all changes except runtime state").
            step.command("Stage all changes except runtime state", {
                argv: ["git", "add", "-A", "--", ":!.agents/runs"],
                output: shippingStageOutput,
            });

            // F2: baseline id "shipping-commit" != kebab("Commit the work").
            step.command("Commit the work", {
                input: input.command`git commit -m ${shippingMessage}`,
                output: slot.commitOutput,
            });
        });
    },
);

export default variant({
    name: "Implement (Quick)",
    summary:
        "Quick dogfood implementation procedure: draft the approach from the plan, implement it, and grind a single reviewer loop until the work is approved — then commit.",
    metadata: { tag: ["dogfood", "implementation", "review", "lean"] },
    behavior: {
        tone: [tone.Direct, tone.Technical],
        method: method.EvidenceFirst,
        verbosity: verbosity.Brief,
    },
    intent: {
        require: [
            intent.focus.Correctness,
            intent.require.Leanness,
            intent.require.ReuseOverReimplement,
            intent.require.ReviewBeforeFinal,
        ],
        avoid: [
            intent.avoid.OverEngineering,
            intent.avoid.ScopeCreep,
            intent.avoid.RubberStampReview,
        ],
    },
    resource: [resource.taskBoard],
    signal: [gateTimedOut],
    port: [port.commitReport, port.parkReportPort],
    procedure: procedureBody,
});
