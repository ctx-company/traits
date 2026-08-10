import { DEFAULT_VARIANT_DOCTRINE, INTEGRITY_DOCTRINE } from "@ctx-traits/agents";
import { condition, defineVariant, flow, input, intent, step, useIntent, useResource } from "@ctx-traits/cdk";
import { agreedDesign, commitMessage, commitOutput, commitReport, gitStatus, refactorFrame, stageOutput, survey, target, unstageOutput, workSummary } from "../data.ts";
import { smart1Role, smart2Role, scribe, worker } from "../agent.ts";
import { reviewVerdictSchema, verdictSlot } from "../schema.ts";
import { architectureDialect, smellCatalog } from "../resource.ts";

const smart1 = smart1Role("Strong analysis model: surveys the target against the standards, designs the boundary, and reviews in the refinement loop.");
const smart2 = smart2Role("Strong review model: independently reviews the refactored state in the refinement loop.");

const verdictSchema = reviewVerdictSchema("default");
const verdict1 = verdictSlot("review-verdict-1", "smart-1", verdictSchema);
const verdict2 = verdictSlot("review-verdict-2", "smart-2", verdictSchema);

const surveyStepText = input.prompt`
            Survey ${target} for refactoring opportunities.
            Read the target and its callers/callees with your tools — explore organically; the friction you encounter while reading IS the signal. Ground every observation in the architecture dialect (${architectureDialect}) and the smell catalog (${smellCatalog}); cite smell ids (S1-S10) or a named deep-module violation for each.
            Sweep is mandatory and complete: (a) walk ALL ten smells S1-S10 in catalog order and for each report findings or explicitly "S<n>: none found"; (b) then assess the three dialect pillars — boundaries (missing Interface/Service contracts, mixed control/view/trace responsibilities, surface-coupled layers), data flow (untyped events/commands, string dispatch), entity containment (entity vocabulary leaked across modules).
            Return a numbered candidate list — for each: the cluster of code involved (paths), why it is coupled, which smell ids or dialect pillar it violates, and how the boundary would change what tests can prove.
            Do NOT propose interfaces or fixes yet. List every real candidate even if it cannot fit one run.`;

const frameStepText = input.prompt`
            From the survey ${survey} of ${target}, select the highest-value candidate set that honestly fits one refactoring run; name the deferred candidates explicitly so they are not lost.
            Verify the selection against the actual code with your tools, then write the problem framing: the constraints any new boundary must satisfy (callers, serialized shapes that must stay byte-stable, gates), the dependencies involved, and a short illustrative sketch that grounds the space without prescribing the answer.
            Judge with ${architectureDialect} and ${smellCatalog}; behavior-preserving is the default contract.`;

const designStepText = input.prompt`
            Design the refactor for the framed problem ${refactorFrame} on ${target}.
            Optimize for both faces of the standard at once: the deepest practical module — few entry points hiding the full implementation — expressed in the house dialect in ${architectureDialect} (Interface/Service pairing per responsibility, surface-agnostic layers over typed Request/Response, typed Event/Command enums normalized once at the edge, entity containment, Context carriage where values re-thread, typed module-owned errors, Response enums instead of display strings). Where depth and dialect pull apart, be opinionated about the trade and say why; weigh how much the design DELETES — replacement that consumes its predecessor beats addition beside it (S9).
            Return the complete agreed design ready to implement from: final boundaries and types, the interface signatures with one usage example per caller class, what complexity gets hidden, file-by-file migration steps, what stays byte-stable, the expected net line delta with what any growth buys, and the validation plan (the repo's standard gates). Close with an explicit "MUST" and "MUST-NOT" list — the concrete, checkable requirements a reviewer enforcing plan fidelity applies verbatim; do not leave fidelity to be inferred from prose elsewhere in the design. Stay within the framing's constraints.`;

