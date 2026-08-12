import * as cdk from "@ctx-traits/cdk";

import {
  benchmarkRuns,
  bestMetric,
  bestRef,
  candidateResult,
  capturedRef,
  commandReceipt,
  commitMessage,
} from "../data.ts";

// Kept inline (not extracted to a resource) matching the recovered
// auto-research port shape: this is a fresh package, so a resource file
// carries no lesser trust burden than an inline body — both need fresh
// human trust approval, and inline keeps the canonical trait document
// self-contained (P516's rationale for the historical package).
const gitRefCapture = `
import { spawnSync } from "node:child_process";
const result = spawnSync("git", ["rev-parse", "--verify", "HEAD^{commit}"], {
    cwd: process.cwd(),
    encoding: "utf8",
});
const sha = (result.stdout ?? "").trim();
if (result.status !== 0 || !/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/.test(sha)) {
    process.stderr.write(result.stderr ?? "could not capture HEAD commit");
    process.exit(1);
}
process.stdout.write(JSON.stringify({ sha }));
`.trim();

// The provenance IS the point (owner ruling): the kept commit message cites
// the measured before/after values and the run count, derived
// deterministically — never a model-authored claim of improvement.
const deriveCommitMessageScript = `
const [beforeText, candidateText, runsText] = process.argv.slice(1);
const before = Number(beforeText);
const candidate = JSON.parse(candidateText);
const runs = Number(runsText);
process.stdout.write(
    "optimize: keep improved candidate (" + before + " -> " + candidate.metric + ", median of " + runs + " runs)",
);
`.trim();

// Every export below is a title-taking factory (mirroring `cdk.stage`'s
// calling convention) rather than a pre-registered step: an argv-based
// command step cannot use `cdk.stage()` (its command mode only accepts a
// tagged-template `input.command`, never a bare `argv` array), and eager
// `step.command(...)` at module scope would register outside any active
// procedure/loop/branch scope.
export function captureInitialRef(title: string) {
  return cdk.step.command(title, {
    id: "capture-initial-ref",
    argv: ["node", "--input-type=module", "--eval", gitRefCapture],
    output: capturedRef,
  });
}
export function captureBestRef(title: string) {
  return cdk.step.project(title, {
    id: "capture-best-ref",
    projections: [{ source: capturedRef, field: "sha", destination: bestRef }],
  });
}
export function captureCommittedRef(title: string) {
  return cdk.step.command(title, {
    id: "capture-committed-ref",
    argv: ["node", "--input-type=module", "--eval", gitRefCapture],
    output: capturedRef,
  });
}
export function advanceBest(title: string) {
  return cdk.step.project(title, {
    id: "advance-best",
    projections: [
      { source: candidateResult, field: "metric", destination: bestMetric },
      { source: capturedRef, field: "sha", destination: bestRef },
    ],
  });
}
export function deriveCommitMessage(title: string) {
  return cdk.step.command(title, {
    id: "derive-commit-message",
    argv: [
      "node",
      "--input-type=module",
      "--eval",
      deriveCommitMessageScript,
      bestMetric,
      candidateResult,
      benchmarkRuns,
    ],
    output: commitMessage,
  });
}
export function stageCandidate(title: string) {
  return cdk.step.command(title, { id: "stage-candidate", argv: ["git", "add", "-A"], output: commandReceipt });
}
export function commitCandidate(title: string) {
  return cdk.step.command(title, {
    id: "commit-candidate",
    argv: ["git", "commit", "-m", commitMessage],
    output: commandReceipt,
  });
}
export function resetCandidate(title: string) {
  return cdk.step.command(title, {
    id: "reset-candidate",
    argv: ["git", "reset", "--hard", bestRef],
    output: commandReceipt,
  });
}
export function cleanCandidate(title: string) {
  return cdk.step.command(title, { id: "clean-candidate", argv: ["git", "clean", "-fd"], output: commandReceipt });
}
