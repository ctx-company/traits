import { leftoverSchema } from "@ctx-traits/agents";
import { schema, slot } from "@ctx-traits/cdk";
import { reviewVerdict } from "./schema.ts";

export const draft = slot.text({
    id: "draft",
    description: "smart-1's implementation draft for the phase — the contract the worker implements.",
    hint: "Scope (restated from the plan), files to touch, approach, reuse/abstraction opportunities, validation plan, risks. A plan, not an implementation.",
});
export const workSummary = slot.text({
    id: "work-summary",
    description:
        "Worker's report of the implemented state; revised in place after each refinement pass.",
    hint: "What changed (files), how it was validated, and open concerns. Revised after every apply-fixes pass.",
});
export const commitOutput = slot.text({
    id: "commit-output",
    description: "Output evidence from the git commit command step: committed hash and subject.",
});

export const verdict = slot({
    id: "review-verdict",
    schema: reviewVerdict,
    description: "smart-1's refinement verdict for the current work state.",
});

