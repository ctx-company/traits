// Teaching template: research-summarize
//
// Scaffolded by `ctx traits new <name> --from research-summarize`. A single
// prompt step that turns a question plus supplied source material into a
// structured summary — the smallest complete shape for a "read this and
// tell me what it means" trait: one input pair, one schema, one output.
//
// The `sources` port is deliberately plain text rather than a file
// reference: this template's caller is expected to paste or interpolate the
// material to summarize. For a trait that reads files itself, declare a
// `resource(...)` instead (see the CDK's `resource` export and
// `modules/core/builtins/templates/implement-phase`'s sibling live
// procedure at `.ctx/traits/implement-phase/` in this repo, which reads a
// plan file that way) and pass it as prompt input the same way a port is
// passed.

import { agent, port, procedure, prompt, ref, schema, sequence, slot, trait } from "@ctx-traits/cdk";

const researcher = agent("researcher", {
    description: "Summarizes source material against a research question, citing exactly what it read.",
    summary: "Research summarizer.",
});

const question = port.input.text({ id: "question", description: "The research question to answer." });
const sources = port.input.text({
    id: "sources",
    description: "The source material to summarize, e.g. pasted excerpts or search results.",
});

const summary = slot({
    id: "summary",
    schema: schema.object(
        "research-summary",
        {
            "key-findings": schema.array(schema.text()),
            "open-questions": schema.array(schema.text()),
            citations: schema.array(schema.text()),
        },
        { description: "A structured research summary: what was found, what remains open, and what it's sourced from." },
    ),
    description: "The structured summary of the source material against the research question.",
});

const output = port.output.of({
    id: "summary",
    schema: ref.schema("research-summary"),
    description: "Structured research summary.",
    value: summary,
});

export default trait("research-summarize", {
    version: "0.1.0",
    name: "Research Summarize",
    summary: "Summarizes a research question against supplied source material into key findings, open questions, and citations.",
    metadata: { tag: ["template", "research"] },
    procedure: procedure({
        description: "Answer a research question from supplied source material with a structured, cited summary.",
        input: [question, sources],
        output,
        sequence: sequence.prompt("summarize", {
            title: "Summarize the sources",
            agent: researcher,
            text: prompt.text`Answer ${question} using only ${sources}. Return exactly one structured summary: a list of key findings, a list of open questions the sources leave unanswered, and a list of citations identifying which part of the source material backs each finding. Do not use outside knowledge the sources don't support.`,
            output: summary,
            input: [question, sources],
        }),
    }),
});
