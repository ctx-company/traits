import { intent, method, procedure, tone, variant, verbosity } from "@ctx-traits/cdk";
import { directScribe, directSmart1 } from "../agent.ts";
import { result, taskFiles, taskInput, writtenFiles } from "../data.ts";
import { formatTaskStep } from "../sequence/formatting.ts";
import { writeTasksStep } from "../sequence/writing.ts";

export default variant({
    name: "Plan (Direct)",
    summary: "Format a described task into one well-formed house-format task file with wording kept near-verbatim, then add it to .internal/tasks/ — no refinement, no invention, the fastest path to just implement.",
    metadata: { tag: ["task", "plan", "bootstrap", "planning"] },
    behavior: { tone: [tone.Direct, tone.Technical], method: method.EvidenceFirst, verbosity: verbosity.Brief },
    intent: { focus: [intent.focus.Specific, intent.focus.Correctness], avoid: [intent.avoid.SpeculativeClaim, intent.avoid.ScopeCreep] },
    port: writtenFiles,
    procedure: procedure({
        description: "Format the described task into one well-formed house-format task file with wording kept near-verbatim, then add it to .internal/tasks/.",
        sequence: [
            formatTaskStep(directSmart1, taskInput, taskFiles),
            writeTasksStep(directScribe, taskFiles, result),
        ],
    }),
});
