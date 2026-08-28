// Shared ports, slots, and schemas for the walkthrough family. The carrying
// shape is a FLAT node list with parent links (never nested JSON): flat
// validates in one round, deep nesting degrades generation, and the renderer
// rebuilds the tree — the same relations pattern the task board uses.
import { reviewVerdictSchema } from "@ctx-traits/agents";
import type { SchemaHandle } from "@ctx-traits/cdk";
import { port, schema, slot } from "@ctx-traits/cdk";

export const topic = port.input.text({
  id: "topic",
  description:
    "The codebase topic to walk through: a file, a module, a subsystem, or a question (e.g. \"the task provider\", \"modules/io/src/dispatch_preflight.rs\", \"how does dispatch refuse a claimed task\").",
});
export const outputDir = port.input.text({
  id: "output-dir",
  optional: true,
  description:
    'Repo-relative directory the walkthrough HTML is written under. Package-defaulted to ".hidden/walkthroughs" via [defaults.port] in trait.toml; consuming repos override it in their own config.',
});

export const codeRefSchema: SchemaHandle = schema.object(
  "code-ref",
  {
    path: schema.field(schema.text(), {
      description: "Repo-relative file path this node's explanation is grounded in. Must exist in the repository.",
    }),
    lines: schema.field(schema.text(), {
      description:
        'Line span "start-end" (1-indexed, inclusive) inside path that this node covers. Verified by opening the file — never guessed. The treemap derives tile sizes from these spans, so they must honestly cover the code the node explains.',
    }),
  },
  { description: "One code anchor grounding a node: a file and the line span the node's prose actually explains." },
);

export const nodeSchema: SchemaHandle = schema.object(
  "walkthrough-node",
  {
    id: schema.field(schema.text(), {
      description: "Stable kebab-case slug, unique across the whole node list.",
    }),
    parent: schema.optional(
      schema.field(schema.text(), {
        description:
          "The id of this node's parent. Absent on exactly one node — the root overview. Every other node names an existing id; the tree is at most three levels deep below the root.",
      }),
    ),
    title: schema.field(schema.text(), { description: "Short human title rendered on the treemap tile." }),
    kind: schema.field(schema.enum(["overview", "subsystem", "component", "symbol", "flow"] as const), {
      description:
        "overview = the single root; subsystem = a major area; component = a file or cohesive unit; symbol = a function/type/impl; flow = a cross-cutting path through the code (a request, a lifecycle).",
    }),
    summary: schema.field(schema.text(), {
      description: "One or two sentences shown at a glance — what this node is and why it exists.",
    }),
    explanation: schema.field(schema.text(), {
      description:
        "The thorough narration for this node's layer, in the register of its depth: the root explains architecture and intent, mid nodes explain responsibilities and collaborations, leaves explain the actual mechanics precisely. Plain prose paragraphs separated by blank lines; inline `code` backticks allowed.",
    }),
    refs: schema.field(schema.list(codeRefSchema), {
      description:
        "At least one code anchor per node. Children's spans should live inside the territory their parent's spans cover.",
    }),
  },
  {
    description:
      "One node of the walkthrough tree, flat with a parent link. Least specific at the root, most specific at the leaves.",
  },
);

export const walkthroughNodes = slot({
  id: "walkthrough-nodes",
  schema: schema.list(nodeSchema),
  description:
    "The complete flat node list for the walkthrough: exactly one root (no parent), every other node linked to an existing parent, every node grounded in verified code refs.",
});
export const topicSlug = slot.text({
  id: "topic-slug",
  description:
    "Deterministic kebab-case slug for the topic, derived by a command step — never agent prose — so the output path is predictable from the topic alone.",
});
export const htmlPath = slot.text({
  id: "html-path",
  description: "Deterministic repo-relative path of the generated walkthrough HTML, derived by a command step.",
});
export const renderLog = slot.text({
  id: "render-log",
  description: "Output evidence from the render command step: the written path and node/byte counts.",
});
export const workSummary = slot.text({
  id: "work-summary",
  description:
    "Investigator's cumulative account: what was explored, how refs were verified, coverage decisions, open concerns.",
});
export const verdict1 = slot({
  id: "review-verdict-1",
  schema: reviewVerdictSchema,
  description: "Reviewer's verdict for the current walkthrough node list.",
});

export const walkthroughPathPort = port.output.text({
  id: "walkthrough-path",
  title: "Walkthrough Path",
  description: "Repo-relative path of the generated interactive walkthrough HTML.",
  optional: true,
  value: htmlPath,
});
export const walkthroughSummaryPort = port.output.text({
  id: "walkthrough-summary",
  title: "Walkthrough Summary",
  description: "Final investigator summary: coverage, verification method, open concerns.",
  optional: true,
  value: workSummary,
});
export const renderReportPort = port.output.text({
  id: "render-report",
  title: "Render Report",
  description: "Render step evidence: the written path and counts.",
  optional: true,
  value: renderLog,
});
