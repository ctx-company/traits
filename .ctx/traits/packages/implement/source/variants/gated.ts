// implement-gated: quick's lean grind with the owner inside it. Same
// draft -> worker/reviewer loop -> commit spine as implement-quick, plus the
// three plannotate grafts (approved 2026-08-06): the owner annotates the plan
// in plannotator before any work starts; the owner holds a gate the machine
// reviewer cannot open — summoned only on a round the reviewer already
// approved with the repository gate green, briefed by a fresh smart frame
// grounded in the working tree; and the approved work is recorded as a
// tracked brief that ships in the same commit.
//
// Hard dependency: the third-party `plannotator` binary. The preflight step
// fails the run before any model call is spent when it is missing — the one
// doctrinal line this variant knowingly crosses (plannotate stayed a
// standalone package for exactly this reason).
//
// Owner rulings 2026-08-06: every plannotator invocation selects its port
// from a range (PORT_SELECTION below) so concurrent runs' UIs coexist on
// their own URLs instead of replacing each other; and a gated briefing the
// owner closes without deciding ("dismissed") fails the summon step and
// halts the run instead of burning another worker round.
//
// DELIBERATELY LEAN, like quick: no recurrence breaker, no earned-exit
// carry/min-rounds, no leftovers adjudication, no spliced doctrine blocks.
// The loop IS the mechanism — the reviewer says "not yet" and the worker goes
// again — and the owner's denial is just one more typed blocker for the next
// round.
import { condition, effect, flow, input, procedure, step, variant } from "@ctx-traits/cdk";

import { FAMILY_BEHAVIOR, GATED_INTENT } from "../core.ts";
import {
    deriveParkReportStep,
    gateTimedOut,
    gateTimedOutAbortIf,
    repoGatesPassed,
    reviewDiff,
} from "../sequence/family.ts";
import { agent } from "./gated/agent.ts";
import * as port from "./gated/port.ts";
import * as resource from "./gated/resource.ts";
import * as slot from "./gated/slot.ts";

// A human annotates/reads in a browser; each plannotator step's own bound
// must beat the repository's `command-idle-seconds` (a step that declares a
// bound owns it — run.rs::resolve_command_bounds).
const HUMAN_IDLE_CEILING_MS = 4 * 60 * 60 * 1000;

// Every plannotator invocation gets a port from this range (first free wins)
// unless the owner's own PLANNOTATOR_PORT is set. Without it, remote
// (SSH_TTY/SSH_CONNECTION) sessions pin plannotator's single fixed default
// port, so concurrent runs replace each other's UI instead of coexisting;
// with a range each live gate binds its own port and URL (`plannotator
// sessions` lists them). Locally plannotator already defaults to an
// ephemeral port — the range just makes URLs predictable there too.
const PORT_SELECTION = 'PLANNOTATOR_PORT="${PLANNOTATOR_PORT:-7411-7460}"';

const gatedProduceText = input.prompt`
    Implement ${port.task} following the plan ${slot.plan}.
    No reviewer verdict attached means this is round 1: implement the plan in full. On every later round a verdict IS attached, and it redefines your task: execute its blockers' open steps, in order, largest first. The verdict was written against the exact working tree you are in — nothing has changed since — so if it says something is missing, it is missing NOW; re-auditing the tree against the plan re-does, worse, the review that produced the verdict, and spends your round on nothing. Your own work summary from the previous round is also attached: it is your memory of what prior rounds did — trust it plus the verdict's step statuses instead of re-verifying the tree. Before claiming any step was already satisfied, run the check its blocker's done-when names and cite its passing output as evidence; extend the summary with per-step progress as you go.
    A repository gate result may be attached. When its ok field is false, re-run its argv verbatim — that exact command is what decides done-ness, whatever any document or reviewer calls the gate — and repair what it reports. Its tail is the gate's own output and states the actual reason — start there rather than guessing. A tail showing the gate could not run at all (missing recipe, command not found) is a repository misconfiguration you cannot fix from inside the run: report it in the work summary instead of attempting a code change. A gate result whose timed-out field is true is a repo condition — the ceiling is fixed in the trait — never your blocker to fix; report it and stop, do not spend the round chasing it.
    An owner decision may be attached: it is the owner's gate verdict on a previous round's briefing — always "annotated" (an approved verdict would already have ended the loop, and a dismissal halts the run before another round starts) — and its feedback text is the owner's exact words on what must change. The reviewer records an unsatisfied annotation as a typed blocker; read the feedback itself as the authoritative statement of what the owner wants.
    Run this task's own Done-when gates before reporting.
    Return the full work summary: what changed (files), how you validated it, open concerns.`;

