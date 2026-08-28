// Shared ports, slots, and schemas for the walkthrough family. The carrying
// shapes: a FLAT node list with parent links (never nested JSON — flat
// validates in one round and the renderer rebuilds the tree), and EXHAUSTIVE
// coverage as arithmetic, not judgment: a deterministic script enumerates
// every symbol in the topic's file closure, every enumerated key must appear
// on exactly one described node, and the loop cannot exit while the set
// difference is non-empty. Revisions are append-only: a later batch node
// with an already-seen id REPLACES the earlier one (last-wins), so nothing
// ever rewrites the whole tree.
import { reviewVerdictSchema } from "@ctx-traits/agents";
import type { SchemaHandle } from "@ctx-traits/cdk";
import { port, schema, slot } from "@ctx-traits/cdk";

export const topic = port.input.text({
  id: "topic",
  description:
    "The codebase topic to walk through: a file, a module, a subsystem, or a question (e.g. \"center\", \"modules/io/src/dispatch_preflight.rs\", \"how does dispatch refuse a claimed task\").",
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
      description:
        'Stable kebab-case slug, unique across the whole walkthrough. Conventions: the root is "root"; a file node is "f-" plus the slugified path; a symbol node is "s-" plus the symbol name plus "-" plus its definition line. Re-emitting an existing id REPLACES that node (last-wins revision).',
    }),
    parent: schema.optional(
      schema.field(schema.text(), {
        description:
          "The id of this node's parent. Absent on exactly one node — the root overview. Every other node names an existing id.",
      }),
    ),
    title: schema.field(schema.text(), { description: "Short human title rendered on the treemap tile." }),
    kind: schema.field(schema.enum(["overview", "area", "file", "type", "function", "flow"] as const), {
      description:
        "overview = the single root; area = a module/subsystem grouping; file = one source file from the closure; type = a struct/enum/trait/interface/class/type-alias; function = a fn/method/macro; flow = a cross-cutting path through the code.",
    }),
    symbol: schema.optional(
      schema.field(schema.text(), {
        description:
          "REQUIRED on every type/function node, absent elsewhere: the coverage key of the symbol-index entry this node describes, copied VERBATIM (format path:line:name). This is how deterministic coverage joins descriptions to enumerated code.",
      }),
    ),
    summary: schema.field(schema.text(), {
      description: "One or two sentences shown at a glance — what this node is and why it exists.",
    }),
    explanation: schema.field(schema.text(), {
      description:
        "The thorough narration for this node's layer, in the register of its depth: the root explains architecture and intent, areas explain responsibilities and collaborations, files explain what lives there and why, type/function leaves explain the actual mechanics precisely — what it does, what it refuses, the edges it guards. Plain prose paragraphs separated by blank lines; inline `code` backticks allowed.",
    }),
    refs: schema.field(schema.list(codeRefSchema), {
      description:
        "At least one code anchor per node. A symbol node's span covers that symbol's definition; a file node's span covers the file; children's spans live inside the territory their parent's spans cover.",
    }),
  },
  {
    description:
      "One node of the walkthrough tree, flat with a parent link. Least specific at the root, ending at actual code symbols at the leaves.",
  },
);

export const closureEntrySchema: SchemaHandle = schema.object(
  "closure-entry",
  {
    path: schema.field(schema.text(), {
      description: "Repo-relative path of one source file in the topic's dependency closure. Must exist.",
    }),
    role: schema.field(schema.text(), {
      description: "One sentence: why this file is in the closure (owns the topic / direct dependency via which import).",
    }),
  },
  { description: "One file of the topic's dependency closure, with the reason it belongs." },
);

export const fileClosure = slot({
  id: "file-closure",
  schema: schema.list(closureEntrySchema),
  description:
    "The topic's complete file closure: every file the topic's code lives in plus every same-workspace file it depends on, traversed until the declared horizon. The symbol index — and therefore the coverage gate — is computed from exactly this list.",
});
export const horizon = slot.text({
  id: "horizon",
  description:
    "Where traversal deliberately stopped and why: std/third-party crates, unrelated subsystems, generated code. Honesty about the boundary is part of the deliverable and is rendered into the walkthrough root.",
});
export const skeletonNodes = slot({
  id: "skeleton-nodes",
  schema: schema.list(nodeSchema),
  description:
    "The upper tree from the survey: the root overview, area nodes, one file node per closure entry (id \"f-\" + slugified path), and any flow nodes. Never contains type/function nodes — those are produced per-file.",
});
export const nodeBatchSchema: SchemaHandle = schema.object(
  "node-batch",
  {
    nodes: schema.field(schema.list(nodeSchema), {
      description: "The described nodes this frame contributes. Empty when the frame has nothing to add.",
    }),
  },
  { description: "One appended batch of described nodes — a per-file frame's output, or a repair/review-fix round's." },
);

export const nodeBatches = slot({
  id: "node-batches",
  schema: schema.list(nodeBatchSchema),
  description:
    "Append-only batches of described nodes: one batch per for-each file frame, plus one batch per repair or review-fix round. Consumers flatten the batches' nodes in order and dedupe by id, last occurrence winning.",
});
export const symbolIndex = slot.text({
  id: "symbol-index",
  description:
    "Deterministic JSON symbol index over the file closure (from symbols.py enumerate): every fn/struct/enum/trait/mod/type/class/interface definition with its coverage key path:line:name. Ground truth for the coverage gate — never edited by a model.",
});
export const coverageReport = slot.text({
  id: "coverage-report",
  description:
    "Deterministic JSON coverage report (from symbols.py coverage): uncovered index entries, duplicate ids, orphaned parents, counts. The repair prompt consumes it verbatim.",
});
export const coverageStatus = slot.text({
  id: "coverage-status",
  description:
    'Deterministic coverage verdict (from symbols.py status): exactly "complete" when every index key is described and links are sound, else "incomplete:<n>".',
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
    "Investigator's cumulative account: what was explored, how the closure and horizon were chosen, how refs were verified, open concerns.",
});
export const verdict1 = slot({
  id: "review-verdict-1",
  schema: reviewVerdictSchema,
  description: "Reviewer's verdict for the current walkthrough state.",
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
  description: "Final investigator summary: closure, horizon, verification method, open concerns.",
  optional: true,
  value: workSummary,
});
export const coveragePort = port.output.text({
  id: "coverage",
  title: "Coverage",
  description: "Deterministic coverage verdict at the end of the run.",
  optional: true,
  value: coverageStatus,
});
export const renderReportPort = port.output.text({
  id: "render-report",
  title: "Render Report",
  description: "Render step evidence: the written path and counts.",
  optional: true,
  value: renderLog,
});
