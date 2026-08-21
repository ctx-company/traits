import { port, schema, slot } from "@ctx-traits/cdk";

import { reviewStatusSchema, reviewVerdictSchema } from "#trait/shared/schema.ts";

export const target = port.input.text({
  id: "target",
  description: "Module, file path, or code area to optimize the benchmark over.",
});
export const benchmarkCommand = port.input.of("benchmark-command", schema.list(schema.text()), {
  description:
    'No-shell argv executed for one baseline or candidate run; must emit JSON matching { status: "ok" | "error", metric: number, "delta-lines"?: number }. The optional delta-lines feeds the max-delta-lines cap; omit it if unmeasured.',
});
export const improvementTarget = port.input.of("improvement-target", schema.number(), {
  description:
    "Lower-is-better metric target; the run completes immediately once the baseline or any kept candidate reaches it.",
});
export const noiseThreshold = port.input.of("noise-threshold", schema.number(), {
  description:
    "Non-negative minimum improvement margin below which a lower aggregate metric is treated as noise, not a real improvement.",
});

export const reviewVerdictSlot = slot({
  id: "review-verdict",
  schema: reviewVerdictSchema(),
  description: "smart-1's single review verdict for the implemented candidate.",
});
export const marginResult = slot.boolean({
  id: "margin-result",
  description: "Whether the candidate's aggregate metric improves on best by more than the noise threshold.",
});
export const roundComplete = slot({
  id: "round-complete",
  schema: schema.enum("optimize-benchmark-round-status", ["open", "complete"], {
    description: "Whether the current round's atomic record step has finished.",
  }),
  description:
    '"open" at round start, "complete" only in the final atomic record step of the round\'s arm — gates the loop\'s `until` so the runtime\'s after-each-step guard evaluation can never terminate mid-commit/reset.',
});
export const reviews = slot.list(reviewStatusSchema, {
  id: "reviews",
  description: "Review verdict status for every round that reached review, in execution order.",
});
