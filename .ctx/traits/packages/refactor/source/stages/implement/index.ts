import { input, output } from "@ctx-traits/cdk";

import { agreedDesign, target, workDeviationReport, workSummary } from "../../data.ts";

export const inputPrompt = input.prompt`
    Implement the agreed design ${agreedDesign} for ${target} VERBATIM: execute exactly as specified, with no improvisation and no ACTUAL departure from what is written, however minor.
    If the design is unsatisfiable as written — it contradicts the actual code, omits a step you need, or cannot be executed against reality — STOP and say so plainly in your work summary.
    If you notice a place the design could be improved, do NOT take it: record it as a typed deviation report entry instead — what was planned, what you actually did.
`;

export const outputPrompt = output.prompt`
    Return exactly two outputs:
    - what changed, files, gate results, open concerns (${workSummary})
    - every rejected-but-considered improvement recorded above (${workDeviationReport})
`;
