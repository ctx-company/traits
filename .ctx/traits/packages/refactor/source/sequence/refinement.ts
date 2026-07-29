import { INTEGRITY_DOCTRINE } from "@ctx-traits/agents";
import type { AgentHandle, ResourceHandle, SlotHandle } from "@ctx-traits/cdk";
import { prompt, sequence } from "@ctx-traits/cdk";
import { agreedDesign, target, workSummary } from "../data.ts";

export function reviewStep(id: string, reviewerNumber: 1 | 2, agent: AgentHandle, dialect: ResourceHandle, variantDoctrine: string, output: SlotHandle, opts: { deviationReport?: SlotHandle; extraInstruction?: string } = {}) {
    const { deviationReport, extraInstruction } = opts;
    return sequence.prompt(id, {
        title: `Review refactor (smart-${reviewerNumber})`, agent,
        text: prompt.template(
            `${reviewerNumber === 2 ? "Independently review" : "Review"} the refactored state of {target} against the agreed design {agreedDesign}. Current work summary: {workSummary}.${deviationReport ? " Recorded deviations: {deviationReport}. Verify every recorded deviation is a genuinely rejected-but-considered proposal — the actual implementation still matches the design verbatim on that point — not a departure that was actually taken; silent adaptation not recorded here is itself a BLOCKER." : ""}
            A BLOCKER always includes a behavior or byte-stability break, a NEW S1-S10 smell introduced by this change (including S9 — code this change supersedes surviving beside its replacement), or an interface widened to make a caller compile; cite the smell id or the deep-module violation. Fidelity to the agreed design is judged SOLELY by the authority rule below — apply it directly, with no separate categorical "any design violation is a blocker" rule layered on top. Residual pre-existing smells outside the framed change, code the agreed design did not cover, and further refactoring the survey deferred are ADVISORY.
            Review the AGREED framed change, not the whole surface: approve when the framed change is correct, behavior-preserving, and adds no new smell — even if the broader ideal remains — and record everything else as advisory for the deferred list. Do not block because the surface is not yet fully ideal.
            ${variantDoctrine}${extraInstruction ? `\n            ${extraInstruction}` : ""}
            Wherever the authority rule above names REVIEW_VERDICT_DOCTRINE, it means the composed baseline stated immediately below — this family splices the leaner, family-compatible INTEGRITY_DOCTRINE in its place, not the implement-phase doctrine.
            ${INTEGRITY_DOCTRINE}`,
            { target, agreedDesign, workSummary, phaseBrief: agreedDesign, productBrief: dialect, ...(deviationReport ? { deviationReport } : {}) },
        ),
        output,
        input: deviationReport ? [target, agreedDesign, workSummary, deviationReport] : [target, agreedDesign, workSummary],
    });
}

export function applyFixesStep(agent: AgentHandle, dialect: ResourceHandle, verdict1: SlotHandle, verdict2: SlotHandle, output: SlotHandle | readonly SlotHandle[] = workSummary) {
    return sequence.prompt("apply-fixes", {
        title: "Apply reviewer fixes (worker)", agent,
        text: prompt.text`
            Apply the reviewer findings for ${target}: ${verdict1} and ${verdict2}.
            Fix every BLOCKER named in either verdict — those are mandatory to merge. Advisory / deferred items go on the deferred-candidates list, not this round, unless the fix is cheap and safe. Prefer the agreed design ${agreedDesign} and the dialect (${dialect}). Re-run the repo validation gates.
            Then return the full updated work summary replacing ${workSummary}: what changed (files), the net line delta and what any growth buys, how behavior was preserved, gate results, open concerns.`,
        output,
    });
}
