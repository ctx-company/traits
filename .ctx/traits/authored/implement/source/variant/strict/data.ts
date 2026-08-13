import { deviationReportSchema, reviewerVerdict, reviewVerdictSchema } from "@ctx-traits/agents";
import { schema, slot } from "@ctx-traits/cdk";

export const planningVerdict = slot({
  id: "planning-verdict",
  schema: reviewerVerdict,
  description: 'Reviewer verdict for the "planning" produce-review round.',
});

export const deviationReport = slot({
  id: "deviation-report",
  schema: schema.list(deviationReportSchema),
  description:
    "Every proposed departure from the verbatim draft — including a rejected improvement the worker considered but did not make — recorded instead of silently adapted. Empty when the draft was followed exactly.",
});

export const verdict1 = slot({
  id: "review-verdict-1",
  schema: reviewVerdictSchema,
  description: "smart-1's refinement verdict for the current work state.",
});
export const verdict2 = slot({
  id: "review-verdict-2",
  schema: reviewVerdictSchema,
  description: "smart-2's refinement verdict for the current work state.",
});

export const buildingSealed = slot.boolean("building-sealed");
