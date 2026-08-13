import { defineVariant, input, slot, useBehavior, useIntent } from "@ctx-traits/cdk";
import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("Complex", { name: "Plan (Complex)", summary: "Turn a described task into codebase-grounded, house-format task files, then independently critique and revise them once before writing to .internal/tasks/.", metadata: { tag: [...shared.metadata.tag, "review"] }, description: "Turn a described task into codebase-grounded, house-format task files, then independently critique and revise them once before writing to .internal/tasks/." });
  useBehavior(shared.metadata.behavior);
  useIntent(shared.intent);
  const smart1 = shared.agent.smart1("Strong model: refines the task, grounds it in the codebase, splits it into task files, and critiques both artifacts.", "Refinement + grounding + critique role.");
  const scribe = shared.agent.scribe("Applies the critique to the task files, then writes them to .internal/tasks/.", "Revise-and-write role.");
  const critique = slot.text({ id: "critique", description: "Independent critique of the grounding notes and task files: sizing, Done-when verifiability, dependency order, and omitted rules/gates." });
  shared.stage.refine.task(smart1);
  shared.stage.split.tasks(smart1);
  smart1.prompt("Critique the task files (smart-1)", { id: "critique", input: input.prompt`
            Independently critique the grounding notes ${shared.data.grounding} and the task files ${shared.data.taskFiles}, as if reviewing someone else's work.
            Check every task for: roughly 10-15 minute sizing (flag anything larger or vaguer); an observable, verifiable Done when; dependency ordering (each task depends only on earlier ones); standalone completeness (the grounding a task needs is folded into its own body); and any rule or validation gate from the grounding that a task omits or contradicts.
            Return the concrete list of defects to fix, each naming the task or grounding section and the specific change needed. If nothing needs fixing, say so explicitly.`, output: critique });
  scribe.prompt("Apply the critique (scribe)", { id: "revise", input: input.prompt`
            Apply the critique ${critique} to the task files ${shared.data.taskFiles}, in one bounded pass — do not expand scope beyond what the critique names.
            Return the full revised set of task files, replacing the prior set.`, output: shared.data.taskFiles });
  shared.stage.write.tasks(scribe);
  return { writtenFiles: shared.data.writtenFiles };
}
