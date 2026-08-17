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

export const briefTrackSchema: SchemaHandle = schema.object(
  "brief-track",
  {
    id: schema.field(schema.text(), { description: "Stable kebab-case slug for this track, unique within the brief." }),
    title: schema.field(schema.text(), { description: "The track's short name, as the brief gives it." }),
    question: schema.field(schema.text(), {
      description: "What the brief wants answered or established for this track.",
    }),
  },
  {
    description:
      "One research track the brief itself enumerates, or one distilled from its prose when it does not enumerate any.",
  },
);
export const deliverableSectionSchema: SchemaHandle = schema.object(
  "deliverable-section",
  {
    id: schema.field(schema.text(), { description: "Stable kebab-case slug for this section." }),
    title: schema.field(schema.text(), { description: "The section heading, as the brief demands it." }),
    expectation: schema.field(schema.text(), {
      description: "What the brief expects this section to contain or establish.",
    }),
  },
  {
    description:
      "One required top-level section of the final report, in the brief's own order — the report's structure is the brief's to dictate, never the run's.",
  },
);
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
    covers: schema.field(schema.list(schema.text()), {
      description:
        "The brief-track ids this stream is responsible for covering. Every planned track must appear in at least one stream's covers; a counterevidence stream lists the tracks whose emerging conclusions it challenges.",
    }),
  },
  { description: "One non-overlapping research stream: what it owns and what kind of evidence it is responsible for." },
);
export const streamDossierSchema: SchemaHandle = schema.object(
  "stream-dossier",
  {
    "stream-id": schema.field(schema.text(), { description: "The stream.id this dossier was produced by." }),
    "notes-path": schema.field(schema.text(), {
      description:
        "Repo-relative path of this stream's full reading-notes file — the evidence corpus the dossier indexes.",
    }),
    summary: schema.field(schema.text(), {
      description: "Cited digest of the stream's findings: what was established, contested, and left open.",
    }),
    "key-claims": schema.field(schema.list(schema.text()), {
      description: "The stream's load-bearing claims, each with its source-quality rating and citation inline.",
    }),
    sources: schema.field(schema.list(schema.text()), {
      description: 'One entry per consulted source: "<A-E> | <citation or url> | <what it supports>".',
    }),
    "open-questions": schema.field(schema.list(schema.text()), {
      description: "What this stream could not settle, each with why (source gap, conflict, access failure).",
    }),
  },
  {
    description:
      "One stream's evidence dossier: the typed index over its reading-notes file, not a replacement for it.",
  },
);

export const briefTracks = slot({
  id: "brief-tracks",
  schema: schema.list(briefTrackSchema),
  description: "The brief's own research tracks, extracted at ingest — coverage is judged against these, not the plan.",
});
export const deliverableSections = slot({
  id: "deliverable-sections",
  schema: schema.list(deliverableSectionSchema),
  description:
    "The deliverable structure the brief demands for report.md, one entry per required top-level section in the brief's order.",
});
export const doneCriteria = slot.texts({
  id: "done-criteria",
  description: "The brief's explicit definition-of-done items, empty when it states none.",
});
export const streamPlan = slot({
  id: "stream-plan",
  schema: schema.list(researchStreamSchema),
  description: "The typed list of non-overlapping research streams this run will investigate.",
});
export const dossiers = slot({
  id: "dossiers",
  schema: schema.list(streamDossierSchema),
  description: "One evidence dossier appended per completed stream by the per-stream research loop.",
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
