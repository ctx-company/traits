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
      description: 'Final board key: the parent slice\'s NNNN plus a child ordinal, e.g. "0150.2".',
    }),
    title: schema.field(schema.text(), { description: "Short imperative title." }),
    "depends-on": schema.field(schema.list(schema.text()), {
      description: "Keys of earlier tasks this one needs completed first; empty when independent.",
    }),
    summary: schema.field(schema.text(), {
      description: "One-paragraph account of the task's work, sized to roughly 10-15 minutes of focused agent effort.",
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
      description: "Final board key of the slice's parent charter task: bare zero-padded NNNN.",
    }),
    title: schema.field(schema.text(), { description: "The slice's goal, as a short title." }),
    covers: schema.field(schema.list(schema.text()), {
      description:
        "work-item ids this slice is responsible for. Every extracted work item must appear in at least one slice's covers.",
    }),
    tasks: schema.field(schema.list(planTaskSchema), {
      description: 'The slice\'s child tasks in dependency order, keys "NNNN.1", "NNNN.2", ...',
    }),
  },
  { description: "One dependency-ordered slice of the plan: a parent charter key plus its child tasks." },
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
export const nextKey = slot.text({
  id: "next-key",
  description:
    "Deterministic zero-padded next free board key, derived by a command step scanning .internal/tasks/ (archived/ included) — never agent arithmetic against the directory.",
});
export const raisedDate = slot.text({
  id: "raised-date",
  description: "Today's date (YYYY-MM-DD), derived by a command step — the Raised stamp every written task carries.",
});
export const slicePlan = slot({
  id: "slice-plan",
  schema: schema.list(planSliceSchema),
  description:
    "The typed plan: parent charter keys with their child tasks, final keys already assigned from slot:next-key.",
});
export const receipts = slot({
  id: "receipts",
  schema: schema.list(writeReceiptSchema),
  description: "One write receipt appended per slice frame (or produced whole by a single-frame variant).",
});
export const grounding = slot.text({
  id: "grounding",
  description:
    "Codebase grounding for the described work: the concrete files and modules it touches, the repo's validation gates (exact commands), and the invariants, rules, and constraints the task files must honor.",
  hint: "Grounded in the codebase: what the project is, its build/test/lint gates (exact invocations), architectural invariants, dependency rules, and constraints an implementer and reviewer must honor. Context the tasks carry, not the step-by-step plan.",
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
export const parkReport = slot({
  id: "park-report",
  schema: schema.list(reviewVerdictSchema),
  description:
    "Typed park record: empty when the reviewed verdict is approved; the revise verdict copied unchanged otherwise. Written each round by deriveParkReportStep, never model-authored.",
});

export const writtenFiles = port.output.of(schema.list(writeReceiptSchema), {
  id: "written-files",
  title: "Written Task Files",
  description: "Per-slice receipts naming every task file written under .internal/tasks/.",
  value: receipts,
  format: ["structured", "table"],
});
export const parkReportPort = port.output.of(schema.list(reviewVerdictSchema), {
  id: "park-report",
  title: "Park Report",
  description:
    "Present only when the review loop exhausted without approval — the run parks with the final revise verdict.",
  optional: true,
  value: parkReport,
  format: ["structured", "table"],
});
