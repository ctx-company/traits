// Teaching template: test-writer
//
// Scaffolded by `ctx traits new <name> --from test-writer`. Another
// single-prompt, single-schema trait, shaped like `research-summarize` but
// worth keeping separate: it demonstrates a *list of nested objects* schema
// (each proposed test case is its own small object), which is the shape
// most "propose N things, each with a few fields" traits want.

import { agent, input, output, port, procedure, ref, schema, sequence, trait } from "@ctx-traits/cdk";

const testWriter = agent("test-writer", {
    description: "Proposes a structured test plan for a piece of code against stated requirements.",
    summary: "Test plan writer.",
});

const targetCode = port.input.text({ id: "target-code", description: "The code to write a test plan for." });
const requirements = port.input.text({
    id: "requirements",
    description: "The behavior the code must satisfy, e.g. acceptance criteria or a spec excerpt.",
});

// A nested object schema: each test case is its own small shape, collected
// into an array on the top-level `test-plan` schema below.
const testCase = schema.object(
    "proposed-test-case",
    {
        name: schema.text(),
        setup: schema.text(),
        assertion: schema.text(),
    },
    { description: "One proposed test case: what it sets up and what it asserts." },
);

// A schema-typed instruction-output: the return-format instruction folds
// into the step's compiled prompt, and its slot (auto-declared at the
// step's own id, "write-plan") backs the output port below.
const planOutput = output.of(
    schema.object(
        "test-plan",
        {
            cases: schema.array(testCase),
            "coverage-gaps": schema.array(schema.text()),
        },
        { description: "A structured test plan: proposed cases plus any requirements the plan can't cover." },
    ),
)`Return exactly one structured plan: one case per behavior worth testing (name, setup, and the assertion it makes), plus a list of any requirements the proposed cases don't cover. Do not write the actual test code — describe the cases.`;

const writePlanStep = sequence.prompt("write-plan", {
    title: "Write the test plan",
    agent: testWriter,
    input: input.text`Propose a test plan for ${targetCode} against the requirements ${requirements}.`,
    output: planOutput,
});

const planPort = port.output.of({
    id: "plan",
    schema: ref.schema("test-plan"),
    description: "Structured proposed test plan.",
    value: planOutput,
});

export default trait("test-writer", {
    version: "0.1.0",
    name: "Test Writer",
    summary: "Proposes a structured test plan for a piece of code against stated requirements.",
    metadata: { tag: ["template", "testing"] },
    schema: [testCase],
    procedure: procedure({
        description: "Propose a test plan for the target code against the stated requirements.",
        input: [targetCode, requirements],
        output: planPort,
        sequence: writePlanStep,
    }),
});