const gatedReviewText = input.prompt`
    Review the work for ${port.task} against the plan ${slot.plan}. Work summary: ${slot.workSummary}.
    ${reviewDiff} lists every file changed this round with its insertion/deletion counts — an index, not the change. Open whatever it points at with your own tools; it omits untracked files. ${repoGatesPassed} is this round's gate verdict; false is grounds for a blocker on its own — UNLESS its timed-out field is true, which is a repo condition (the bound is the repository's own config), never the worker's blocker, and never grounds for a blocker on its own. This gate is the repository's PER-ROUND gate and proves only what it runs — a repo may reserve heavier proof (full integration suites) for its landing gate, which runs once at merge. Never withhold approval for a check this round's gate does not run: judge the work against the task's own Done-when and this verdict, not against a suite that runs later.
    If an owner decision is attached to this frame, it is the owner's "annotated" verdict on a previous round's briefing (an approved one would already have ended the loop; a dismissal halts the run): treat any feedback it carries as an unsatisfied BLOCKER, never advisory, until the work visibly addresses it — record it as a typed blocker like any other, and carry it forward under the same verbatim rules.
    A BLOCKER is a correctness bug, a failing gate this task's Done-when requires, clear over-build, un-abstracted duplication, or an unaddressed owner annotation. Everything else is advisory and never blocks.
    Your own verdict from last round is attached when one exists: this round's verdict EXTENDS it. Carry every open blocker's id and steps forward VERBATIM — same order, same wording; verify each open step with your own tools and flip its status to done only on evidence you confirmed, recording that evidence on the step. Append genuinely new findings as new steps at the end or as new blockers; set recurrence-of on every carried blocker; drop a blocker only when every step is done, and name the drop in the advisory. Never renumber, reword, or re-derive a carried step — a vanished or rewritten step erases the worker's progress and moves the target, which is how loops stall.
    Set status to revise while any blocker remains, approved when none do. Say "not yet" as many rounds as it takes; do not approve to end the loop, and do not invent a blocker to extend it. Approving does not end the loop by itself: it summons the owner, whose gate is the final word.
    Escalation: set escalation to needs-owner if and only if the run as a whole cannot reach an approvable state (the task file is a placeholder, lacks a falsifiable Done-when, names a prerequisite task not landed in this tree, or is marked superseded/cancelled) — never merely because one blocker is outside this round's reach. Record the one owner action that would clear it in escalation-reason; otherwise set escalation to none and leave escalation-reason empty.`;

const gatedBriefingText = input.prompt`
    The reviewer has approved this round's work for ${port.task} with the repository gate green, and the owner is about to be summoned to confirm it ships. Plan: ${slot.plan}. Work summary: ${slot.workSummary}. Reviewer verdict: ${slot.verdict}.
    Inspect the working tree with your tools (git diff, git status, the changed files) so the briefing reflects what actually changed, not just what the summary claims.
    Write a short markdown briefing for the owner, to be read cold — the owner has not seen the working tree: what changed and why, what the review checked and found (including advisory notes worth the owner's attention), and what the owner is being asked to confirm.
    Return only the briefing markdown.`;

const gatedBriefText = input.prompt`
    The build loop for ${port.task} has ended in a double approval (reviewer and owner) and the work is being recorded and committed. Plan: ${slot.plan}. Work summary: ${slot.workSummary}. Final reviewer verdict: ${slot.verdict}.
    Inspect the working tree with your tools (git diff, git status) so the record reflects the actual change, not just the summary's claims.
    Write: a short kebab-case slug for this task; the brief in markdown — what was built, why, how it was validated, and what the review found; and the commit message — a short subject line naming the change, then a one-paragraph summary of what was implemented and how it was validated.
    Return the slug, the markdown, and the commit message as the typed output. Do not write any file and do not run git commands; later runtime steps save and commit.`;

