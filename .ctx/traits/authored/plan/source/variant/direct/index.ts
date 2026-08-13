import { defineVariant, input, useBehavior, useIntent } from "@ctx-traits/cdk";
import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("Direct", { name: "Plan (Direct)", summary: "Format a described task into one well-formed house-format task file with wording kept near-verbatim, then add it to .internal/tasks/ — no refinement, no invention, the fastest path to just implement.", metadata: { tag: shared.metadata.tag }, description: "Format a described task into one well-formed house-format task file with wording kept near-verbatim, then add it to .internal/tasks/ — no refinement, no invention, the fastest path to just implement." });
  useBehavior(shared.metadata.behavior);
  useIntent(shared.intent);
  const smart1 = shared.agent.smart1("Formats the task text into one well-formed task file, near-verbatim.", "Format role.");
  const scribe = shared.agent.scribe("Writes the task file to .internal/tasks/.", "Write role.");
  smart1.prompt("Format into a task file (smart-1)", { id: "format", input: input.prompt(`Format the task below into exactly one well-formed task file for .internal/tasks/.
            Task as described: {task}
            Keep the task's own wording near-verbatim. Do NOT refine, invent, or add requirements the task does not state; do not read the codebase or ground it in anything beyond the task text itself.
            ${shared.resource.TASK_FORMAT_DOCTRINE}
            Do not implement anything.`, { task: shared.data.taskInput }), output: shared.data.taskFiles });
  shared.stage.write.tasks(scribe);
  return { writtenFiles: shared.data.writtenFiles };
}
