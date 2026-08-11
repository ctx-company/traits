import { input, output } from "@ctx-traits/cdk";

import { survey, target } from "../../data.ts";

export const inputPrompt = input.prompt`
    Survey ${target} for refactoring opportunities.
    Read the target and its callers/callees with your tools — explore organically; the friction you encounter while reading IS the signal. Ground every observation in the architecture dialect and the smell catalog; cite smell ids (S1-S10) or a named deep-module violation for each.
    Sweep is mandatory and complete: (a) walk ALL ten smells S1-S10 in catalog order and for each report findings or explicitly "S<n>: none found"; (b) then assess the three dialect pillars — boundaries (missing Interface/Service contracts, mixed control/view/trace responsibilities, surface-coupled layers), data flow (untyped events/commands, string dispatch), entity containment (entity vocabulary leaked across modules).
    Do NOT propose interfaces or fixes yet.
`;

export const outputPrompt = output.prompt`
    Return a numbered candidate list — for each: the cluster of code involved (paths), why it is coupled, which smell ids or dialect pillar it violates, and how the boundary would change what tests can prove. List every real candidate even if it cannot fit one run. (${survey})
`;