const implementStepText = input.prompt`
            Implement the agreed design ${agreedDesign} for ${target}.
            Behavior-preserving is the contract unless the design explicitly says otherwise: serialized shapes, digests, and CLI output stay byte-stable. Respect the dialect (${architectureDialect}); do not widen any interface to make a caller compile — fix the caller.
            Prefer deletion: code your change supersedes is removed in the same change, never left beside its replacement (S9). Run the repo validation gates before reporting. Return a work summary: what changed (files), the net line delta and what any growth buys, how behavior was preserved, gate results, open concerns.`;

const review1Text = input.prompt(
        `Review the refactored state of {target} against the agreed design {agreedDesign}. Current work summary: {workSummary}.
            A BLOCKER always includes a behavior or byte-stability break, a NEW S1-S10 smell introduced by this change (including S9 — code this change supersedes surviving beside its replacement), or an interface widened to make a caller compile; cite the smell id or the deep-module violation. Fidelity to the agreed design is judged SOLELY by the authority rule below — apply it directly, with no separate categorical "any design violation is a blocker" rule layered on top. Residual pre-existing smells outside the framed change, code the agreed design did not cover, and further refactoring the survey deferred are ADVISORY.
            Review the AGREED framed change, not the whole surface: approve when the framed change is correct, behavior-preserving, and adds no new smell — even if the broader ideal remains — and record everything else as advisory for the deferred list. Do not block because the surface is not yet fully ideal.
            ${DEFAULT_VARIANT_DOCTRINE}
            Wherever the authority rule above names REVIEW_VERDICT_DOCTRINE, it means the composed baseline stated immediately below — this family splices the leaner, family-compatible INTEGRITY_DOCTRINE in its place, not the implement-phase doctrine.
            ${INTEGRITY_DOCTRINE}`,
        { target, agreedDesign, workSummary, phaseBrief: agreedDesign, productBrief: architectureDialect },
    );

const review2Text = input.prompt(
        `Independently review the refactored state of {target} against the agreed design {agreedDesign}. Current work summary: {workSummary}.
            A BLOCKER always includes a behavior or byte-stability break, a NEW S1-S10 smell introduced by this change (including S9 — code this change supersedes surviving beside its replacement), or an interface widened to make a caller compile; cite the smell id or the deep-module violation. Fidelity to the agreed design is judged SOLELY by the authority rule below — apply it directly, with no separate categorical "any design violation is a blocker" rule layered on top. Residual pre-existing smells outside the framed change, code the agreed design did not cover, and further refactoring the survey deferred are ADVISORY.
            Review the AGREED framed change, not the whole surface: approve when the framed change is correct, behavior-preserving, and adds no new smell — even if the broader ideal remains — and record everything else as advisory for the deferred list. Do not block because the surface is not yet fully ideal.
            ${DEFAULT_VARIANT_DOCTRINE}
            Wherever the authority rule above names REVIEW_VERDICT_DOCTRINE, it means the composed baseline stated immediately below — this family splices the leaner, family-compatible INTEGRITY_DOCTRINE in its place, not the implement-phase doctrine.
            ${INTEGRITY_DOCTRINE}`,
        { target, agreedDesign, workSummary, phaseBrief: agreedDesign, productBrief: architectureDialect },
    );

const applyFixesText = input.prompt`
            Apply the reviewer findings for ${target}: ${verdict1} and ${verdict2}.
            Fix every BLOCKER named in either verdict — those are mandatory to merge. Advisory / deferred items go on the deferred-candidates list, not this round, unless the fix is cheap and safe. Prefer the agreed design ${agreedDesign} and the dialect (${architectureDialect}). Re-run the repo validation gates.
            Then return the full updated work summary replacing ${workSummary}: what changed (files), the net line delta and what any growth buys, how behavior was preserved, gate results, open concerns.`;

