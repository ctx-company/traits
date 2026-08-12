import * as cdk from "@ctx-traits/cdk";

import {
  baselineResult,
  bestMetric,
  decisions,
  history,
  keptCount,
  readinessSlot,
  roundCount,
  summary,
} from "../data.ts";

// Neither preflight nor baseline gating has a "rounds"/"kept" story yet
// (seed-best has not run), so these are typed deterministic command steps —
// never an agent-authored summary — filling every field the summary schema
// requires: status "aborted", reason "aborted", zero rounds/kept, empty
// rows (baseline-failure keeps the one failing measurement as an honest
// baseline row).
export function abortSummaryStage(title: string) {
  return cdk.step.command(title, {
    id: "abort-summary",
    argv: [
      "node",
      "--input-type=module",
      "--eval",
      `
const [readinessText] = process.argv.slice(1);
const readiness = JSON.parse(readinessText);
process.stdout.write(JSON.stringify({
    status: "aborted",
    reason: "aborted",
    rounds: 0,
    kept: 0,
    rows: [],
    detail: readiness.detail,
}));
            `.trim(),
      readinessSlot,
    ],
    output: summary,
  });
}
export function baselineFailureSummaryStage(title: string) {
  return cdk.step.command(title, {
    id: "baseline-failure-summary",
    argv: [
      "node",
      "--input-type=module",
      "--eval",
      `
const [baselineText] = process.argv.slice(1);
const baseline = JSON.parse(baselineText);
process.stdout.write(JSON.stringify({
    status: "aborted",
    reason: "aborted",
    rounds: 0,
    kept: 0,
    rows: [{ measurement: baseline, decision: "baseline" }],
    detail: "trusted baseline command did not return status ok: " + JSON.stringify(baseline),
}));
            `.trim(),
      baselineResult,
    ],
    output: summary,
  });
}

// Never reconstructed from correlated counters: each stop reason is a
// distinct command invocation whose only difference is the literal
// `reason` argv it supplies. `rows` are replayed strictly from the
// runtime-owned decisions/history ledgers, never from a model claim.
const deriveSummaryScript = `
const [reason, roundsText, keptText, bestText, decisionsText, measurementsText, reviewsText] = process.argv.slice(1);
const rounds = Number(roundsText);
const kept = Number(keptText);
const best = Number(bestText);
const decisions = JSON.parse(decisionsText);
const measurements = JSON.parse(measurementsText);
const reviewList = reviewsText === undefined ? undefined : JSON.parse(reviewsText);
if (!["target-reached", "iteration-limit-reached"].includes(reason)
    || !Number.isSafeInteger(rounds) || rounds < 0
    || !Number.isSafeInteger(kept) || kept < 0
    || !Number.isFinite(best)
    || !Array.isArray(decisions) || !Array.isArray(measurements)
    || (reviewList !== undefined && !Array.isArray(reviewList))
    || decisions[0] !== "baseline"
    || decisions.filter((decision) => decision === "kept").length !== kept) {
    process.exit(1);
}
let measurementIndex = 0;
const rows = decisions.map((decision) => {
    if (decision === "review-rejected") return { decision };
    const measurement = measurements[measurementIndex];
    measurementIndex += 1;
    return { measurement, decision };
});
if (measurementIndex !== measurements.length) process.exit(1);
const detail = "Decisions and counts projected from runtime-owned keep/discard"
    + (reviewList === undefined ? "" : "/review")
    + " branches; stop reason preserved from the guard/exhaustion branch that actually terminated the run."
    + (reviewList === undefined ? "" : " Review verdicts: " + JSON.stringify(reviewList));
process.stdout.write(JSON.stringify({
    status: "completed",
    reason,
    rounds,
    kept,
    best,
    rows,
    detail,
}));
`.trim();

export function deriveSummaryStage(
  title: string,
  reason: "target-reached" | "iteration-limit-reached",
  reviewsSlot?: cdk.SlotHandle,
) {
  const argv = [
    "node",
    "--input-type=module",
    "--eval",
    deriveSummaryScript,
    reason,
    roundCount,
    keptCount,
    bestMetric,
    decisions,
    history,
  ];
  if (reviewsSlot !== undefined) argv.push(reviewsSlot);
  return cdk.step.command(title, {
    id: `derive-summary-${reason}`,
    argv,
    output: summary,
  });
}
