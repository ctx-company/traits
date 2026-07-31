import { agent, dependency, input, output, port, procedure, ref, schema, sequence, trait } from "@ctx-traits/cdk";

const refiner = agent("refiner", {
    description: "Produces source-anchored trait refinement scaffolds.",
    summary: "Trait refinement agent.",
});

const source = port.input.text({ id: "source", description: "Canonical source trait text." });
const sourceDigest = port.input.text({ id: "source-digest", description: "sha256 digest of the source trait." });
const sourcePath = port.input.text({ id: "source-path", description: "Filesystem path whose lines anchors must reference." });
const changeRequest = port.input.text({ id: "change-request", description: "Requested refinement." });
const sourceAnchor = schema.object("source-anchor", { file: schema.text(), start: schema.number(), end: schema.number() }, { description: "A one-based inclusive source location." });
const refinePatch = schema.object("refine-patch", { change: schema.text(), anchor: sourceAnchor }, { description: "One requested source-anchored refinement." });
const agentTraitsSchema = ref.resource({ id: "agent-traits-schema", dependency: "trait-spec" });
const designRubric = ref.resource({ id: "design-rubric", dependency: "trait-spec" });

// A schema-typed instruction-output: the return-format instruction folds
// into the step's compiled prompt, and its slot (auto-declared at the
// step's own id, "refine") backs the output port below.
const refinementOutput = output.of(
    schema.object("refine-scaffold", { "source-trait-id": schema.text(), "source-digest": schema.text(), "proposed-trait": schema.any(), patches: schema.array(refinePatch) }, { description: "A source-anchored refinement proposal." }),
)`Ground the result in ${agentTraitsSchema} and ${designRubric}. Return exactly one structured refine scaffold with anchored patches.`;

const refineStep = sequence.prompt("refine", {
    title: "Refine trait",
    agent: refiner,
    input: input.text`Refine ${source} at ${sourcePath} for ${changeRequest}. Preserve source identity, use ${sourceDigest} as source evidence, and anchor every patch to real one-based inclusive lines in ${sourcePath}.`,
    output: refinementOutput,
});

const refinementPort = port.output.of({ id: "refinement", schema: ref.schema("refine-scaffold"), description: "Refinement scaffold.", value: refinementOutput });

export default trait("refine-trait", {
    version: "0.1.0",
    name: "Refine Trait",
    summary: "Produces a validated source-anchored trait refinement scaffold.",
    metadata: { tag: ["first-party", "meta-trait", "refinement"] },
    dependency: dependency({ alias: "trait-spec", id: "trait-spec", version: "0.1.0", source: { path: "../trait-spec" } }),
    schema: [sourceAnchor, refinePatch],
    procedure: procedure({
        description: "Propose one refinement scaffold whose trait identity remains equal to the source identity.",
        input: [source, sourceDigest, sourcePath, changeRequest],
        output: refinementPort,
        sequence: refineStep,
    }),
});
