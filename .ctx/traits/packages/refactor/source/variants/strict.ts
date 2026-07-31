import { deviationReportSchema, STRICT_VARIANT_DOCTRINE } from "@ctx-traits/agents";
import { condition, intent, procedure, prompt, schema, sequence, slot, variant } from "@ctx-traits/cdk";
import { agreedDesign, commitReport, gitStatus, target, workSummary } from "../data.ts";
import { commitMessageStep, gitCommitStep, gitStageStep, guardedCommitTail } from "../sequence/commit.ts";
import { designStep } from "../sequence/design.ts";
import { surveyStep, frameStep } from "../sequence/survey.ts";
import { reviewStep } from "../sequence/refinement.ts";
import { reviewVerdictSchema, verdictSlot } from "../schema.ts";
import { smart1Role, smart2Role, scribe, worker } from "../agent.ts";
import { architectureDialect, smellCatalog } from "../standards.ts";

const smart1 = smart1Role("Strong analysis model: surveys the target against the standards, designs the boundary, and reviews in the refinement loop.");
const smart2 = smart2Role("Strong review model: independently reviews the refactored state, including every recorded deviation, in the refinement loop.");

const deviationReport = slot({
    id: "deviation-report",
    schema: schema.list(deviationReportSchema),
    description:
        "Every proposed departure from the verbatim agreed design — including a rejected improvement the worker considered but did not make — recorded instead of silently adapted. Empty when the design was followed exactly.",
});

const STRICT_DEVIATION_BLOCKER =
    "A BLOCKER also includes an unrecorded deviation from the agreed design, or any recorded deviation that reflects an actual departure rather than a rejected proposal. Approve ONLY verbatim work where every recorded deviation is a rejected proposal — never a proposal that was taken.";

const verdictSchema = reviewVerdictSchema("strict");
const verdict1 = verdictSlot("review-verdict-1", "smart-1", verdictSchema);
const verdict2 = verdictSlot("review-verdict-2", "smart-2", verdictSchema);

export default variant({
    name: "Refactor (Strict)",
    summary:
        "Surveyed refactoring procedure with verbatim execution: survey, frame, design, implement the agreed design exactly as written, refine against two independent reviewers with a typed deviation record, and commit only once both approve.",
    metadata: { tag: ["first-party", "refactoring", "review", "multi-agent"] },
    resource: [architectureDialect, smellCatalog],
    intent: {
        require: [
            intent.require.reviewBeforeFinal,
            intent.require.boundedRefinement,
            { id: "behavior-preserving-default", summary: "Preserve observed behavior unless the phase explicitly changes it." },
            { id: "verbatim-design-execution", summary: "Execute the agreed design exactly as written; record every departure instead of silently adapting." },
        ],
        avoid: [
            intent.avoid.rubberStampReview,
            { id: "interface-widening", summary: "Do not broaden public interfaces without a concrete caller need." },
            { id: "silent-plan-deviation", summary: "Never adapt around an unsatisfiable or contradicted design without recording and disposing of the deviation." },
        ],
    },
    procedure: procedure({
        description:
            "Refactor one module or entity end to end with verbatim design execution: standards-grounded survey, problem framing, boundary design, verbatim implementation with a typed deviation record, bounded dual-reviewer refinement, commit — blocking rather than adapting when the design cannot be satisfied.",
        input: target,
        output: commitReport,
        sequence: [
            surveyStep(smart1, architectureDialect, smellCatalog),
            frameStep(smart1, architectureDialect, smellCatalog),
            designStep(smart1, architectureDialect),
            sequence.prompt("implement", {
                title: "Implement the design verbatim (worker)",
                agent: worker,
                text: prompt.text`
                    Implement the agreed design ${agreedDesign} for ${target} VERBATIM: execute exactly as specified, with no improvisation and no ACTUAL departure from what is written, however minor.
                    If the design is unsatisfiable as written — it contradicts the actual code, omits a step you need, or cannot be executed against reality — STOP and say so plainly in your work summary rather than adapting around it; do not implement a modified version.
                    If you notice a place the design could be improved, do NOT take it: record it as a typed deviation report entry instead — what was planned, what you actually did (which must match the design verbatim), the rationale for the proposed improvement, and disposition "rejected — implemented verbatim". A deviation report entry never records something you actually did differently from the design; an actual departure means the design was unsatisfiable, and that means STOP, not a recorded deviation.
                    Respect the dialect (${architectureDialect}); do not widen any interface to make a caller compile — fix the caller. Run the repo validation gates before reporting.
                    Return exactly two outputs: work-summary (what changed, files, gate results, open concerns) and deviation-report (every rejected-but-considered improvement recorded above; an empty list when none arose).`,
                output: [workSummary, deviationReport],
            }),
            sequence.loop("refinement-loop", {
                title: "Doubly-reviewed verbatim refinement",
                sequence: sequence.linear("refine-refactor", [
                    reviewStep("review-1", 1, smart1, architectureDialect, STRICT_VARIANT_DOCTRINE, verdict1, {
                        deviationReport,
                        extraInstruction: STRICT_DEVIATION_BLOCKER,
                    }),
                    reviewStep("review-2", 2, smart2, architectureDialect, STRICT_VARIANT_DOCTRINE, verdict2, {
                        deviationReport,
                        extraInstruction: STRICT_DEVIATION_BLOCKER,
                    }),
                    sequence.prompt("apply-fixes", {
                        title: "Apply reviewer fixes verbatim (worker)",
                        agent: worker,
                        text: prompt.text`
                            Apply the reviewer findings for ${target}: ${verdict1} and ${verdict2}.
                            Fix every BLOCKER named in either verdict while staying strictly verbatim to the agreed design ${agreedDesign} — never introduce a new ACTUAL departure to satisfy a finding. If a blocker can only be fixed by actually departing from the design, do not deviate: STOP and say so plainly in the work summary so the loop can exhaust and block rather than silently adapting. Respect the dialect (${architectureDialect}). Re-run the repo validation gates.
                            Return exactly two outputs: work-summary (replacing the prior one: what changed, files, gate results, open concerns) and deviation-report (the complete updated list of rejected-but-considered proposals, including any new ones this round; never an entry describing an actual departure).`,
                        output: [workSummary, deviationReport],
                    }),
                ]),
                until: condition.all([
                    condition.fieldEquals(verdict1, "status", "approved"),
                    condition.fieldEquals(verdict2, "status", "approved"),
                ]),
                iterations: 6,
                onExhausted: "block",
            }),
            ...guardedCommitTail(gitStatus, [
                commitMessageStep(scribe, verdict1, verdict2),
                gitStageStep,
                gitCommitStep,
            ]),
        ],
    }),
});
