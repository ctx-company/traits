import { agent, dependency, input, output, port, procedure, ref, schema, sequence, trait } from "@ctx-traits/cdk";

const generator = agent("generator", {
  description:
    "Narrates a plain-language advisory explanation strictly grounded in a deterministic receipt-anchored scaffold.",
  summary: "Trait explanation narrator.",
});
const sourceTraitId = port.input.text({ id: "source-trait-id", description: "Explained trait identity." });
const receiptDigest = port.input.text({
  id: "receipt-digest",
  description: "Digest of the check receipt the scaffold is grounded in.",
});
const scaffold = port.input.text({
  id: "scaffold",
  description: "Deterministic explain-scaffold JSON text — the sole grounding evidence.",
});
const designRubric = ref.resource({ id: "design-rubric", dependency: "spec" });

const anchor = schema.object(
  "source-anchor",
  { file: schema.text(), start: schema.number(), end: schema.number() },
  { description: "An exact one-based inclusive source-map location." },
);
const section = schema.object(
  "explain-section",
  {
    title: schema.text(),
    "receipt-section": schema.text(),
    "receipt-fact": schema.text(),
    "construct-ref": schema.text(),
    anchor,
  },
  { description: "One receipt-grounded explanation section, copied verbatim from the input scaffold." },
);

// A schema-typed instruction-output: the return-format instruction folds
// into the step's compiled prompt, and its slot (auto-declared at the
// step's own id, "explain") backs the output port below.
const narrationOutput = output.of(
  schema.object("explain-scaffold", {
    "trait-id": schema.text(),
    "receipt-digest": schema.text(),
    sections: schema.list(section),
    warnings: schema.field(schema.list(schema.text()), {
      required: false,
      description: "Absent when the input scaffold carried none — never invented.",
    }),
    explanation: schema.text(),
  }),
)`Return exactly one explain-scaffold object. Copy trait-id, receipt-digest, sections, and warnings from ${scaffold} verbatim and unmodified — never alter, drop, or add a fact, anchor, or section. Add a non-empty explanation that narrates in plain language only what ${scaffold}'s sections establish, using the vocabulary of ${designRubric} — derived kind, speech act, dataflow — so the narration names what the trait is. This is generated advisory text, not authority, a check receipt, or a substitute for the canonical trait. Do not claim runtime behavior or validation that the scaffold does not establish.`;

const explainStep = sequence.prompt("explain", {
  title: "Explain trait",
  agent: generator,
  input: input.prompt`Narrate an explanation of trait ${sourceTraitId} at receipt ${receiptDigest}. Use only the scaffold below, verbatim.\n<<<SCAFFOLD>>>\n${scaffold}\n<<<END SCAFFOLD>>>`,
  output: narrationOutput,
});

const narrationPort = port.output.of({
  id: "explanation",
  schema: ref.schema("explain-scaffold"),
  description: "Generated advisory explanation, echoing its grounding scaffold verbatim.",
  value: narrationOutput,
});

export default trait("explain", {
  version: "0.1.0",
  name: "Explain Trait",
  description:
    "Narrates a plain-language advisory explanation strictly grounded in a deterministic receipt-anchored scaffold.",
  metadata: { tag: ["first-party", "meta-trait", "explain"] },
  dependency: dependency({
    alias: "spec",
    id: "spec",
    version: "0.1.0",
    source: { path: "../spec" },
  }),
  schema: [anchor, section],
  port: narrationPort,
  procedure: procedure({
    description:
      "Generate one untrusted plain-language narration strictly grounded in the supplied deterministic explain scaffold.",
    input: [sourceTraitId, receiptDigest, scaffold],
    output: narrationPort,
    sequence: explainStep,
  }),
});
