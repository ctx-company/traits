import { agent, dependency, input, output, port, procedure, ref, schema, sequence, trait } from "@ctx-traits/cdk";

const generator = agent("generator", {
    description: "Produces one complete trait draft from a concise authoring goal.",
    summary: "Trait draft generator.",
});

const name = port.input.text({ id: "name", description: "Human-readable trait name." });
const brief = port.input.text({ id: "brief", description: "Goal and constraints for the new trait." });
const agentTraitsSchema = ref.resource({ id: "agent-traits-schema", dependency: "trait-spec" });
const designRubric = ref.resource({ id: "design-rubric", dependency: "trait-spec" });

// A schema-typed instruction-output: the return-format instruction folds
// into the step's compiled prompt, and its slot (auto-declared at the
// step's own id, "generate") backs the output port below.
const candidateOutput = output.of(
    schema.object("trait-draft", { trait: schema.any() }, { description: "One complete canonical trait draft; the candidate gate validates its full shape." }),
)`Ground it in ${agentTraitsSchema} and ${designRubric}. Return structured output only.`;

const generateStep = sequence.prompt("generate", {
    title: "Generate trait",
    agent: generator,
    input: input.text`Create exactly one complete canonical trait draft for ${name} from ${brief}.`,
    output: candidateOutput,
});

const candidatePort = port.output.of({ id: "candidate", schema: ref.schema("trait-draft"), description: "Generated trait candidate.", value: candidateOutput });

export default trait("generate-trait", {
    version: "0.1.0",
    name: "Generate Trait",
    summary: "Generates one complete trait draft grounded in the trait specification dependency.",
    metadata: { tag: ["first-party", "meta-trait", "generation"] },
    dependency: dependency({ alias: "trait-spec", id: "trait-spec", version: "0.1.0", source: { path: "../trait-spec" } }),
    procedure: procedure({
        description: "Produce one structured trait draft; deterministic candidate gates verify it downstream.",
        input: [name, brief],
        output: candidatePort,
        sequence: generateStep,
    }),
});
