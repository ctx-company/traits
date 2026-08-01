import { intent, procedure, variant } from "@ctx-traits/cdk";

import { directWorkReport } from "../../direct-data.ts";
import {
    directAnnotationStep,
    directChecklistStep,
    directImplementationStep,
} from "../../sequence/direct.ts";

export default variant({
    name: "Refactor (Direct)",
    summary: "Turns human annotations into directly implemented changes — no review loop, no commit.",
    metadata: { tag: ["first-party", "refactoring", "annotations"] },
    intent: {
        require: [
            intent.require.AnnotationFidelity,
            intent.require.PreserveScope,
        ],
        avoid: [intent.avoid.ScopeCreep, intent.avoid.OverEngineering],
    },
    port: directWorkReport,
    procedure: procedure({
        description: "Collect human annotations interactively, plan them as a checklist, implement every item, and report.",
        sequence: [directAnnotationStep, directChecklistStep, directImplementationStep],
    }),
});
