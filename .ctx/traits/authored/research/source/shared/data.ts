// Shared ports, slots, schemas, and resources for the research family
// (0153, pattern: .ctx/traits/packages/refactor/source/data.ts). The variant
// is the structural guarantee — depth and rigor are properties of which
// variant a run chose, never an input knob — so no quality/depth ports and
// no numeric verdict schema live anywhere in this file.
import { reviewVerdictSchema } from "@ctx-traits/agents";
import type { SchemaHandle } from "@ctx-traits/cdk";
import { port, schema, slot } from "@ctx-traits/cdk";

export const topic = port.input.text({
  id: "topic",
  description: "The research topic or question to investigate.",
});
export const outputDir = port.input.text({
  id: "output-dir",
  optional: true,
  description:
    'Repo-relative root directory research output is written under. Package-defaulted to ".internal/research" via [defaults.port] in runtime.toml; consuming repos override it in their own config.',
});

export const researchStreamSchema: SchemaHandle = schema.object(
  "research-stream",
  {
    id: schema.field(schema.text(), { description: "Stable kebab-case slug for this stream, unique within the plan." }),
    focus: schema.field(schema.text(), {
      description: "The complete, non-overlapping research question this stream owns.",
    }),
    kind: schema.field(schema.enum(["primary", "counterevidence"] as const), {
      description:
        "primary investigates the topic directly; counterevidence deliberately searches for disconfirming evidence and competing conclusions against the other streams' emerging findings.",
    }),
  },
  { description: "One non-overlapping research stream: what it owns and what kind of evidence it is responsible for." },
);
export const streamFindingSchema: SchemaHandle = schema.object(
  "stream-finding",
  {
    "stream-id": schema.field(schema.text(), { description: "The stream.id this finding set was produced by." }),
    summary: schema.field(schema.text(), {
      description: "Cited findings for this stream: claims, sources, and any counterevidence encountered.",
    }),
  },
  { description: "One stream's researched, cited findings." },
);

export const streamPlan = slot({
  id: "stream-plan",
  schema: schema.list(researchStreamSchema),
  description: "The typed list of non-overlapping research streams this run will investigate serially.",
});
export const currentStream = slot({
  id: "current-stream",
  schema: researchStreamSchema,
  description: "The stream the per-item research step is currently working.",
});
export const findings = slot({
  id: "findings",
  schema: schema.list(streamFindingSchema),
  description: "Cited findings accumulated one entry per completed research stream.",
});
export const triangulation = slot.text({
  id: "triangulation",
  description:
    "Cross-stream triangulation: where independent streams agree, where they conflict, source-quality ratings, and remaining gaps.",
});
export const draftReport = slot.text({
  id: "draft-report",
  description: "The research report's current draft content, revised in place after each refinement pass.",
});
export const workSummary = slot.text({
  id: "work-summary",
  description:
    "Worker's cumulative account of the delivered research state: files written, how citations were verified, open concerns.",
});

export const verdict1 = slot({
  id: "review-verdict-1",
  schema: reviewVerdictSchema,
  description: "First reviewer's verdict for the current research state.",
});
export const verdict2 = slot({
  id: "review-verdict-2",
  schema: reviewVerdictSchema,
  description: "Second, independent reviewer's verdict for the current research state.",
});
export const parkReport = slot({
  id: "park-report",
  schema: schema.list(reviewVerdictSchema),
  description:
    "This round's typed park record: empty when every verdict reviewed this round is approved; one entry per reviewed verdict that is revise, each copied unchanged. Written each round by deriveParkReportStep, never model-authored.",
});

export const topicSlug = slot.text({
  id: "topic-slug",
  description:
    "Deterministic kebab-case slug for the topic, derived by a command step — never agent prose — so every deliverable path is predictable.",
});
export const reportPath = slot.text({
  id: "report-path",
  description:
    "Deterministic repo-relative path to this variant's primary deliverable, derived by a command step from port:output-dir and topic-slug.",
});
export const commitOutput = slot.text({
  id: "commit-output",
  description: "Output evidence from the git commit command step: committed hash and subject.",
});

export const commitReport = port.output.text({
  id: "commit-report",
  description:
    "Final commit evidence from the git commit command step. Absent when the clean-tree gate skipped the commit tail entirely.",
  optional: true,
  value: commitOutput,
});
export const parkReportPort = port.output.of(schema.list(reviewVerdictSchema), {
  id: "park-report",
  title: "Park Report",
  description:
    "Typed park record for an unapproved run: one entry per reviewed verdict still revise in the final round. Present only when the build loop exhausted without approval — the run parks and no commit is created.",
  optional: true,
  value: parkReport,
  format: ["structured", "table"],
});
export const researchReportPort = port.output.text({
  id: "research-report",
  title: "Research Report",
  description:
    "Final work summary for the completed research: what was written, how citations were verified, open concerns.",
  optional: true,
  value: workSummary,
  format: "markdown",
});
export const reportPathPort = port.output.text({
  id: "report-path",
  title: "Report Path",
  description: "Repo-relative path to this run's primary deliverable, copied from slot:report-path.",
  optional: true,
  value: reportPath,
});
