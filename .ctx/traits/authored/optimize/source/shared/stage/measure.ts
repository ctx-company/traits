import * as cdk from "@ctx-traits/cdk";

import { benchmarkRuns } from "#trait/shared/data.ts";

// A `benchmark-runs` value is a runtime-resolved port, not a compile-time
// literal, so it cannot drive `flow.loop`'s `maxIterations` (which only
// accepts a literal number, 0102). This single deterministic node runner
// executes the caller-supplied no-shell argv N times and computes the
// aggregate in one step instead — same determinism as a port-driven loop,
// one step, no argv-size blowup from threading N intermediate results
// through the runtime.
//
// Median, not mean: the aggregate metric must be a value some run actually
// reported, never an average that fabricates a number no run produced. Odd
// N picks the middle element of the ascending sort; even N picks the upper
// -median element (index n/2) for the same reason — still one real run's
// value, never an interpolation between two.
//
// delta-lines (optional, feeds the max-delta-lines cap) follows the same
// actually-measured-value rule: it is carried from whichever run produced
// the median metric, never averaged or recomputed. Runs that omit the field
// leave it absent on the aggregate, matching the capGuard's supplied-but
// -unmeasurable-therefore-discards arm.
const aggregateRunnerScript = `
import { spawnSync } from "node:child_process";
const [argvText, runsText] = process.argv.slice(1);
const argv = JSON.parse(argvText);
const runs = Number(runsText);
if (!Array.isArray(argv) || argv.length === 0 || !Number.isSafeInteger(runs) || runs < 1) {
    process.stderr.write("invalid aggregate-runner input");
    process.exit(1);
}
const records = [];
let allOk = true;
for (let index = 0; index < runs; index += 1) {
    const result = spawnSync(argv[0], argv.slice(1), { encoding: "utf8" });
    let parsed;
    try {
        parsed = JSON.parse((result.stdout ?? "").trim());
    } catch {
        parsed = undefined;
    }
    const ok = result.status === 0 && parsed !== undefined && parsed !== null
        && parsed.status === "ok" && Number.isFinite(parsed.metric);
    if (!ok) {
        allOk = false;
        continue;
    }
    records.push({
        metric: parsed.metric,
        deltaLines: Number.isFinite(parsed["delta-lines"]) ? parsed["delta-lines"] : undefined,
    });
}
const sorted = [...records].sort((a, b) => a.metric - b.metric);
const medianIndex = Math.floor(sorted.length / 2);
const median = sorted.length > 0 ? sorted[medianIndex] : undefined;
const metric = median ? median.metric : Number.NaN;
const output = {
    status: allOk && records.length === runs ? "ok" : "error",
    metric,
    runs: records.map((record) => record.metric),
};
if (median && median.deltaLines !== undefined) {
    output["delta-lines"] = median.deltaLines;
}
process.stdout.write(JSON.stringify(output));
`.trim();

export function measureAggregateStage(title: string, commandPort: cdk.PortHandle<string[]>, output: cdk.SlotHandle) {
  return cdk.step.command(title, {
    argv: ["node", "--input-type=module", "--eval", aggregateRunnerScript, commandPort, benchmarkRuns],
    output,
  });
}
