// Shared ports, slots, and schemas for the plan family. The deliverable is
// TaskDocument TOML on the board (.internal/tasks/) — the format `[tasks]`
// dispatch, `port:task` binding, and the dependency preflights actually
// resolve (a `.md` task file is invisible to all three). Content travels in
// typed slots, never a notes directory (owner ruling 2026-08-17: plan is
// slot-only); oversized values ride the `[drive] inline-prompt-bytes` spill.
import { reviewVerdictSchema } from "@ctx-traits/agents";
import type { SchemaHandle } from "@ctx-traits/cdk";
import { port, schema, slot } from "@ctx-traits/cdk";

export const taskInput = port.input.text({
  id: "task",
  description:
    "The work you describe, in your own words — rough is fine. May reference a source document (a research report, an MVP plan) by repo-relative path.",
});

export const durationTarget = port.input.text({
  id: "duration",
  title: "Task Duration Target",
  optional: true,
  default: { value: "10-15 minutes" },
  description:
    'Target duration of one child task, e.g. "10-15 minutes" or "~45m". Slicing and review both judge tasks against this.',
});

export const workItemSchema: SchemaHandle = schema.object(
  "work-item",
  {
    id: schema.field(schema.text(), {
      description: "Stable kebab-case slug for this work item, unique within the plan.",
    }),
    title: schema.field(schema.text(), {
      description: "The work item's short name, in the source's own words where a source document exists.",
    }),
    requirement: schema.field(schema.text(), {
      description: "What the source demands this item deliver or establish.",
    }),
  },
  {
    description:
      "One unit of demanded work, extracted from the source document (or distilled from the described task when no document is referenced). Coverage is judged against these.",
  },
);
export const planTaskSchema: SchemaHandle = schema.object(
  "plan-task",
  {
    key: schema.field(schema.text(), {
      description: 'Symbolic key: the parent slice\'s KEY<n> plus a child ordinal, e.g. "KEY2.3" or "KEY2.11" — ordinals continue past 9, never zero-padded. Real board numbers are assigned by the final renumber step, never here.',
    }),
    title: schema.field(schema.text(), { description: "Short imperative title." }),
    "depends-on": schema.field(schema.list(schema.text()), {
      description: "Keys of earlier tasks this one needs completed first; empty when independent.",
    }),
    summary: schema.field(schema.text(), {
      description: "One-paragraph account of the task's work, sized to the run's duration target (default: roughly 10-15 minutes) of focused agent effort.",
    }),
  },
  {
    description:
      "One child task in a slice's plan — the index entry; the full TaskDocument body is composed by the slice's own frame.",
  },
);
export const planSliceSchema: SchemaHandle = schema.object(
  "plan-slice",
  {
    key: schema.field(schema.text(), {
      description: 'Symbolic key of the slice: "KEY1", "KEY2", ... in plan order. The final renumber step assigns the real board number.',
    }),
    title: schema.field(schema.text(), { description: "The slice's goal, as a short title." }),
    covers: schema.field(schema.list(schema.text()), {
      description:
        "work-item ids this slice is responsible for. Every extracted work item must appear in at least one slice's covers.",
    }),
    tasks: schema.field(schema.list(planTaskSchema), {
      description:
        'The slice\'s child tasks in dependency order, keys "KEY<n>.1", "KEY<n>.2", ... continuing ".10", ".11" beyond nine — EMPTY when the slice is one standalone bare task rather than a charter.',
    }),
  },
  {
    description:
      "One dependency-ordered slice of the plan: a standalone bare task (empty tasks), or a parent charter key plus its child tasks when a genuine umbrella exists.",
  },
);
export const writeReceiptSchema: SchemaHandle = schema.object(
  "write-receipt",
  {
    "slice-key": schema.field(schema.text(), { description: "The slice charter's key." }),
    files: schema.field(schema.list(schema.text()), {
      description: "Repo-relative paths of every task file this slice's frame wrote.",
    }),
  },
  { description: "One slice frame's account of the task files it wrote to the board." },
);

export const workItems = slot({
  id: "work-items",
  schema: schema.list(workItemSchema),
  description: "The source's own units of demanded work — coverage is judged against these, not the slice plan.",
});
export const doneCriteria = slot({
  id: "done-criteria",
  schema: schema.list(schema.text()),
  description: "The source's explicit definition-of-done items, empty when it states none.",
});
export const boardSnapshot = slot.text({
  id: "board-snapshot",
  description:
    "Deterministic checksum of the board listing at run start — the reference the pre-write guard compares against, so a frame that writes task files before the write phase fails the run instead of merging out-of-plan files.",
});
// The runtime's own `check`-step contract (P565): a check step's output
// slot must declare `ok` (schema:boolean) plus `argv` (schema:text list).
const boardCheckSchema = schema.object("board-untouched", {
  ok: schema.field(schema.boolean(), {
    description: "True when the board listing still matches the run-start snapshot at the write boundary.",
  }),
  argv: schema.field(schema.list(schema.text()), { description: "The exact argv that decided it." }),
});
export const boardCheck = slot({
  id: "board-check",
  schema: boardCheckSchema,
  description: "The pre-write board guard's verdict: detection that no earlier step wrote to the board.",
});
export const keyMap = slot.text({
  id: "key-map",
  description:
    'The renumber step\'s mapping, one "KEY<n> -> NNNN" line per assigned key — derived mechanically from the live board at the end of the run, never agent arithmetic.',
});
export const raisedDate = slot.text({
  id: "raised-date",
  description: "Today's date (YYYY-MM-DD), derived by a command step — the Raised stamp every written task carries.",
});
export const slicePlan = slot({
  id: "slice-plan",
  schema: schema.list(planSliceSchema),
  description:
    "The typed plan: symbolic-keyed slices — bare tasks or charters with children; real board numbers are assigned by the final renumber step.",
});
export const receipts = slot({
  id: "receipts",
  schema: schema.list(writeReceiptSchema),
  description: "One write receipt appended per slice frame (or produced whole by a single-frame variant).",
});
export const grounding = slot.text({
  id: "grounding",
  description:
    "Codebase grounding for the described work: the affected modules or areas, applicable invariants, rules, and constraints, and the repo's existing validation gates (exact commands) the task files must honor.",
  hint: "Grounded scope-level context: affected modules or areas, existing build/test/lint gates (exact invocations), architectural invariants, dependency rules, and constraints an implementer and reviewer must honor. Context the tasks carry, not a step-by-step plan; defer exact files, symbols, signatures, edits, and edit sequencing to architect.",
});

export const verdict = slot({
  id: "review-verdict",
  schema: reviewVerdictSchema,
  description: "Independent reviewer's verdict on the written board slice(s).",
});
export const revisionLog = slot.text({
  id: "revision-log",
  description: "The revise pass's account of the task files it changed or added, one path per line.",
});
export const writtenFiles = port.output.of("written-files", schema.list(writeReceiptSchema), {
  title: "Written Task Files",
  description:
    "Per-slice receipts naming every task file written under .internal/tasks/ — symbolic paths as written; the key map translates them to final board keys.",
  value: receipts,
});
export const finalKeys = port.output.of("final-keys", schema.text(), {
  title: "Final Board Keys",
  description: 'The renumber step\'s "KEY<n> -> NNNN" mapping — the real board keys the written files ended up under.',
  value: keyMap,
});
