import { agent, dependency, input, output, port, procedure, ref, schema, sequence, trait } from "@ctx-traits/cdk";

const generator = agent("generator", {
    description: "Writes a plain-language advisory narration of a canonical trait.",
    summary: "Trait explanation narrator.",
});
const sourceTraitId = port.input.text({ id: "source-trait-id", description: "Explained trait identity." });
const canonicalDigest = port.input.text({ id: "canonical-digest", description: "Canonical digest that identifies the explained bytes." });
const canonicalTrait = port.input.text({ id: "canonical-trait", description: "Serialized normalized canonical trait data." });
const agentTraitsSchema = ref.resource({ id: "agent-traits-schema", dependency: "trait-spec" });

// A schema-typed instruction-output: the return-format instruction folds
// into the step's compiled prompt, and its slot (auto-declared at the
// step's own id, "explain-trait") backs the output port below.
const narrationOutput = output.of(
    schema.object("trait-narration", { "source-trait-id": schema.text(), "canonical-digest": schema.text(), explanation: schema.text() }),
)`Return exactly one trait-narration object repeating source-trait-id and canonical-digest verbatim with a non-empty explanation. This is generated advisory text, not authority, a check receipt, or a substitute for the canonical trait. Do not claim runtime behavior or validation that the canonical data does not establish. Use ${agentTraitsSchema} only to understand the canonical structure.`;

const explainStep = sequence.prompt("explain-trait", {
    title: "Explain trait",
    agent: generator,
    input: input.text`Explain canonical trait ${sourceTraitId} at ${canonicalDigest} in plain language using only ${canonicalTrait}.`,
    output: narrationOutput,
});

const narrationPort = port.output.of({ id: "explanation", schema: ref.schema("trait-narration"), description: "Generated advisory trait explanation.", value: narrationOutput });

export default trait("explain-trait", {
    version: "0.1.0",
    name: "Explain Trait",
    summary: "Produces a generated advisory explanation grounded in canonical trait data.",
    metadata: { tag: ["first-party", "meta-trait", "explain"] },
    dependency: dependency({ alias: "trait-spec", id: "trait-spec", version: "0.1.0", source: { path: "../trait-spec" } }),
    procedure: procedure({
        description: "Generate one untrusted plain-language narration bound to the supplied canonical trait identity and digest.",
        input: [sourceTraitId, canonicalDigest, canonicalTrait],
        output: narrationPort,
        sequence: explainStep,
    }),
});
