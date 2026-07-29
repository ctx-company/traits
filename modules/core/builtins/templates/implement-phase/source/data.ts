import { port as cdkPort, ref, slot as cdkSlot } from "@ctx-traits/cdk";

import * as schemas from "./schema.ts";

// The trait's one input: the task to implement.
const task = cdkPort.input.text({
    id: "task",
    description: "The task to implement, in enough detail to plan from (scope, constraints, done-when).",
});

// Three slots, one per step's structured output. `draft` and `workSummary`
// are typed as plain text — free-form is the right shape for a plan and a
// change summary. `verdict` is a small structured schema so the trait's
// caller gets a stable field to branch on instead of parsing prose.
const draft = cdkSlot.text({
    id: "draft",
    description: "The implementation draft: scope, approach, files to touch, and how it will be validated.",
});
const workSummary = cdkSlot.text({
    id: "work-summary",
    description: "What the worker changed and how it was validated.",
});
const verdict = cdkSlot({
    id: "verdict",
    schema: schemas.implementationVerdict,
    description: "The reviewer's structured verdict for the implemented work.",
});

const output = cdkPort.output.of({
    id: "verdict",
    schema: ref.schema("implementation-verdict"),
    description: "The reviewer's final verdict on the implemented work.",
    value: verdict,
});

export const port = { task, output };
export const slot = { draft, workSummary, verdict };
