import { defineVariant, useBehavior, useIntent } from "@ctx-traits/cdk";
import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("Quick", { name: "Plan (Quick)", summary: "Distill a described task directly into house-format task files and add them to .internal/tasks/ — no separate grounding pass, no review pass.", metadata: { tag: shared.metadata.tag }, description: "Distill a described task directly into house-format task files and add them to .internal/tasks/ — no separate grounding pass, no review pass.", procedureDescription: "Distill the described task, grounded in the codebase, straight into house-format task files, then add them to .internal/tasks/." });
  useBehavior(shared.metadata.behavior);
  useIntent(shared.intent);
  const smart1 = shared.agent.smart1("Distills the task directly into house-format task files.", "Distill role.");
  const scribe = shared.agent.scribe("Writes the task files to .internal/tasks/.", "Write role.");
  shared.stage.distill.tasks(smart1);
  shared.stage.write.tasks(scribe);
  return { writtenFiles: shared.data.writtenFiles };
}
