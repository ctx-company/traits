import { agent, dependency, port, procedure, prompt, ref, schema, sequence, slot, trait } from "@ctx-traits/cdk";

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
const scaffold = slot({
    id: "refinement",
    schema: schema.object("refine-scaffold", { "source-trait-id": schema.text(), "source-digest": schema.text(), "proposed-trait": schema.any(), patches: schema.array(refinePatch) }, { description: "A source-anchored refinement proposal." }),
    description: "Typed refinement scaffold for validation and candidate evaluation.",
});
const output = port.output.of({ id: "refinement", schema: ref.schema("refine-scaffold"), description: "Refinement scaffold.", value: scaffold });

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
        output,
        sequence: sequence.prompt("refine", {
            title: "Refine trait",
            agent: refiner,
            input: [source, sourceDigest, sourcePath, changeRequest, ref.resource({ id: "agent-traits-schema", dependency: "trait-spec" }), ref.resource({ id: "design-rubric", dependency: "trait-spec" })],
            text: prompt.text`Refine ${source} at ${sourcePath} for ${changeRequest}. Preserve source identity, use ${sourceDigest} as source evidence, and anchor every patch to real one-based inclusive lines in ${sourcePath}. Ground the result in ${ref.resource({ id: "agent-traits-schema", dependency: "trait-spec" })} and ${ref.resource({ id: "design-rubric", dependency: "trait-spec" })}. Return exactly one structured refine scaffold with anchored patches.`,
            output: scaffold,
        }),
    }),
});
