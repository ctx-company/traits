import { input, output } from "@ctx-traits/cdk";

import { agreedDesign, refactorFrame, target } from "../../data.ts";

export const inputPrompt = input.prompt`
    Design the refactor for the framed problem ${refactorFrame} on ${target}.
    Optimize for both faces of the standard at once: the deepest practical module — few entry points hiding the full implementation — expressed in the house dialect (Interface/Service pairing per responsibility, surface-agnostic layers over typed Request/Response, typed Event/Command enums normalized once at the edge, entity containment, Context carriage where values re-thread, typed module-owned errors, Response enums instead of display strings). Where depth and dialect pull apart, be opinionated about the trade and say why; weigh how much the design DELETES — replacement that consumes its predecessor beats addition beside it (S9).
    Stay within the framing's constraints.
`;

export const outputPrompt = output.prompt`
    Return the complete agreed design ready to implement from: final boundaries and types, the interface signatures with one usage example per caller class, what complexity gets hidden, file-by-file migration steps, what stays byte-stable, the expected net line delta with what any growth buys, and the validation plan (the repo's standard gates). Close with an explicit "MUST" and "MUST-NOT" list — the concrete, checkable requirements a reviewer enforcing plan fidelity applies verbatim; do not leave fidelity to be inferred from prose elsewhere in the design. (${agreedDesign})
`;
