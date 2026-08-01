import type { AgentHandle, SlotHandle } from "@ctx-traits/cdk";
import { prompt, sequence } from "@ctx-traits/cdk";

export function critiqueTasksStep(agent: AgentHandle, grounding: SlotHandle, taskFiles: SlotHandle, critique: SlotHandle) {
    return sequence.prompt("critique", {
        title: "Critique the task files (smart-1)",
        agent,
        text: prompt.text`
            Independently critique the grounding notes ${grounding} and the task files ${taskFiles}, as if reviewing someone else's work.
            Check every task for: roughly 10-15 minute sizing (flag anything larger or vaguer); an observable, verifiable Done when; dependency ordering (each task depends only on earlier ones); standalone completeness (the grounding a task needs is folded into its own body); and any rule or validation gate from the grounding that a task omits or contradicts.
            Return the concrete list of defects to fix, each naming the task or grounding section and the specific change needed. If nothing needs fixing, say so explicitly.`,
        output: critique,
        input: [grounding, taskFiles],
    });
}

export function reviseTasksStep(agent: AgentHandle, critique: SlotHandle, taskFiles: SlotHandle) {
    return sequence.prompt("revise", {
        title: "Apply the critique (scribe)",
        agent,
        text: prompt.text`
            Apply the critique ${critique} to the task files ${taskFiles}, in one bounded pass — do not expand scope beyond what the critique names.
            Return the full revised set of task files, replacing the prior set.`,
        output: taskFiles,
        input: [critique, taskFiles],
    });
}
