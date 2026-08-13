import { defineVariant, useBehavior, useIntent } from "@ctx-traits/cdk";
import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("Default", { name: "Plan", summary: "Turn a described task into codebase-grounded, house-format task files in .internal/tasks/ — the board the implement family runs from in any repo.", metadata: { tag: shared.metadata.tag }, description: "Turn a described task into codebase-grounded, house-format task files in .internal/tasks/ — the board the implement family runs from in any repo.", procedureDescription: "Refine the described task against the codebase, split it into small task files, and write them to .internal/tasks/ so the implement family can run." });
  useBehavior(shared.metadata.behavior);
  useIntent(shared.intent);
  const smart1 = shared.agent.smart1("Strong model: refines the task, grounds it in the codebase, and splits it into task files.", "Refinement + grounding role.");
  const scribe = shared.agent.scribe("Writes the task files to .internal/tasks/.", "Bootstrap-writer role.");
  shared.stage.refine.task(smart1);
  shared.stage.split.tasks(smart1);
  shared.stage.write.tasks(scribe);
  return { writtenFiles: shared.data.writtenFiles };
}
