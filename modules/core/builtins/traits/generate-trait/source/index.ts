import { agent, dependency, port, procedure, prompt, ref, schema, sequence, slot, trait } from "@ctx-traits/cdk";

const generator = agent("generator", {
    description: "Produces one complete trait draft from a concise authoring goal.",
    summary: "Trait draft generator.",
});

const name = port.input.text({ id: "name", description: "Human-readable trait name." });
const brief = port.input.text({ id: "brief", description: "Goal and constraints for the new trait." });
const draft = slot({
    id: "candidate",
    schema: schema.object("trait-draft", { trait: schema.any() }, { description: "One complete canonical trait draft; the candidate gate validates its full shape." }),
    description: "Structured trait draft emitted for candidate validation.",
});
const output = port.output.of({ id: "candidate", schema: ref.schema("trait-draft"), description: "Generated trait candidate.", value: draft });

export default trait("generate-trait", {
    version: "0.1.0",
    name: "Generate Trait",
    summary: "Generates one complete trait draft grounded in the trait specification dependency.",
    metadata: { tag: ["first-party", "meta-trait", "generation"] },
    dependency: dependency({ alias: "trait-spec", id: "trait-spec", version: "0.1.0", source: { path: "../trait-spec" } }),
    procedure: procedure({
        description: "Produce one structured trait draft; deterministic candidate gates verify it downstream.",
        input: [name, brief],
        output,
        sequence: sequence.prompt("generate", {
            title: "Generate trait",
            agent: generator,
            input: [name, brief, ref.resource({ id: "agent-traits-schema", dependency: "trait-spec" }), ref.resource({ id: "design-rubric", dependency: "trait-spec" })],
            text: prompt.text`Create exactly one complete canonical trait draft for ${name} from ${brief}. Ground it in ${ref.resource({ id: "agent-traits-schema", dependency: "trait-spec" })} and ${ref.resource({ id: "design-rubric", dependency: "trait-spec" })}. Return structured output only.`,
            output: draft,
        }),
    }),
});
