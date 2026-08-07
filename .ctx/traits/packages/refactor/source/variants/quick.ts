import { blockerSchema, INTEGRITY_DOCTRINE, reviewerRole, QUICK_VARIANT_DOCTRINE, scribeRole, workerRole } from "@ctx-traits/agents";
import { condition, input, intent, port, procedure, schema, sequence, slot, variant } from "@ctx-traits/cdk";

import { architectureDialect } from "../resource.ts";

const smart1 = reviewerRole(
    "smart-1",
    "Turns the target into an actionable checklist and reviews the implemented work once.",
    "Checklist and reviewer.",
);
const worker = workerRole("worker", "Implements the checklist and applies the reviewer's single pass.");
const scribe = scribeRole("scribe", "Writes the refactor commit message from the checklist and review record");

const target = port.input.text({
    id: "target",
    description:
        'Module, file path, or entity to refactor; append an optional emphasis after a dash (e.g. "modules/cli/src/app/run_view.rs", "crate::lockfile — typed errors only").',
});

const checklist = slot.text({
    id: "checklist",
    description: "Actionable checklist for the target: concrete steps, no framing or design prose.",
});
const workSummary = slot.text({
    id: "work-summary",
    description: "Worker's report of the refactored state; revised once after the single review pass.",
    hint: "What changed (files), how behavior was preserved, gate results, open concerns.",
});
const commitMessage = slot.text({
    id: "commit-message",
    description: "Refactor commit message; injected directly into the git commit command step.",
});
const stageOutput = slot.text({
    id: "stage-output",
    description: "Output evidence from the git staging command step.",
});
const commitOutput = slot.text({
    id: "commit-output",
    description: "Output evidence from the git commit command step: committed hash and subject.",
});
const gitStatus = slot.text({
    id: "git-status",
    description: "Working-tree status captured immediately before the commit tail: git status --porcelain output verbatim.",
    hint: "Empty exactly when the tree is clean; the commit tail is skipped entirely in that case, so no commit is attempted and the scribe never runs.",
});

const reviewVerdict = schema.object(
    "review-verdict",
    {
        status: schema.field(schema.enum(["approved", "revise"] as const), {
            description:
                "approved when the checklist is correct, behavior-preserving, and introduces no new smell; revise only when a blocking defect remains. Set to approved if and only if blockers is empty.",
        }),
        blockers: schema.field(schema.list(blockerSchema), {
            required: false,
            description:
                "The blocking defects that must be fixed before merge: a behavior break, a new smell, or an interface widened to make a caller compile. Checklist-item fidelity is judged solely under the authority rule below — a left-undone step is forgivable there with a recorded reason and is never listed here as a categorical blocker on its own. Present when status is revise; empty or absent when approved.",
        }),
        advisory: schema.field(schema.text(), {
            required: false,
            description: "Non-blocking notes: taste, follow-up, deferred cleanup. Never affects status.",
        }),
        "forgiveness-reason": schema.field(schema.text(), {
            required: false,
            description:
                "Present ONLY when this verdict forgives a plan-fidelity gap — a missing or altered checklist step that changed nothing an observer could break. One line naming the gap and why it was reasonable to diverge. Never present for a genuine defect, which is always a blocker regardless of reason.",
        }),
    },
    {
        description:
            "Typed single-pass review verdict. The loop blocks only on blockers; forgivable plan-fidelity gaps are recorded, not silently dropped.",
    },
);

const verdict = slot({
    id: "review-verdict",
    schema: reviewVerdict,
    description: "smart-1's single review verdict for the implemented checklist.",
});

const commitReport = port.output.text({
    id: "commit-report",
    description:
        "Final commit evidence from the git commit command step. Absent when the clean-tree gate (P397) skipped the commit tail entirely — a clean working tree at gate time means nothing to commit.",
    optional: true,
    value: commitOutput,
});

