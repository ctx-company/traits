import { agent, dependency, input, output, port, procedure, ref, schema, sequence, trait } from "@ctx-traits/cdk";

const critic = agent("critic", {
  description: "Reports source-backed advisory trait design findings.",
  summary: "Trait design reviewer.",
});

const source = port.input.text({ id: "source", description: "Canonical source trait text." });
const sourceDigest = port.input.text({ id: "source-digest", description: "sha256 digest of the source trait." });
const sourcePath = port.input.text({ id: "source-path", description: "Reviewed trait path." });
const sourceMap = port.input.text({
  id: "source-map",
  description: "Serialized source map whose anchors are authoritative.",
});
const anchor = schema.object(
  "source-anchor",
  { file: schema.text(), start: schema.number(), end: schema.number() },
  { description: "An exact one-based inclusive source-map location." },
);
const finding = schema.object(
  "review-finding",
  { rule: schema.text(), message: schema.text(), "construct-ref": schema.text(), anchor },
  { description: "One advisory source-backed design finding." },
);
const agentTraitsSchema = ref.resource({ id: "agent-traits-schema", dependency: "trait-spec" });
const designRubric = ref.resource({ id: "design-rubric", dependency: "trait-spec" });

// A schema-typed instruction-output: the return-format instruction folds
// into the step's compiled prompt, and its slot (auto-declared at the
// step's own id, "critique") backs the output port below.
const reviewOutput = output.of(
  schema.object(
    "review-scaffold",
    { "source-trait-id": schema.text(), "source-digest": schema.text(), findings: schema.array(finding) },
    { description: "An advisory critique with exact source-map anchors." },
  ),
)`Return exactly one structured review scaffold using ${sourceDigest}. Findings are advisory, not proof or enforcement. Use only these rules: unbounded-loop, unattributed-output, missing-review-before-final, over-abstraction, stringly-reference, weak-schema, over-broad-trust. Every finding must name a canonical construct reference and copy its anchor exactly from ${sourceMap}; do not guess, repair, or infer anchors. Ground the critique in ${agentTraitsSchema} and ${designRubric}.`;

const critiqueStep = sequence.prompt("critique", {
  title: "Critique trait",
  agent: critic,
  input: input.text`Critique ${source} at ${sourcePath}.`,
  output: reviewOutput,
});

const reviewPort = port.output.of({
  id: "review",
  schema: ref.schema("review-scaffold"),
  description: "Advisory critique scaffold.",
  value: reviewOutput,
});

export default trait("critique-trait", {
  version: "0.1.0",
  name: "Critique Trait",
  description: "Produces source-backed advisory trait design critiques.",
  metadata: { tag: ["first-party", "meta-trait", "critique"] },
  dependency: dependency({
    alias: "trait-spec",
    id: "trait-spec",
    version: "0.1.0",
    source: { path: "../trait-spec" },
  }),
  schema: [anchor, finding],
  procedure: procedure({
    description: "Report advisory design findings without modifying the reviewed trait.",
    input: [source, sourceDigest, sourcePath, sourceMap],
    output: reviewPort,
    sequence: critiqueStep,
  }),
});