const procedureBody = procedure.from(
    {
        description:
            "Implement one task from the task board with the owner in the loop: confirm plannotator is installed, draft the work and put the owner inside plannotator to annotate the draft, refine it into the plan, then repeat worker-then-review — summoning the owner on every gate-green approved round — until the reviewer and the owner both approve; then record a tracked brief and commit.",
    },
    () => {
        // First step, before any prompt frame: a missing binary fails here,
        // not after a model call has already been spent.
        step.command("Confirm plannotator is installed", {
            id: "preflight-binary",
            argv: ["sh", "-c", "command -v plannotator"],
            output: slot.preflightOutput,
        });

        agent.smart.prompt("Draft the work", {
            id: "draft-writing",
            input: input.prompt`
        Create an implementation draft for ${port.task} from its file on the task board ${resource.taskBoard}. Task files are named NNNN-kebab-slug.md; the requested task names its file by number, full name, or filename — read that file with your tools. It is the sole binding authority for this run.
        Cover: scope, files to touch, approach, validation plan, risks.
        Reference files by path. Do not implement anything. The owner will annotate this draft next — write it to be read cold.`,
            output: slot.draft,
        });

        // A command step has no stdin channel, so piping the hook envelope
        // into plannotator rides `sh -c`; the draft reaches the shell as a
        // positional `$1` argument, never spliced into the script string.
        // Closing the plan UI without deciding cannot halt here: plannotator's
        // plan-mode hook collapses a UI exit into a plain deny ("Plan changes
        // requested"), indistinguishable from a real annotation — so a closed
        // plan window flows into plan-refine as a deny, and only the gated
        // briefing steps below carry the distinguishable "dismissed" verdict.
        step.command("Annotate the plan (plannotator, plan mode)", {
            id: "plan-owner-review",
            argv: [
                "sh",
                "-c",
                `printf %s "$1" | jq -n --arg plan "$(cat)" '{hook_event_name: "PreToolUse", tool_name: "ExitPlanMode", tool_input: {plan: $plan}}' | PLANNOTATOR_ORIGIN=implement-gated ${PORT_SELECTION} plannotator`,
                "sh",
                "{slot:draft}",
            ],
            idleTimeoutMs: HUMAN_IDLE_CEILING_MS,
            output: slot.planDecision,
        });

        agent.smart.prompt("Refine the plan against the owner's annotations", {
            id: "plan-refine",
            input: input.prompt`
        Refine the draft ${slot.draft} into the final plan, using plannotator's plan-mode verdict ${slot.planDecision}.
        If hookSpecificOutput.decision.behavior is "approve", the refined plan may be the draft unchanged.
        If it is "deny", hookSpecificOutput.decision.message is the owner's annotation — every point it raises must be visibly and provably addressed in the refined plan (not merely acknowledged): change the affected section, or state explicitly why the draft already covers it.
        Reference files by path; do not inline documents. Do not implement anything yet.`,
            output: slot.plan,
        });

        flow.loop("Building", (loop) => {
            loop.maxIterations(10, { onExhausted: "abort" });

            agent.worker.prompt("Building Produce", {
                input: gatedProduceText,
                output: slot.workSummary,
                include: [
                    slot.verdict.optional(),
                    slot.workSummary.optional(),
                    repoGatesPassed.optional(),
                    slot.ownerDecision.optional(),
                ],
            });

            step.check("Run the repository gate chain", {
                id: "repo-gates",
                argv: ["just", "test"],
                output: repoGatesPassed,
            });

            step.command("Capture the changed-file inventory", {
                id: "capture-diff",
                argv: ["git", "diff", "--stat", "--", ".", ":(exclude).agents/runs"],
                output: reviewDiff,
            });

            agent.smart.prompt("Building Review", {
                input: gatedReviewText,
                output: slot.verdict,
                include: [slot.verdict.optional(), slot.ownerDecision.optional()],
            });

            deriveParkReportStep(slot.verdict, { parkReportSlot: slot.parkReport });

            flow.when("Gate Timed Out", gateTimedOutAbortIf, flow.Abort);
            effect.onAbort(gateTimedOut);

            // The owner is summoned only on a round the reviewer already
            // approved WITH the repository gate green — a summon on a red
            // gate would ask the owner to confirm a round the loop cannot
            // exit anyway (extends plannotate's reviewer-approved-only rule).
            flow.when(
                "Owner Gate",
                condition.all([
                    condition.equals(slot.verdict.status, "approved"),
                    condition.equals(repoGatesPassed.ok, true),
                ]),
                () => {
                    agent.smart.prompt("Write the round briefing", {
                        id: "round-briefing",
                        input: gatedBriefingText,
                        output: slot.ownerBriefing,
                    });
                    // plannotator's `annotate` requires a real `.md` file path
                    // (no stdin form), so the briefing goes through a transient
                    // temp file created and removed inside this one step;
                    // plannotator's stdout is the only stdout, so the gate JSON
                    // lands in the output slot clean. A "dismissed" verdict —
                    // the owner closed the briefing via Exit without deciding —
                    // exits 65: the step fails and the run halts instead of
                    // spinning another worker round nobody asked for; the
                    // decision JSON is echoed first so the failure evidence
                    // carries the verdict.
                    step.command("Summon the owner (plannotator, gated)", {
                        id: "owner-review",
                        argv: [
                            "sh",
                            "-c",
                            `d=$(mktemp -d) && printf %s "$1" > "$d/briefing.md" && ${PORT_SELECTION} plannotator annotate "$d/briefing.md" --gate --json > "$d/decision.json"; s=$?; [ -s "$d/decision.json" ] && cat "$d/decision.json"; if [ $s -eq 0 ] && grep -q '"decision":"dismissed"' "$d/decision.json"; then echo "owner closed the briefing without a decision — halting the run" >&2; s=65; fi; rm -rf "$d"; exit $s`,
                            "sh",
                            "{slot:owner-briefing}",
                        ],
                        idleTimeoutMs: HUMAN_IDLE_CEILING_MS,
                        output: slot.ownerDecision,
                    });
                },
            );

            flow.until(condition.all([
                condition.equals(slot.verdict.status, "approved"),
                condition.equals(repoGatesPassed.ok, true),
                condition.equals(slot.ownerDecision.decision, "approved"),
            ]));
        });

        // The commit tail, clean-tree-guarded like quick's, with plannotate's
        // brief grafts inside the guard: the brief is written and displayed
        // only when there is something to commit, and ships in the same
        // commit as the work it describes.
        step.command("Check working tree status", {
            id: "shipping-status",
            argv: ["git", "status", "--porcelain"],
            output: slot.shippingStatus,
        });

        flow.when("Shipping Maybe Commit", condition.not(condition.equals(slot.shippingStatus, "")), () => {
            agent.smart.prompt("Write the brief and commit message", {
                id: "summary-brief",
                input: gatedBriefText,
                output: slot.brief,
            });
            step.project("Lift Brief", {
                projections: [
                    { source: slot.brief, field: "slug", destination: slot.briefSlug },
                    { source: slot.brief, field: "markdown", destination: slot.briefMarkdown },
                    { source: slot.brief, field: "commit-message", destination: slot.commitMessage },
                ],
            });
            step.command("Save the brief to .internal/briefs/", {
                id: "write-brief",
                argv: [
                    "sh",
                    "-c",
                    'mkdir -p .internal/briefs && printf %s "$1" > ".internal/briefs/$2.md"',
                    "sh",
                    "{slot:brief-markdown}",
                    "{slot:brief-slug}",
                ],
                output: slot.briefWriteOutput,
            });
            // No `--gate`: the brief is a record, not a decision — closing it
            // is the step's normal end, never a halt.
            step.command("Show the brief to the owner (plannotator, display only)", {
                id: "display-brief",
                argv: [
                    "sh",
                    "-c",
                    `${PORT_SELECTION} plannotator annotate "$1"`,
                    "sh",
                    ".internal/briefs/{slot:brief-slug}.md",
                ],
                include: [slot.briefWriteOutput],
                idleTimeoutMs: HUMAN_IDLE_CEILING_MS,
                output: slot.briefDisplayOutput,
            });
            // Two steps, not one excluding add — a pathspec that mentions a
            // gitignored `.agents` exits 1 (see familyCommitTail, run-42bd7fb2).
            // The brief under .internal/briefs/ is inside this stage — it
            // ships in the same commit as the work it describes.
            step.command("Stage all changes", {
                id: "git-stage",
                argv: ["git", "add", "-A"],
                output: slot.stageOutput,
            });
            step.command("Unstage runtime state", {
                id: "git-unstage-runtime",
                argv: ["git", "reset", "-q", "--", ".agents/runs"],
                output: slot.unstageOutput,
            });
            step.command("Commit the work", {
                id: "git-commit",
                argv: ["git", "commit", "-m", "{slot:commit-message}"],
                input: [slot.commitMessage],
                output: slot.commitOutput,
            });
        });
    },
);

export default variant({
    name: "Implement (Gated)",
    summary:
        "Quick's implementation loop with the owner inside it: draft the approach, have the owner annotate the plan in plannotator, then grind a single reviewer loop where every gate-green approved round summons the owner to a gated briefing — on double approval, record a tracked brief and commit.",
    metadata: { tag: ["dogfood", "implementation", "review", "lean", "human-in-the-loop", "plannotator"] },
    behavior: FAMILY_BEHAVIOR,
    intent: GATED_INTENT,
    resource: [resource.taskBoard],
    signal: [gateTimedOut],
    port: [port.commitReport, port.parkReportPort, port.briefReport],
    procedure: procedureBody,
});
