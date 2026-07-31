// Teaching template: pr-description
//
// Scaffolded by `ctx traits new <name> --from pr-description`. The last of
// the five single-prompt templates: a flat schema (no nested arrays), the
// simplest structured-output shape. Compare against `code-review` (an array
// of nested findings) and `test-writer` (an array of nested cases) once you
// need to grow beyond a flat record.

import { agent, input, output, port, procedure, ref, schema, sequence, trait } from "@ctx-traits/cdk";

const writer = agent("writer", {
    description: "Writes a pull-request description from a diff and its context.",
    summary: "PR description writer.",
});

const diff = port.input.text({ id: "diff", description: "The diff the pull request contains." });
const context = port.input.text({
    id: "context",
    description: "Why the change is being made, e.g. the linked issue or task.",
});

// A schema-typed instruction-output: the return-format instruction folds
// into the step's compiled prompt, and its slot (auto-declared at the
// step's own id, "write-description") backs the output port below.
const descriptionOutput = output.of(
    schema.object(
        "pr-description",
        {
            title: schema.text(),
            summary: schema.text(),
            "testing-notes": schema.text(),
            risk: schema.oneOf("pr-risk", ["low", "medium", "high"]),
        },
        { description: "A structured pull-request description: title, summary, testing notes, and risk level." },
    ),
)`Return exactly one structured description: a short title, a summary of what changed and why, testing notes describing how the change was or should be validated, and a risk level of low, medium, or high.`;

const writeStep = sequence.prompt("write-description", {
    title: "Write the description",
    agent: writer,
    input: input.text`Write a pull-request description for ${diff}, given the context ${context}.`,
    output: descriptionOutput,
});

const descriptionPort = port.output.of({
    id: "description",
    schema: ref.schema("pr-description"),
    description: "Structured pull-request description.",
    value: descriptionOutput,
});

export default trait("pr-description", {
    version: "0.1.0",
    name: "PR Description",
    summary: "Writes a structured pull-request description — title, summary, testing notes, and risk — from a diff and its context.",
    metadata: { tag: ["template", "pull-request"] },
    procedure: procedure({
        description: "Write a structured pull-request description from a diff and its context.",
        input: [diff, context],
        output: descriptionPort,
        sequence: writeStep,
    }),
});
