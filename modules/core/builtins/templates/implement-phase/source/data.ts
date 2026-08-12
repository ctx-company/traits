import { port as cdkPort, slot as cdkSlot } from "@ctx-traits/cdk";

// The trait's one input: the task to implement.
const task = cdkPort.input.text({
  id: "task",
  description: "The task to implement, in enough detail to plan from (scope, constraints, done-when).",
});

// Two slots, one per plan/implementation step's structured output — plain
// text is the right shape for a free-form plan and change summary. The
// review step's structured verdict is declared inline as an `output.of(...)`
// instruction-output instead (see `sequence/review.ts`): its slot and the
// trait's output port are both derived from that one declaration.
const draft = cdkSlot.text({
  id: "draft",
  description: "The implementation draft: scope, approach, files to touch, and how it will be validated.",
});
const workSummary = cdkSlot.text({
  id: "work-summary",
  description: "What the worker changed and how it was validated.",
});

export const port = { task };
export const slot = { draft, workSummary };
