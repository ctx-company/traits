import { port, schema, slot } from "@ctx-traits/cdk";

import { aggregateResult, decisionStatus, gitRefSchema, proposalSchema, readiness, summarySchema } from "#trait/shared/schema.ts";

export const maxDelta = port.input.of(schema.integer(), {
  id: "max-delta-lines",
  optional: true,
  description:
    "Optional maximum accepted changed lines relative to the current best Git ref; the cap is skipped when omitted, and discards a candidate that supplies no delta-lines measurement.",
});
export const benchmarkRuns = port.input.of(schema.integer(), {
  id: "benchmark-runs",
  description:
    "Number of measurement runs per baseline/candidate; the deterministic aggregate (median) feeds the keep/discard guard. Odd counts pick the middle run; even counts pick the upper-median run — always an actually-measured value.",
});

export const readinessSlot = slot({
  id: "readiness",
  schema: readiness,
  description: "Latest typed worktree readiness result.",
});
export const bestRef = slot.text({
  id: "best-ref",
  description: "Immutable current-best commit SHA, advanced only after a kept commit succeeds.",
});
export const capturedRef = slot({
  id: "captured-ref",
  schema: gitRefSchema,
  description: "Latest immutable commit SHA captured by a trusted Git command.",
});
export const commandReceipt = slot.text({
  id: "command-receipt",
  description: "Latest deterministic Git command's raw stdout.",
});
export const baselineResult = slot({
  id: "baseline-result",
  schema: aggregateResult,
  description: "Trusted N-run aggregate measurement that seeds best state.",
});
export const candidateResult = slot({
  id: "candidate-result",
  schema: aggregateResult,
  description: "Current trusted N-run aggregate measurement read by the declared keep guard.",
});
export const bestMetric = slot.number({
  id: "best-metric",
  description: "Loop-carried lower-is-better metric projected only from trusted command output.",
});
export const history = slot.list(aggregateResult, {
  id: "history",
  description: "Baseline and candidate aggregate measurements in deterministic ledger order.",
});
export const roundCount = slot({
  id: "round-count",
  schema: schema.integer(),
  description: "Rounds recorded by keep/discard/review-rejected arms.",
});
export const keptCount = slot({
  id: "kept-count",
  schema: schema.integer(),
  description: "Rounds selected by the declared keep guard.",
});
export const decisions = slot.list(decisionStatus, {
  id: "decisions",
  description: "Baseline marker and runtime-owned keep/discard/review-rejected dispositions in execution order.",
});
export const commitMessage = slot.text({
  id: "commit-message",
  description:
    "Derived commit message citing bestMetric (before), the aggregate metric (after), and the run count; injected directly into the git commit command step.",
});
export const summary = slot({
  id: "summary",
  schema: summarySchema,
  description: "Terminal typed run report.",
});
export const summaryPort = port.output.of(summarySchema, {
  id: "summary",
  title: "Optimize Summary",
  description: "Typed baseline, aggregate measurements, kept count, and final best metric.",
  value: summary,
  format: "json",
});

// experiment-variant-only slots
export const proposal = slot({
  id: "proposal",
  schema: proposalSchema,
  description: "Current fresh experiment proposal.",
});
export const applyReceipt = slot.text({
  id: "apply-receipt",
  description: "Worker receipt for the bounded candidate mutation.",
});

// benchmark-variant-only slots
export const scopeSlot = slot.text({
  id: "scope",
  description:
    "smart-1's bounded scope for one benchmark-improvement attempt, derived from the target, current best metric, and measurement history.",
});
export const draftSlot = slot.text({
  id: "draft",
  description: "smart-1's concrete worker draft turning the scope into an actionable implementation brief.",
});
export const implementReceipt = slot.text({
  id: "implement-receipt",
  description: "Worker's report of the implemented candidate.",
});
