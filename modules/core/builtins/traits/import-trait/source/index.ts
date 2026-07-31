import { agent, dependency, input, output, port, procedure, ref, schema, sequence, trait } from "@ctx-traits/cdk";

const generator = agent("generator", {
    description: "Enriches a deterministic import scaffold with source-grounded trait content.",
    summary: "Trait import enrichment agent.",
});

const scaffold = port.input.text({ id: "scaffold", description: "Deterministic import scaffold (trait.toml text) to use as the authoritative baseline." });
const source = port.input.text({ id: "source", description: "Raw source text (e.g. SKILL.md) to ground enrichment in." });
const sourceProfile = port.input.text({ id: "source-profile", description: "Profile describing the source format and its known fields." });
const traitId = port.input.text({ id: "trait-id", description: "Trait id the imported package must keep." });
const targetSchema = port.input.text({ id: "target-schema", description: "Target canonical schema (agent-traits/canonical-trait) the draft must conform to." });
const agentTraitsSchema = ref.resource({ id: "agent-traits-schema", dependency: "trait-spec" });
const designRubric = ref.resource({ id: "design-rubric", dependency: "trait-spec" });

// A schema-typed instruction-output: the return-format instruction folds
// into the step's compiled prompt, and its slot (auto-declared at the
// step's own id, "import") backs the output port below.
const candidateOutput = output.of(
    schema.object("trait-draft", { trait: schema.any() }, { description: "One complete canonical trait draft; the candidate gate validates its full shape." }),
)`Conform to ${targetSchema}, grounding the result in ${agentTraitsSchema} and ${designRubric}. Return exactly one structured trait draft.`;

const importStep = sequence.prompt("import", {
    title: "Import trait",
    agent: generator,
    input: input.text`Treat ${scaffold} as the authoritative baseline for trait ${traitId}, imported from a ${sourceProfile} source. Preserve trait identity and import provenance exactly as given in ${scaffold}. Enrich only intent, behavior, metadata, and descriptions that ${source} actually supports; never invent claims the source does not make.`,
    output: candidateOutput,
});

const candidatePort = port.output.of({ id: "candidate", schema: ref.schema("trait-draft"), description: "Imported trait candidate.", value: candidateOutput });

export default trait("import-trait", {
    version: "0.1.0",
    name: "Import Trait",
    summary: "Enriches a deterministic import scaffold with source-grounded trait content, grounded in the trait specification dependency.",
    metadata: { tag: ["first-party", "meta-trait", "import"] },
    dependency: dependency({ alias: "trait-spec", id: "trait-spec", version: "0.1.0", source: { path: "../trait-spec" } }),
    procedure: procedure({
        description: "Produce one structured trait draft grounded in the deterministic scaffold and raw source; deterministic candidate gates verify it downstream.",
        input: [scaffold, source, sourceProfile, traitId, targetSchema],
        output: candidatePort,
        sequence: importStep,
    }),
});
