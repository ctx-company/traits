import { input, output, sequence } from "@ctx-traits/cdk";

import { agent } from "../agent.ts";
import { port, slot } from "../data.ts";
import { implementationVerdict } from "../schema.ts";

// A schema-typed instruction-output: auto-declares this step's verdict slot
// (id "review", the step id) and folds the return-format instruction into
// the compiled prompt — `index.ts` binds the trait's output port straight to
// this handle, so the schema, the slot, and the port all trace to one
// declaration.
export const verdict = output.of(implementationVerdict)`Return a verdict of approved or revise, with notes explaining the verdict.`;

export default sequence.prompt("review", {
    title: "Review the result",
    agent: agent.reviewer,
    input: input.text`Review the implemented work for ${port.task} against the draft ${slot.draft}. The reported work summary is ${slot.workSummary}.`,
    output: verdict,
});
