import { Method, procedure, prompt, sequence, slot, Tone, variant, Verbosity } from "@ctx-traits/cdk";
import { planningAgents } from "../agent.ts";
import { planData } from "../data.ts";
import { refineTaskStep, splitPlanStep } from "../sequence/planning.ts";
import { writeContractAndPlanStep } from "../sequence/writing.ts";

const { smart1, scribe } = planningAgents(
    "Strong model: refines the task, grounds it in the codebase, derives the product contract, splits it into phases, and critiques both artifacts.", "Refinement + grounding + critique role.",
    "Applies the critique to the contract and plan, then writes both to .docs/PRODUCT.md and .plans/EXECUTION_PLAN.md.", "Revise-and-write role.",
);
const { taskInput, productContext, executionPlan, result, writtenFiles } = planData(true);
if (!productContext) throw new Error("complex plan requires product context");
const critique = slot.text({ id: "critique", description: "Independent critique of the product contract and execution plan: sizing, DoD verifiability, dependency order, and omitted rules/gates." });

export default variant({
    name: "Plan (Complex)",
    summary: "Turn a described task into a codebase-grounded product contract and a phased execution plan, then independently critique and revise both once before writing .docs/PRODUCT.md and .plans/EXECUTION_PLAN.md.",
    metadata: { tag: ["task", "plan", "bootstrap", "planning", "review"] },
    behavior: { tone: [Tone.direct, Tone.technical], method: Method.evidenceFirst, verbosity: Verbosity.brief },
    intent: { focus: ["specific", "correctness"], avoid: ["speculative-claim", "scope-creep"] },
    procedure: procedure({
        description: "Refine the described task against the codebase into a product contract, split it into small phases, independently critique both artifacts, apply one bounded revise pass, and write both .docs/PRODUCT.md and .plans/EXECUTION_PLAN.md so implement-phase can run.",
        input: taskInput, output: writtenFiles,
        sequence: [
            refineTaskStep(smart1, taskInput, productContext),
            splitPlanStep(smart1, taskInput, productContext, executionPlan),
            sequence.prompt("critique", {
                title: "Critique the contract and plan (smart-1)", agent: smart1,
                text: prompt.text`
                    Independently critique the product contract ${productContext} and the execution plan ${executionPlan}, as if reviewing someone else's work.
                    Check every phase for: roughly 10-15 minute sizing (flag anything larger or vaguer); an observable, verifiable Definition of Done; dependency ordering (each phase depends only on earlier ones); and any house rule or validation gate from the contract that a phase omits or contradicts.
                    Return the concrete list of defects to fix, each naming the phase or contract section and the specific change needed. If nothing needs fixing, say so explicitly.`,
                output: critique, input: [productContext, executionPlan],
            }),
            sequence.prompt("revise", {
                title: "Apply the critique (scribe)", agent: scribe,
                text: prompt.text`
                    Apply the critique ${critique} to the product contract ${productContext} and the execution plan ${executionPlan}, in one bounded pass — do not expand scope beyond what the critique names.
                    Return the full revised product contract and the full revised execution plan, each replacing the artifact it corresponds to.`,
                output: [productContext, executionPlan], input: [critique, productContext, executionPlan],
            }),
            writeContractAndPlanStep(scribe, productContext, executionPlan, result),
        ],
    }),
});
