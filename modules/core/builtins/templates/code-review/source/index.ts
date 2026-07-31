// Teaching template: code-review
//
// Scaffolded by `ctx traits new <name> --from code-review`. This file is a
// complete, buildable v2 CDK trait source — the same authoring surface every
// hand-authored trait uses (see modules/core/builtins/traits/*/source for
// the first-party meta-traits this pattern is drawn from). Read the comments
// below, then edit freely: nothing here is special because it came from a
// template.
//
// Shape of one CDK trait:
//   1. Declare ports  — the trait's typed input/output boundary.
//   2. Declare a schema — the shape of the step's structured output.
//   3. Declare an agent — an abstract role a step delegates to.
//   4. Wire one `procedure` with a `sequence` of steps that read ports and
//      end at an `output.of(schema)` instruction-output, which auto-declares
//      its own backing slot.
//
// `ctx traits build` compiles this file to `generated/index.toml` (the
// canonical trait document) and `generated/index.map` (a source map from
// every canonical construct back to this file). `ctx traits new` also runs
// `ctx traits sync` immediately after, writing `trait.lock` — the same
// build + lock path `ctx traits build` and `ctx traits sync` use directly.

import { agent, input, output, port, procedure, ref, schema, sequence, trait } from "@ctx-traits/cdk";

// An abstract role: the runtime resolves "reviewer" to a configured agent
// harness at run time. The trait only ever refers to this handle.
const reviewer = agent("reviewer", {
    description: "Reviews a diff for correctness, risk, and scope, and reports findings the author can act on.",
    summary: "Diff reviewer.",
});

// Input ports: the trait's typed request boundary.
const diff = port.input.text({ id: "diff", description: "The diff or changed-files excerpt to review." });
const focus = port.input.text({
    id: "focus",
    description: "What to focus the review on, e.g. \"correctness and error handling\" or \"public API surface\".",
});

// One structured finding: a stable shape callers can render or filter on,
// instead of parsing prose.
const finding = schema.object(
    "code-review-finding",
    {
        severity: schema.field(schema.oneOf("severity", ["blocker", "advisory"]), {
            description: "Whether this finding must be fixed before merge, or is a lower-priority suggestion.",
        }),
        file: schema.text(),
        summary: schema.text(),
    },
    { description: "One reviewer finding tied to a specific file." },
);

// A schema-typed instruction-output: the return-format instruction folds
// into the review step's compiled prompt, and its slot (auto-declared at
// the step's own id, "review") backs the trait's output port below — one
// declaration, not a separately hand-declared slot plus hand-written prose.
const reviewOutput = output.of(
    schema.object(
        "code-review-scaffold",
        {
            verdict: schema.verdict(),
            findings: schema.array(finding),
        },
        { description: "A structured code review: a pass/fail/needs-changes verdict plus per-file findings." },
    ),
)`Return exactly one structured review: a verdict of pass, needs-changes, or fail, and one finding per issue you find, each naming its file, a severity of blocker or advisory, and a concise summary. Do not rewrite the diff; report only.`;

// A one-step sequence: one prompt, one agent, one structured output.
// Multi-step traits chain several `sequence.prompt` (or other sequence
// kinds) entries in an array instead of a single value — see
// `modules/core/builtins/templates/implement-phase` for that shape.
const reviewStep = sequence.prompt("review", {
    title: "Review the diff",
    agent: reviewer,
    input: input.text`Review ${diff} with a focus on ${focus}.`,
    output: reviewOutput,
});

// The output port: what callers of this trait receive, bound straight to
// the review step's instruction-output.
const reviewPort = port.output.of({
    id: "review",
    schema: ref.schema("code-review-scaffold"),
    description: "Structured review verdict and findings.",
    value: reviewOutput,
});

export default trait("code-review", {
    version: "0.1.0",
    name: "Code Review",
    summary: "Reviews a diff against a stated focus and returns structured, source-anchored findings with a pass/needs-changes verdict.",
    metadata: { tag: ["template", "review"] },
    schema: [finding],
    procedure: procedure({
        description: "Review a diff for the stated focus and return one structured verdict with findings.",
        input: [diff, focus],
        output: reviewPort,
        sequence: reviewStep,
    }),
});
