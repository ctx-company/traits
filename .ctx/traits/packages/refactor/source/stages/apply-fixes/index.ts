import type { PromptTemplate, SlotHandle } from "@ctx-traits/cdk";
import { input, output } from "@ctx-traits/cdk";

import { agreedDesign, target, workDeviationReport, workSummary } from "../../data.ts";

export function applyFixesPrompt(verdict1: SlotHandle, verdict2: SlotHandle): PromptTemplate {
    return input.prompt`
        Apply the reviewer findings for ${target}: ${verdict1} and ${verdict2}.
        Fix every BLOCKER named in either verdict while staying strictly verbatim to the agreed design ${agreedDesign} — never introduce a new ACTUAL departure to satisfy a finding. If a blocker can only be fixed by actually departing from the design, do not deviate: STOP and say so plainly in the work summary so the loop can exhaust and block rather than silently adapting.
    `;
}

export const applyFixesOutput = output.prompt`
    Return exactly two outputs:
    - the updated work summary replacing the prior one: what changed, files, gate results, open concerns (${workSummary})
    - the complete updated deviation report: every rejected-but-considered proposal, including any new ones this round (${workDeviationReport})
`;