const commitMessageStepText = input.prompt`
            The refinement loop for the refactor of ${target} has ended and the work is being committed. The final reviewer verdicts are ${verdict1} and ${verdict2} — read their status fields FIRST. The loop exits early once both approve, so a status still set to revise means the rounds ran out with that reviewer unsatisfied: say so plainly rather than implying a clean approval.
            Write the commit message from the agreed design ${agreedDesign}, the survey ${survey}, and those verdicts: subject line "refactor(${target}): <boundary in a few words>", then one paragraph summarizing the new boundary and behavior preservation, then a short "Deferred candidates:" list copied from the survey (omit the list if none were deferred).
            If either verdict status is revise, end the message with an "Unresolved reviewer blockers:" section listing each open blocker in one line — the honest record that the budget ran out before every reviewer was satisfied. Never write that section when there are none.
            Return exactly that message as your output; the runtime injects it into the git commit step.
            Do not run any git commands and do not write any files for the message; staging and committing happen in later runtime steps.`;

export default function () {
    defineVariant("default", {
        name: "Refactor (Default)",
        summary:
            "Surveyed refactoring procedure: survey the target against the house standards, frame the problem, design the boundary, implement it, refine against two independent reviewers, and commit.",
        metadata: { tag: ["first-party", "refactoring", "review", "multi-agent"] },
        procedure:
            "Refactor one module or entity end to end: standards-grounded survey, problem framing, boundary design, implementation, bounded dual-reviewer refinement, commit.",
    });
    useIntent({
        require: [
            intent.require.ReviewBeforeFinal,
            intent.require.BoundedRefinement,
            intent.require.BehaviorPreservingDefault,
        ],
        avoid: [
            intent.avoid.UnboundedLoop,
            intent.avoid.RubberStampReview,
            intent.avoid.InterfaceWidening,
            intent.avoid.TasteOnlyBlocking,
        ],
    });
    useResource([architectureDialect, smellCatalog]);

    smart1.prompt("Survey the target (smart-1)", { id: "survey", input: surveyStepText, output: survey });
    smart1.prompt("Frame the problem (smart-1)", { id: "frame", input: frameStepText, output: refactorFrame });
    smart1.prompt("Design the boundary (smart-1)", { id: "design", input: designStepText, output: agreedDesign });
    worker.prompt("Implement the refactor (worker)", { id: "implement", input: implementStepText, output: workSummary });

    flow.loop("Doubly-reviewed refinement", (loop) => {
        loop.id("refinement-loop");
        loop.maxIterations(6);
        smart1.prompt("Review refactor (smart-1)", {
            id: "review-1",
            input: review1Text,
            output: verdict1,
            include: [target, agreedDesign, workSummary],
        });
        smart2.prompt("Review refactor (smart-2)", {
            id: "review-2",
            input: review2Text,
            output: verdict2,
            include: [target, agreedDesign, workSummary],
        });
        worker.prompt("Apply reviewer fixes (worker)", { id: "apply-fixes", input: applyFixesText, output: workSummary });
        flow.until(condition.all([
            condition.fieldEquals(verdict1, "status", "approved"),
            condition.fieldEquals(verdict2, "status", "approved"),
        ]));
    });

    step.command("Check working tree status", { id: "check-git-status", argv: ["git", "status", "--porcelain"], output: gitStatus });
    flow.when("Maybe Commit", condition.not(condition.equals(gitStatus, "")), { id: "maybe-commit" }, () => {
        scribe.prompt("Write the commit message (scribe)", { id: "summarization", input: commitMessageStepText, output: commitMessage });
        // Two steps, not one excluding add — a pathspec that mentions a
        // gitignored `.agents` exits 1 (run-42bd7fb2).
        step.command("Stage all changes", { id: "git-stage", argv: ["git", "add", "-A"], output: stageOutput });
        step.command("Unstage runtime state", { id: "git-unstage-runtime", argv: ["git", "reset", "-q", "--", ".agents/runs"], output: unstageOutput });
        step.command("Commit the refactor", { id: "git-commit", argv: ["git", "commit", "-m", commitMessage], output: commitOutput });
    });

    return { commitReport };
}
