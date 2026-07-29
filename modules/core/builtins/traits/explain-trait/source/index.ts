import { agent, dependency, port, procedure, prompt, ref, schema, sequence, slot, trait } from "@ctx-traits/cdk";

const generator = agent("generator", {
    description: "Writes a plain-language advisory narration of a canonical trait.",
    summary: "Trait explanation narrator.",
});
const sourceTraitId = port.input.text({ id: "source-trait-id", description: "Explained trait identity." });
const canonicalDigest = port.input.text({ id: "canonical-digest", description: "Canonical digest that identifies the explained bytes." });
const canonicalTrait = port.input.text({ id: "canonical-trait", description: "Serialized normalized canonical trait data." });
const narration = slot({
    id: "explanation",
    schema: schema.object("trait-narration", { "source-trait-id": schema.text(), "canonical-digest": schema.text(), explanation: schema.text() }),
    description: "Generated advisory narration, bound to one trait identity and canonical digest.",
});
const output = port.output.of({ id: "explanation", schema: ref.schema("trait-narration"), description: "Generated advisory trait explanation.", value: narration });

export default trait("explain-trait", {
    version: "0.1.0",
    name: "Explain Trait",
    summary: "Produces a generated advisory explanation grounded in canonical trait data.",
    metadata: { tag: ["first-party", "meta-trait", "explain"] },
    dependency: dependency({ alias: "trait-spec", id: "trait-spec", version: "0.1.0", source: { path: "../trait-spec" } }),
    procedure: procedure({
        description: "Generate one untrusted plain-language narration bound to the supplied canonical trait identity and digest.",
        input: [sourceTraitId, canonicalDigest, canonicalTrait],
        output,
        sequence: sequence.prompt("explain-trait", {
            title: "Explain trait",
            agent: generator,
            input: [sourceTraitId, canonicalDigest, canonicalTrait, ref.resource({ id: "agent-traits-schema", dependency: "trait-spec" })],
            text: prompt.text`Explain canonical trait ${sourceTraitId} at ${canonicalDigest} in plain language using only ${canonicalTrait}. Return exactly one trait-narration object repeating source-trait-id and canonical-digest verbatim with a non-empty explanation. This is generated advisory text, not authority, a check receipt, or a substitute for the canonical trait. Do not claim runtime behavior or validation that the canonical data does not establish. Use ${ref.resource({ id: "agent-traits-schema", dependency: "trait-spec" })} only to understand the canonical structure.`,
            output: narration,
        }),
    }),
});
