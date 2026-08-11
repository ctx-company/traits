import { INTEGRITY_DOCTRINE } from "@ctx-traits/agents";
import type { PromptInterpolation, PromptTemplate } from "@ctx-traits/cdk";
import { input } from "@ctx-traits/cdk";

import { agreedDesign, target, workSummary } from "../../data.ts";
import { architectureDialect } from "../../resource.ts";

export type ReviewPromptFields = {
    /** The variant's authority doctrine, spliced between the opening and the INTEGRITY_DOCTRINE baseline. */
    readonly doctrine: string;
    /** Extra opening-scope text after the work summary (e.g. strict's recorded-deviations verification); may use additional {placeholders} satisfied via `bindings`. */
    readonly scope?: string;
    /** Variant rule paragraphs appended after the doctrine (e.g. strict's deviation blocker). */
    readonly rules?: readonly string[];
    /** Named bindings beyond the shared core (target, agreedDesign, workSummary, phaseBrief, productBrief). */
    readonly bindings?: Readonly<Record<string, PromptInterpolation>>;
};

/**
 * The one review prompt both refinement reviewers receive verbatim —
 * reviewer independence comes from the agents and their separate verdict
 * slots, never from divergent wording. Blocker classes, the advisory
 * split, and the approval rule live on the verdict schema's field
 * descriptions; only the citation instruction is prompt-side, having no
 * schema home.
 */
export function reviewPrompt(fields: ReviewPromptFields): PromptTemplate {
    const scope = fields.scope === undefined ? "" : ` ${fields.scope}`;
    const rules = (fields.rules ?? []).map((rule) => `\n            ${rule}`).join("");
    return input.prompt(
        `Review the refactored state of {target} against the agreed design {agreedDesign}, independently of the other reviewer. Current work summary: {workSummary}.${scope}
            Cite the smell id or the deep-module violation for every blocker.
            ${fields.doctrine}${rules}
            Wherever the authority rule above names REVIEW_VERDICT_DOCTRINE, it means the composed baseline stated immediately below — this family splices the leaner, family-compatible INTEGRITY_DOCTRINE in its place, not the implement-phase doctrine.
            ${INTEGRITY_DOCTRINE}`,
        {
            target,
            agreedDesign,
            workSummary,
            phaseBrief: agreedDesign,
            productBrief: architectureDialect,
            ...fields.bindings,
        },
    );
}
