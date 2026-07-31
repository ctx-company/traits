import { Method, procedure, prompt, sequence, slot, Tone, variant, Verbosity } from "@ctx-traits/cdk";
import { planningAgents } from "../agent.ts";
import { planData } from "../data.ts";
import { refineTaskStep, splitTasksStep } from "../sequence/planning.ts";
import { writeTasksStep } from "../sequence/writing.ts";

const { smart1, scribe } = planningAgents(
    "Strong model: refines the task, grounds it in the codebase, splits it into task files, and critiques both artifacts.", "Refinement + grounding + critique role.",
    "Applies the critique to the task files, then writes them to .internal/tasks/.", "Revise-and-write role.",
);
const { taskInput, grounding, taskFiles, result, writtenFiles } = planData(true);
if (!grounding) throw new Error("complex plan requires grounding");
const critique = slot.text({ id: "critique", description: "Independent critique of the grounding notes and task files: sizing, Done-when verifiability, dependency order, and omitted rules/gates." });

export default variant({
    name: "Plan (Complex)",
    summary: "Turn a described task into codebase-grounded, house-format task files, then independently critique and revise them once before writing to .internal/tasks/.",
    metadata: { tag: ["task", "plan", "bootstrap", "planning", "review"] },
    behavior: { tone: [Tone.direct, Tone.technical], method: Method.evidenceFirst, verbosity: Verbosity.brief },
    intent: { focus: ["specific", "correctness"], avoid: ["speculative-claim", "scope-creep"] },
    procedure: procedure({
        description: "Refine the described task against the codebase, split it into small task files, independently critique them, apply one bounded revise pass, and write them to .internal/tasks/ so the implement family can run.",
        input: taskInput, output: writtenFiles,
        sequence: [
            refineTaskStep(smart1, taskInput, grounding),
            splitTasksStep(smart1, taskInput, grounding, taskFiles),
            sequence.prompt("critique", {
                title: "Critique the task files (smart-1)", agent: smart1,
                text: prompt.text`
                    Independently critique the grounding notes ${grounding} and the task files ${taskFiles}, as if reviewing someone else's work.
                    Check every task for: roughly 10-15 minute sizing (flag anything larger or vaguer); an observable, verifiable Done when; dependency ordering (each task depends only on earlier ones); standalone completeness (the grounding a task needs is folded into its own body); and any rule or validation gate from the grounding that a task omits or contradicts.
                    Return the concrete list of defects to fix, each naming the task or grounding section and the specific change needed. If nothing needs fixing, say so explicitly.`,
                output: critique, input: [grounding, taskFiles],
            }),
            sequence.prompt("revise", {
                title: "Apply the critique (scribe)", agent: scribe,
                text: prompt.text`
                    Apply the critique ${critique} to the task files ${taskFiles}, in one bounded pass — do not expand scope beyond what the critique names.
                    Return the full revised set of task files, replacing the prior set.`,
                output: taskFiles, input: [critique, taskFiles],
            }),
            writeTasksStep(scribe, taskFiles, result),
        ],
    }),
});
