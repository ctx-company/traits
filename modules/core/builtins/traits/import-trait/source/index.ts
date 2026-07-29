import { agent, dependency, port, procedure, prompt, ref, schema, sequence, slot, trait } from "@ctx-traits/cdk";

const generator = agent("generator", {
    description: "Enriches a deterministic import scaffold with source-grounded trait content.",
    summary: "Trait import enrichment agent.",
});

const scaffold = port.input.text({ id: "scaffold", description: "Deterministic import scaffold (trait.toml text) to use as the authoritative baseline." });
const source = port.input.text({ id: "source", description: "Raw source text (e.g. SKILL.md) to ground enrichment in." });
const sourceProfile = port.input.text({ id: "source-profile", description: "Profile describing the source format and its known fields." });
const traitId = port.input.text({ id: "trait-id", description: "Trait id the imported package must keep." });
const targetSchema = port.input.text({ id: "target-schema", description: "Target canonical schema (agent-traits/canonical-trait) the draft must conform to." });
const draft = slot({
    id: "candidate",
    schema: schema.object("trait-draft", { trait: schema.any() }, { description: "One complete canonical trait draft; the candidate gate validates its full shape." }),
    description: "Structured trait draft emitted for candidate validation.",
});
const output = port.output.of({ id: "candidate", schema: ref.schema("trait-draft"), description: "Imported trait candidate.", value: draft });

export default trait("import-trait", {
    version: "0.1.0",
    name: "Import Trait",
    summary: "Enriches a deterministic import scaffold with source-grounded trait content, grounded in the trait specification dependency.",
    metadata: { tag: ["first-party", "meta-trait", "import"] },
    dependency: dependency({ alias: "trait-spec", id: "trait-spec", version: "0.1.0", source: { path: "../trait-spec" } }),
    procedure: procedure({
        description: "Produce one structured trait draft grounded in the deterministic scaffold and raw source; deterministic candidate gates verify it downstream.",
        input: [scaffold, source, sourceProfile, traitId, targetSchema],
        output,
        sequence: sequence.prompt("import", {
            title: "Import trait",
            agent: generator,
            input: [scaffold, source, sourceProfile, traitId, targetSchema, ref.resource({ id: "agent-traits-schema", dependency: "trait-spec" }), ref.resource({ id: "design-rubric", dependency: "trait-spec" })],
            text: prompt.text`Treat ${scaffold} as the authoritative baseline for trait ${traitId}, imported from a ${sourceProfile} source. Preserve trait identity and import provenance exactly as given in ${scaffold}. Enrich only intent, behavior, metadata, and descriptions that ${source} actually supports; never invent claims the source does not make. Conform to ${targetSchema}, grounding the result in ${ref.resource({ id: "agent-traits-schema", dependency: "trait-spec" })} and ${ref.resource({ id: "design-rubric", dependency: "trait-spec" })}. Return exactly one structured trait draft.`,
            output: draft,
        }),
    }),
});
