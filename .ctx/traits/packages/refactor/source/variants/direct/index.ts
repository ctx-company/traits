import { intent, procedure, variant } from "@ctx-traits/cdk";

import { port } from "./data.ts";
import annotate from "./sequence/annotation.ts";
import checklist from "./sequence/checklist.ts";
import implement from "./sequence/implementation.ts";

export default variant({
    name: "Refactor (Direct)",
    summary: "Turns human annotations into directly implemented changes — no review loop, no commit.",
    metadata: { tag: ["first-party", "refactoring", "annotations"] },
    intent: {
        require: [
            { id: "annotation-fidelity", summary: "Every annotation becomes exactly one checklist item; nothing is dropped or merged away." },
            intent.PreserveScope,
        ],
        avoid: [intent.ScopeCreep, intent.OverEngineering],
    },
    procedure: procedure({
        description: "Collect human annotations interactively, plan them as a checklist, implement every item, and report.",
        output: port.workReport,
        sequence: [annotate, checklist, implement],
    }),
});
