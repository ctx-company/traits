const [experimentsText, keptText, bestText, measurementsText, decisionsText] = process.argv.slice(2);
const experiments = Number(experimentsText);
const kept = Number(keptText);
const best = Number(bestText);
const measurements = JSON.parse(measurementsText);
const decisions = JSON.parse(decisionsText);
if (!Number.isSafeInteger(experiments) || experiments < 0
    || !Number.isSafeInteger(kept) || kept < 0
    || !Number.isFinite(best)
    || !Array.isArray(measurements) || !Array.isArray(decisions)
    || measurements.length !== experiments + 1
    || decisions.length !== measurements.length
    || decisions[0] !== "baseline"
    || decisions.slice(1).some((decision) => decision !== "kept" && decision !== "discarded")
    || decisions.filter((decision) => decision === "kept").length !== kept) {
    process.exit(1);
}
let replayedBest = measurements[0]?.metric;
for (let index = 1; index < measurements.length; index += 1) {
    if (decisions[index] === "kept") replayedBest = measurements[index]?.metric;
}
if (!Number.isFinite(replayedBest) || replayedBest !== best) process.exit(1);
const rows = measurements.map((measurement, index) => ({ measurement, decision: decisions[index] }));
process.stdout.write(JSON.stringify({
    status: "completed",
    experiments,
    kept,
    best,
    rows,
    detail: "Decisions and counts projected from runtime-owned keep/discard branches.",
}));
