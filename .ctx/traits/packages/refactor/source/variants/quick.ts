import { INTEGRITY_DOCTRINE, QUICK_VARIANT_DOCTRINE } from "@ctx-traits/agents";
import { condition, defineVariant, flow, input, intent, step, useIntent, useResource } from "@ctx-traits/cdk";

import { architectureDialect } from "../resource.ts";
import { scribe, smart1, worker } from "./quick/agent.ts";
import {
    checklist,
    commitMessage,
    commitOutput,
    commitReport,
    gitStatus,
    stageOutput,
    target,
    unstageOutput,
    workSummary,
} from "./quick/data.ts";
import { verdict } from "./quick/schema.ts";

const checklistText = input.prompt`
                    Turn ${target} into an actionable refactoring checklist.
                    Read the target with your tools; list concrete steps only — no framing prose, no illustrative design. Keep it to what honestly fits one quick run.
                    Return the checklist.`;
const implementText = input.prompt`
                    Implement the checklist ${checklist} for ${target}.
                    Behavior-preserving is the contract: serialized shapes, digests, and CLI output stay byte-stable. Do not widen any interface to make a caller compile — fix the caller.
                    Run the repo validation gates before reporting. Return a work summary: what changed (files), how behavior was preserved, gate results, open concerns.`;
const reviewText = input.prompt(
                    `Review the implemented state of {target} against the checklist {checklist}. Current work summary: {workSummary}.
                    A BLOCKER always includes a behavior break, a new smell, or an interface widened to make a caller compile. Checklist-item fidelity is judged SOLELY by the authority rule below — apply it directly, with no separate "undone step is always a blocker" rule layered on top. ${QUICK_VARIANT_DOCTRINE}
                    This is the ONLY review pass — the worker applies your findings once and the run commits.
                    Wherever the authority rule above names REVIEW_VERDICT_DOCTRINE, it means the composed baseline stated immediately below — this family splices the leaner, family-compatible INTEGRITY_DOCTRINE in its place, not the implement-phase doctrine.
                    ${INTEGRITY_DOCTRINE}`,
                    { target, checklist, workSummary, phaseBrief: checklist, productBrief: architectureDialect },
                );
const applyText = input.prompt`
                    Apply the reviewer findings for ${target}: ${verdict}.
                    Fix every BLOCKER named in the verdict — those are mandatory to merge. This is the only round: leave nothing planned for a next pass that does not exist. Re-run the repo validation gates.
                    Then return the full updated work summary replacing ${workSummary}: what changed (files), how behavior was preserved, gate results, open concerns.`;
const scribeText = input.prompt`
                        The single review pass for the refactor of ${target} has ended and the work is being committed. The reviewer verdict is ${verdict} — read its status field FIRST; a status still set to revise means the one round did not fully satisfy the reviewer: say so plainly.
                        Write the commit message from the checklist ${checklist} and that verdict: subject line "refactor(${target}): <boundary in a few words>", then one paragraph summarizing what changed and behavior preservation.
                        If the verdict carries a forgiveness-reason, add one line noting the forgiven plan-fidelity gap and its reason. If status is revise, end with an "Unresolved reviewer blockers:" section listing each open blocker in one line. Omit both sections when not applicable.
                        Return exactly that message as your output; the runtime injects it into the git commit step.
                        Do not run any git commands and do not write any files for the message; staging and committing happen in later runtime steps.`;

export default function () {
    defineVariant("quick", {
        name: "Refactor (Quick)",
        summary:
            "Quick refactoring procedure: turn the target into an actionable checklist, implement it, one reviewer pass, apply that pass once if needed, and commit.",
        metadata: { tag: ["first-party", "refactoring", "review", "multi-agent"] },
        procedure:
            "Refactor one module or entity quickly: an actionable checklist, implementation, exactly one reviewer pass, one round of fixes if needed, commit.",
    });
    useIntent({
        require: [
            intent.require.ReviewBeforeFinal,
            intent.require.BehaviorPreservingDefault,
        ],
        avoid: [
            intent.avoid.RubberStampReview,
            intent.avoid.InterfaceWidening,
        ],
    });
    useResource(architectureDialect);

    smart1.prompt("Checklist the target (smart-1)", { id: "checklist", input: checklistText, output: checklist });
    worker.prompt("Implement the checklist (worker)", { id: "implement", input: implementText, output: workSummary });
    smart1.prompt("Review the implementation (smart-1)", {
        id: "review",
        input: reviewText,
        output: verdict,
        include: [target, checklist, workSummary],
    });
    worker.prompt("Apply the reviewer's pass (worker)", { id: "apply-fixes", input: applyText, output: workSummary });

    step.command("Check working tree status", { id: "check-git-status", argv: ["git", "status", "--porcelain"], output: gitStatus });
    flow.when("Maybe Commit", condition.not(condition.equals(gitStatus, "")), { id: "maybe-commit" }, () => {
        scribe.prompt("Write the commit message (scribe)", { id: "summarization", input: scribeText, output: commitMessage });
        // Two steps, not one excluding add — a pathspec that mentions a
        // gitignored `.agents` exits 1 (run-42bd7fb2).
        step.command("Stage all changes", { id: "git-stage", argv: ["git", "add", "-A"], output: stageOutput });
        step.command("Unstage runtime state", { id: "git-unstage-runtime", argv: ["git", "reset", "-q", "--", ".agents/runs"], output: unstageOutput });
        step.command("Commit the refactor", { id: "git-commit", argv: ["git", "commit", "-m", commitMessage], output: commitOutput });
    });

    return { commitReport };
}
