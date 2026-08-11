import { STRICT_VARIANT_DOCTRINE } from "@ctx-traits/agents";
import { condition, defineVariant, flow, step, useIntent, useResource } from "@ctx-traits/cdk";

import {
    commitMessage,
    commitOutput,
    commitReport,
    gitStatus,
    stageOutput,
    unstageOutput,
    workDeviationReport,
} from "../../data.ts";
import { reviewVerdictSchema, verdictSlot } from "../../schema.ts";
import { smart1Role, smart2Role, scribe, worker } from "../../agent.ts";
import * as shared from "../../shared/index.ts";
import * as stages from "../../stages/index.ts";

const smart1 = smart1Role(
    "Strong analysis model: surveys the target against the standards, designs the boundary, and reviews in the refinement loop.",
);
const smart2 = smart2Role(
    "Strong review model: independently reviews the refactored state, including every recorded deviation, in the refinement loop.",
);

const STRICT_DEVIATION_SCOPE =
    "Recorded deviations: {deviationReport}. Verify every recorded deviation is a genuinely rejected-but-considered proposal — the actual implementation still matches the design verbatim on that point — not a departure that was actually taken; silent adaptation not recorded here is itself a BLOCKER.";
const STRICT_DEVIATION_BLOCKER =
    "A BLOCKER also includes an unrecorded deviation from the agreed design, or any recorded deviation that reflects an actual departure rather than a rejected proposal. Approve ONLY verbatim work where every recorded deviation is a rejected proposal — never a proposal that was taken.";

const verdictSchema = reviewVerdictSchema("strict");
const verdict1 = verdictSlot("review-verdict-1", "smart-1", verdictSchema);
const verdict2 = verdictSlot("review-verdict-2", "smart-2", verdictSchema);

// One text for both reviewers — independence lives in the agents and their
// separate verdict slots, not in divergent wording.
const reviewText = stages.review.reviewPrompt({
    doctrine: STRICT_VARIANT_DOCTRINE,
    scope: STRICT_DEVIATION_SCOPE,
    rules: [STRICT_DEVIATION_BLOCKER],
    bindings: { deviationReport: workDeviationReport },
});
const applyFixesText = stages.applyFixes.applyFixesPrompt(verdict1, verdict2);
const commitMessageText = stages.commit.commitMessagePrompt(verdict1, verdict2);

export default function () {
    defineVariant("strict", {
        name: "Refactor (Strict)",
        summary:
            "Surveyed refactoring procedure with verbatim execution: survey, frame, design, implement the agreed design exactly as written, refine against two independent reviewers with a typed deviation record, and commit only once both approve.",
        metadata: { tag: ["first-party", "refactoring", "review", "multi-agent"] },
        procedure:
            "Refactor one module or entity end to end with verbatim design execution: standards-grounded survey, problem framing, boundary design, verbatim implementation with a typed deviation record, bounded dual-reviewer refinement, commit — blocking rather than adapting when the design cannot be satisfied.",
    });

    useIntent(shared.strictIntent);
    useResource(shared.resources);

    smart1.prompt("Survey the target", {
        id: "survey",
        input: stages.survey.inputPrompt,
        output: stages.survey.outputPrompt,
    });

    smart1.prompt("Frame the problem", {
        id: "frame",
        input: stages.frame.inputPrompt,
        output: stages.frame.outputPrompt,
    });

    smart1.prompt("Design the boundary", {
        id: "design",
        input: stages.design.inputPrompt,
        output: stages.design.outputPrompt,
    });

    worker.prompt("Implement the design specification", {
        id: "implement",
        input: stages.implement.inputPrompt,
        output: stages.implement.outputPrompt,
    });

    flow.loop("Doubly-reviewed verbatim refinement", (loop) => {
        loop.id("refinement-loop");
        loop.maxIterations(6, { onExhausted: "abort" });
        smart1.prompt("Review refactor", {
            id: "review-1",
            input: reviewText,
            output: verdict1,
        });
        smart2.prompt("Cross-review refactor", {
            id: "review-2",
            input: reviewText,
            output: verdict2,
        });
        worker.prompt("Apply reviewer fixes verbatim", {
            id: "apply-fixes",
            input: applyFixesText,
            output: stages.applyFixes.applyFixesOutput,
        });
        flow.until(
            condition.all([
                condition.fieldEquals(verdict1, "status", "approved"),
                condition.fieldEquals(verdict2, "status", "approved"),
            ]),
        );
    });

    step.command("Check working tree status", {
        id: "check-git-status",
        argv: ["git", "status", "--porcelain"],
        output: gitStatus,
    });
    flow.when(
        "Maybe Commit",
        condition.not(condition.equals(gitStatus, "")),
        { id: "maybe-commit" },
        () => {
            scribe.prompt("Write the commit message", {
                id: "summarization",
                input: commitMessageText,
                output: stages.commit.commitMessageOutput,
            });
            // Two steps, not one excluding add — a pathspec that mentions a
            // gitignored `.agents` exits 1 (run-42bd7fb2).
            step.command("Stage all changes", {
                id: "git-stage",
                argv: ["git", "add", "-A"],
                output: stageOutput,
            });
            step.command("Unstage runtime state", {
                id: "git-unstage-runtime",
                argv: ["git", "reset", "-q", "--", ".agents/runs"],
                output: unstageOutput,
            });
            step.command("Commit the refactor", {
                id: "git-commit",
                argv: ["git", "commit", "-m", commitMessage],
                output: commitOutput,
            });
        },
    );

    return { commitReport };
}
