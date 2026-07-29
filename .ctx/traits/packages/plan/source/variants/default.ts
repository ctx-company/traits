import { Method, procedure, Tone, variant, Verbosity } from "@ctx-traits/cdk";
import { planningAgents } from "../agent.ts";
import { planData } from "../data.ts";
import { refineTaskStep, splitPlanStep } from "../sequence/planning.ts";
import { writeContractAndPlanStep } from "../sequence/writing.ts";

const { smart1, scribe } = planningAgents(
    "Strong model: refines the task, grounds it in the codebase, derives the product contract, and splits it into phases.", "Refinement + grounding role.",
    "Writes the product contract to .docs/PRODUCT.md and the phased plan to .plans/EXECUTION_PLAN.md.", "Bootstrap-writer role.",
);
const { taskInput, productContext, executionPlan, result, writtenFiles } = planData(true);
if (!productContext) throw new Error("default plan requires product context");

export default variant({
    name: "Plan",
    summary: "Turn a described task into a codebase-grounded product contract (.docs/PRODUCT.md) and a phased execution plan (.plans/EXECUTION_PLAN.md) — the two files implement-phase needs to run in any repo.",
    metadata: { tag: ["task", "plan", "bootstrap", "planning"] },
    behavior: { tone: [Tone.direct, Tone.technical], method: Method.evidenceFirst, verbosity: Verbosity.brief },
    intent: { focus: ["specific", "correctness"], avoid: ["speculative-claim", "scope-creep"] },
    procedure: procedure({
        description: "Refine the described task against the codebase into a product contract, split it into small phases, and write both .docs/PRODUCT.md and .plans/EXECUTION_PLAN.md so implement-phase can run.",
        input: taskInput, output: writtenFiles,
        sequence: [
            refineTaskStep(smart1, taskInput, productContext),
            splitPlanStep(smart1, taskInput, productContext, executionPlan),
            writeContractAndPlanStep(scribe, productContext, executionPlan, result),
        ],
    }),
});
