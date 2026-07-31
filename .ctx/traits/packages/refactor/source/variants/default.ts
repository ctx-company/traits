import { DEFAULT_VARIANT_DOCTRINE } from "@ctx-traits/agents";
import { condition, intent, procedure, sequence, variant } from "@ctx-traits/cdk";
import { applyFixesStep, reviewStep } from "../sequence/refinement.ts";
import { commitMessageStep, gitCommitStep, gitStageStep, guardedCommitTail } from "../sequence/commit.ts";
import { commitReport, gitStatus, target, workSummary } from "../data.ts";
import { smart1Role, smart2Role, scribe, worker } from "../agent.ts";
import { reviewVerdictSchema, verdictSlot } from "../schema.ts";
import { designStep, implementStep } from "../sequence/design.ts";
import { frameStep, surveyStep } from "../sequence/survey.ts";
import { architectureDialect, smellCatalog } from "../standards.ts";

const smart1 = smart1Role("Strong analysis model: surveys the target against the standards, designs the boundary, and reviews in the refinement loop.");
const smart2 = smart2Role("Strong review model: independently reviews the refactored state in the refinement loop.");

const verdictSchema = reviewVerdictSchema("default");
const verdict1 = verdictSlot("review-verdict-1", "smart-1", verdictSchema);
const verdict2 = verdictSlot("review-verdict-2", "smart-2", verdictSchema);

export default variant({
    name: "Refactor (Default)",
    summary:
        "Surveyed refactoring procedure: survey the target against the house standards, frame the problem, design the boundary, implement it, refine against two independent reviewers, and commit.",
    metadata: { tag: ["first-party", "refactoring", "review", "multi-agent"] },
    intent: {
        require: [
            intent.require.reviewBeforeFinal,
            intent.require.boundedRefinement,
            { id: "behavior-preserving-default", summary: "Preserve observed behavior unless the phase explicitly changes it." },
        ],
        avoid: [
            intent.avoid.unboundedLoop,
            intent.avoid.rubberStampReview,
            { id: "interface-widening", summary: "Do not broaden public interfaces without a concrete caller need." },
            { id: "taste-only-findings", summary: "Do not block on subjective style without a concrete behavioral consequence." },
        ],
    },
    resource: [architectureDialect, smellCatalog],
    procedure: procedure({
        description:
            "Refactor one module or entity end to end: standards-grounded survey, problem framing, boundary design, implementation, bounded dual-reviewer refinement, commit.",
        input: target,
        output: commitReport,
        sequence: [
            surveyStep(smart1, architectureDialect, smellCatalog),
            frameStep(smart1, architectureDialect, smellCatalog),
            designStep(smart1, architectureDialect),
            implementStep(worker, architectureDialect),
            sequence.loop("refinement-loop", {
                title: "Doubly-reviewed refinement",
                sequence: sequence.linear("refine-refactor", [
                    reviewStep("review-1", 1, smart1, architectureDialect, DEFAULT_VARIANT_DOCTRINE, verdict1),
                    reviewStep("review-2", 2, smart2, architectureDialect, DEFAULT_VARIANT_DOCTRINE, verdict2),
                    applyFixesStep(worker, architectureDialect, verdict1, verdict2, workSummary),
                ]),
                until: condition.all([
                    condition.fieldEquals(verdict1, "status", "approved"),
                    condition.fieldEquals(verdict2, "status", "approved"),
                ]),
                iterations: 6,
            }),
            ...guardedCommitTail(gitStatus, [
                commitMessageStep(scribe, verdict1, verdict2),
                gitStageStep,
                gitCommitStep,
            ]),
        ],
    }),
});
