import { agent, dependency, port, procedure, prompt, ref, schema, sequence, slot, trait } from "@ctx-traits/cdk";

const generator = agent("generator", { description: "Proposes deferred eval declarations.", summary: "Eval synthesis author." });
const source = port.input.text({ id: "source", description: "Canonical source trait text." });
const sourceTraitId = port.input.text({ id: "source-trait-id", description: "Source trait identity." });
const sourceDigest = port.input.text({ id: "source-digest", description: "Source trait digest." });
const ioContract = port.input.text({ id: "io-contract", description: "Serialized source trait I/O contract." });
const scenario = schema.object("scenario", { id: schema.text(), variant: schema.text(), input: schema.text(), output: schema.text() });
const evalProposal = schema.object("eval-proposal", { id: schema.text(), variant: schema.text(), input: schema.text(), output: schema.text(), scenario: schema.any() });
const synthesis = slot({
    id: "eval-synthesis",
    schema: schema.object("eval-synthesis-scaffold", { "source-trait-id": schema.text(), "source-digest": schema.text(), scenarios: schema.array(scenario), evals: schema.array(evalProposal) }),
    description: "Untrusted generated scenarios and deferred eval declarations.",
});
const output = port.output.of({ id: "eval-synthesis", schema: ref.schema("eval-synthesis-scaffold"), description: "Eval synthesis scaffold.", value: synthesis });

export default trait("generate-evals", {
    version: "0.1.0",
    name: "Generate Evals",
    summary: "Proposes source-grounded scenarios and deferred behavioral/runtime eval declarations.",
    metadata: { tag: ["first-party", "meta-trait", "eval-generation"] },
    dependency: dependency({ alias: "trait-spec", id: "trait-spec", version: "0.1.0", source: { path: "../trait-spec" } }),
    schema: [scenario, evalProposal],
    procedure: procedure({
        description: "Generate untrusted declarations; the CLI merges and checks the complete candidate trait.",
        input: [source, sourceTraitId, sourceDigest, ioContract],
        output,
        sequence: sequence.prompt("generate-evals", {
            title: "Generate eval declarations",
            agent: generator,
            input: [source, sourceTraitId, sourceDigest, ioContract, ref.resource({ id: "agent-traits-schema", dependency: "trait-spec" }), ref.resource({ id: "design-rubric", dependency: "trait-spec" })],
            text: prompt.text`Read ${source} and its I/O contract ${ioContract}. Return exactly one eval-synthesis scaffold for ${sourceTraitId} using ${sourceDigest}. Propose positive, negative, and edge scenarios where justified. Scenarios are specifications, not evidence. Every eval must use only behavioral or runtime, link scenarios through canonical singular scenario (a scalar or array), and use only render:, fixture:, trait:, or report: input/output refs. Behavioral and runtime executors are deferred; do not claim they ran. Ground proposals in ${ref.resource({ id: "agent-traits-schema", dependency: "trait-spec" })} and ${ref.resource({ id: "design-rubric", dependency: "trait-spec" })}.`,
            output: synthesis,
        }),
    }),
});