export default variant({
    name: "Refactor (Quick)",
    summary:
        "Quick refactoring procedure: turn the target into an actionable checklist, implement it, one reviewer pass, apply that pass once if needed, and commit.",
    metadata: { tag: ["first-party", "refactoring", "review", "multi-agent"] },
    intent: {
        require: [
            intent.require.ReviewBeforeFinal,
            intent.require.BehaviorPreservingDefault,
        ],
        avoid: [
            intent.avoid.RubberStampReview,
            intent.avoid.InterfaceWidening,
        ],
    },
    resource: architectureDialect,
    port: commitReport,
    procedure: procedure({
        description:
            "Refactor one module or entity quickly: an actionable checklist, implementation, exactly one reviewer pass, one round of fixes if needed, commit.",
        sequence: [
            sequence.prompt("checklist", {
                title: "Checklist the target (smart-1)",
                agent: smart1,
                text: input.prompt`
                    Turn ${target} into an actionable refactoring checklist.
                    Read the target with your tools; list concrete steps only — no framing prose, no illustrative design. Keep it to what honestly fits one quick run.
                    Return the checklist.`,
                output: checklist,
            }),
            sequence.prompt("implement", {
                title: "Implement the checklist (worker)",
                agent: worker,
                text: input.prompt`
                    Implement the checklist ${checklist} for ${target}.
                    Behavior-preserving is the contract: serialized shapes, digests, and CLI output stay byte-stable. Do not widen any interface to make a caller compile — fix the caller.
                    Run the repo validation gates before reporting. Return a work summary: what changed (files), how behavior was preserved, gate results, open concerns.`,
                output: workSummary,
            }),
            sequence.prompt("review", {
                title: "Review the implementation (smart-1)",
                agent: smart1,
                text: input.prompt(
                    `Review the implemented state of {target} against the checklist {checklist}. Current work summary: {workSummary}.
                    A BLOCKER always includes a behavior break, a new smell, or an interface widened to make a caller compile. Checklist-item fidelity is judged SOLELY by the authority rule below — apply it directly, with no separate "undone step is always a blocker" rule layered on top. ${QUICK_VARIANT_DOCTRINE}
                    This is the ONLY review pass — the worker applies your findings once and the run commits.
                    Wherever the authority rule above names REVIEW_VERDICT_DOCTRINE, it means the composed baseline stated immediately below — this family splices the leaner, family-compatible INTEGRITY_DOCTRINE in its place, not the implement-phase doctrine.
                    ${INTEGRITY_DOCTRINE}`,
                    { target, checklist, workSummary, phaseBrief: checklist, productBrief: architectureDialect },
                ),
                output: verdict,
                input: [target, checklist, workSummary],
            }),
            sequence.prompt("apply-fixes", {
                title: "Apply the reviewer's pass (worker)",
                agent: worker,
                text: input.prompt`
                    Apply the reviewer findings for ${target}: ${verdict}.
                    Fix every BLOCKER named in the verdict — those are mandatory to merge. This is the only round: leave nothing planned for a next pass that does not exist. Re-run the repo validation gates.
                    Then return the full updated work summary replacing ${workSummary}: what changed (files), how behavior was preserved, gate results, open concerns.`,
                output: workSummary,
            }),
            sequence.command({ id: "check-git-status", title: "Check working tree status", argv: ["git", "status", "--porcelain"], output: gitStatus }),
            sequence.when("maybe-commit", {
                if: condition.not(condition.equals(gitStatus, "")),
                then: [
                    sequence.prompt("summarization", {
                        title: "Write the commit message (scribe)",
                        agent: scribe,
                        text: input.prompt`
                        The single review pass for the refactor of ${target} has ended and the work is being committed. The reviewer verdict is ${verdict} — read its status field FIRST; a status still set to revise means the one round did not fully satisfy the reviewer: say so plainly.
                        Write the commit message from the checklist ${checklist} and that verdict: subject line "refactor(${target}): <boundary in a few words>", then one paragraph summarizing what changed and behavior preservation.
                        If the verdict carries a forgiveness-reason, add one line noting the forgiven plan-fidelity gap and its reason. If status is revise, end with an "Unresolved reviewer blockers:" section listing each open blocker in one line. Omit both sections when not applicable.
                        Return exactly that message as your output; the runtime injects it into the git commit step.
                        Do not run any git commands and do not write any files for the message; staging and committing happen in later runtime steps.`,
                        output: commitMessage,
                    }),
                    sequence.command({
                        id: "git-stage",
                        title: "Stage all changes except runtime state",
                        argv: ["git", "add", "-A", "--", ":(exclude).agents/runs"],
                        output: stageOutput,
                    }),
                    sequence.command({
                        id: "git-commit",
                        title: "Commit the refactor",
                        argv: ["git", "commit", "-m", commitMessage],
                        output: commitOutput,
                    }),
                ],
            }),
        ],
    }),
});
