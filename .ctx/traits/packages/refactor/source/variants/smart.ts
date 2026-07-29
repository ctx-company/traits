import { planAmendmentSchema, SMART_VARIANT_DOCTRINE } from "@ctx-traits/agents";
import { condition, Intent, procedure, prompt, schema, sequence, slot, variant } from "@ctx-traits/cdk";
import { agreedDesign, commitReport, gitStatus, survey, target, workSummary } from "../data.ts";
import { commitMessageStep, gitCommitStep, gitStageStep, guardedCommitTail } from "../sequence/commit.ts";
import { designStep, implementStep } from "../sequence/design.ts";
import { frameStep } from "../sequence/survey.ts";
import { reviewStep } from "../sequence/refinement.ts";
import { reviewVerdictSchema, verdictSlot } from "../schema.ts";
import { smart1Role, smart2Role, scribe, worker } from "../agent.ts";
import { architectureDialect, smellCatalog } from "../standards.ts";

const smart1 = smart1Role(
    "Strong analysis model with research tools: researches prior art before surveying, designs the boundary, and reviews in the refinement loop.",
);
const smart2 = smart2Role("Strong review model: independently reviews the refactored state in the refinement loop.");

const CROSS_REVIEWER_CONFLICT_BLOCKER =
    "Treat a work summary that reports an unresolved cross-reviewer amendment conflict as a BLOCKER in your verdict: the conflict stays open until both reviewers reconcile it in a later round, and status must not be approved while it remains.";

const researchNotes = slot.text({
    id: "research-notes",
    description:
        "Prior art and precedent gathered before the survey: how comparable codebases or the house's own history handled this boundary, and any external guidance worth grounding the design in.",
    hint: "Better inputs, not more rounds — this step runs once, before the survey, and is never repeated.",
});

const verdictSchema = reviewVerdictSchema("smart", {
    amendments: schema.field(schema.list(planAmendmentSchema), {
        required: false,
        description:
            "Typed amendments to the agreed design under smart authority, present when the plan and the observed work genuinely contradicted. Each becomes the binding contract going forward; an unrecorded departure is still a plan-fidelity gap, not an amendment.",
    }),
});
const verdict1 = verdictSlot("review-verdict-1", "smart-1", verdictSchema);
const verdict2 = verdictSlot("review-verdict-2", "smart-2", verdictSchema);

export default variant({
    name: "Refactor (Smart)",
    summary:
        "Research-informed surveyed refactoring procedure: research prior art, survey the target, frame the problem, design the boundary, implement it, refine against two independent reviewers who may amend the design, and commit.",
    metadata: { tag: ["first-party", "refactoring", "review", "multi-agent"] },
    resource: [architectureDialect, smellCatalog],
    intent: {
        require: [
            Intent.require.reviewBeforeFinal,
            Intent.require.boundedRefinement,
            { id: "behavior-preserving-default", summary: "Preserve observed behavior unless the phase explicitly changes it." },
        ],
        avoid: [
            Intent.avoid.unboundedLoop,
            Intent.avoid.rubberStampReview,
            { id: "interface-widening", summary: "Do not broaden public interfaces without a concrete caller need." },
            { id: "taste-only-findings", summary: "Do not block on subjective style without a concrete behavioral consequence." },
        ],
    },
    procedure: procedure({
        description:
            "Refactor one module or entity end to end with a research-informed survey: research, survey, problem framing, boundary design, implementation, bounded dual-reviewer refinement with typed amendments, commit.",
        input: target,
        output: commitReport,
        sequence: [
            sequence.prompt("research", {
                title: "Research prior art (smart-1)",
                agent: smart1,
                text: prompt.text`
                    Before surveying ${target}, research prior art: how comparable modules in this codebase solved a similar boundary problem, and any external precedent worth grounding the design in. Use web-capable tools if available.
                    Keep this to one pass — the loop budget is unchanged from the default flow; smart means a better-informed survey and design, not more rounds.
                    Return your research notes: what you found, its relevance, and anything that should constrain the coming design.`,
                output: researchNotes,
            }),
            sequence.prompt("survey", {
                title: "Survey the target (smart-1)",
                agent: smart1,
                text: prompt.text`
                    Survey ${target} for refactoring opportunities, informed by your research ${researchNotes}.
                    Read the target and its callers/callees with your tools — explore organically; the friction you encounter while reading IS the signal. Ground every observation in the architecture dialect (${architectureDialect}) and the smell catalog (${smellCatalog}); cite smell ids (S1-S10) or a named deep-module violation for each.
                    Sweep is mandatory and complete: (a) walk ALL ten smells S1-S10 in catalog order and for each report findings or explicitly "S<n>: none found"; (b) then assess the three dialect pillars — boundaries, data flow, entity containment.
                    Return a numbered candidate list — for each: the cluster of code involved (paths), why it is coupled, which smell ids or dialect pillar it violates, and how the boundary would change what tests can prove.
                    Do NOT propose interfaces or fixes yet. List every real candidate even if it cannot fit one run.`,
                output: survey,
            }),
            frameStep(smart1, architectureDialect, smellCatalog),
            designStep(smart1, architectureDialect),
            implementStep(worker, architectureDialect),
            sequence.loop("refinement-loop", {
                title: "Doubly-reviewed refinement with amendments",
                sequence: sequence.linear("refine-refactor", [
                    reviewStep("review-1", 1, smart1, architectureDialect, SMART_VARIANT_DOCTRINE, verdict1, {
                        extraInstruction: CROSS_REVIEWER_CONFLICT_BLOCKER,
                    }),
                    reviewStep("review-2", 2, smart2, architectureDialect, SMART_VARIANT_DOCTRINE, verdict2, {
                        extraInstruction: CROSS_REVIEWER_CONFLICT_BLOCKER,
                    }),
                    sequence.prompt("apply-fixes", {
                        title: "Apply reviewer fixes and amendments (worker)",
                        agent: worker,
                        text: prompt.text`
                            Apply the reviewer findings for ${target}: ${verdict1} and ${verdict2}.
                            Fix every BLOCKER named in either verdict — those are mandatory to merge. When a verdict carries amendments, those become the new binding contract: rewrite the agreed design to incorporate them exactly, then implement to the amended design. If verdict1 and verdict2 propose amendments that genuinely conflict with each other, do NOT adjudicate between them yourself: leave the agreed design unamended on the conflicting point, implement neither side of the conflict, and report the conflict verbatim (both proposed amendments and why they contradict) as an open item in the work summary — the conflict must remain unresolved and blocking until the reviewers reconcile it in a later round, never silently decided here. Advisory / deferred items go on the deferred-candidates list, not this round, unless the fix is cheap and safe. Prefer the dialect (${architectureDialect}). Re-run the repo validation gates.
                            Return exactly two outputs: agreed-design (the complete design text, amended by every non-conflicting amendment, unchanged on any conflicting point) and work-summary (what changed, files, net line delta, how behavior was preserved, gate results, open concerns — including any unresolved amendment conflict, named explicitly so the next review round blocks on it).`,
                        output: [agreedDesign, workSummary],
                    }),
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
