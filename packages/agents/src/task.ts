import type { SlotHandle, SlotWithFields } from "@ctx-traits/cdk";
import { schema, slot } from "@ctx-traits/cdk";

/**
 * One ordered work item inside a task, mirroring
 * `modules/core/src/task/mod.rs::Step`'s kebab-case JSON shape exactly —
 * `ctx tasks show --json` is this schema's only producer.
 */
export const stepSchema = schema.object("task-step", {
  id: schema.text(),
  title: schema.text(),
  done: schema.boolean(),
  content: schema.text(),
});

/**
 * One subtask edge inlined one level deep: identity plus its own content,
 * never its own steps or subtasks (0062's honest cut — `ResolvedRelations`
 * carries edges, not documents, so nesting stops here).
 */
export const subtaskNodeSchema = schema.object("task-subtask-node", {
  key: schema.text(),
  title: schema.text(),
  content: schema.text(),
});

/**
 * The task value the kit's run-facing provider surface emits, mirroring
 * `modules/core/src/task/provider.rs::ResolvedTask` plus its document:
 * `key`/`title`/`content` at the root, `steps` as authored, `open-steps`
 * derived (steps not yet `done`), and `subtasks` inlined one level. Every
 * node — root, step, subtask — exposes the same `content` accessor (the
 * "uniform node" contract 0062 is named for).
 */
export const taskSchema = schema.object("task", {
  key: schema.text(),
  title: schema.text(),
  content: schema.text(),
  steps: schema.field(schema.list(stepSchema)),
  "open-steps": schema.field(schema.list(stepSchema)),
  subtasks: schema.field(schema.list(subtaskNodeSchema)),
});

/**
 * The shared typed task slot every family/trait extracts into — replaces
 * the retired text `slot:task-brief`. Declared once so `task.content`,
 * `task.openSteps`/`task["open-steps"]`, and `task.subtasks` all resolve
 * through the same `FieldRef`s everywhere it's imported.
 * @example `const taskSlot = declareTaskSlot(); feasibilityStep(agentHandle, taskSlot);`
 */
export function declareTaskSlot(id = "task"): SlotHandle<TaskValue> & SlotWithFields<TaskValue> {
  return slot({ id, schema: taskSchema, description: "The dispatched task, extracted deterministically." });
}

/** The kit's task value shape — see {@link taskSchema}. */
export type TaskValue = {
  readonly key: string;
  readonly title: string;
  readonly content: string;
  readonly steps: readonly StepValue[];
  readonly "open-steps": readonly StepValue[];
  readonly subtasks: readonly SubtaskNodeValue[];
};

/** One step's value — see {@link stepSchema}. */
export type StepValue = {
  readonly id: string;
  readonly title: string;
  readonly done: boolean;
  readonly content: string;
};

/** One inlined subtask node's value — see {@link subtaskNodeSchema}. */
export type SubtaskNodeValue = {
  readonly key: string;
  readonly title: string;
  readonly content: string;
};
