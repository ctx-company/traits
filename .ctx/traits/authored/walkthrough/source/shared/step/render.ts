import { input, step } from "@ctx-traits/cdk";

import { coverageReport, coverageStatus, fileClosure, htmlPath, nodeBatches, renderLog, skeletonNodes, symbolIndex } from "../data.ts";
import { renderScript, symbolsScript } from "../resource.ts";

/**
 * Deterministic symbol enumeration over the surveyed file closure: the
 * ground truth the coverage gate joins against. `{resource:symbols-script}`
 * resolves to the package's symbols.py; the typed closure slot travels as
 * one JSON argv element. No shell, no model.
 */
export function enumerateSymbolsStep(): void {
  step.command("Enumerate symbols", {
    id: "enumerate-symbols",
    input: input.command`python3 ${symbolsScript} enumerate ${fileClosure}`,
    output: symbolIndex,
  });
}

/** Full coverage report: uncovered keys, duplicate ids, orphaned parents — consumed verbatim by the repair prompt. */
export function coverageReportStep(idSuffix: string): void {
  step.command(`Coverage report ${idSuffix}`, {
    id: `coverage-report-${idSuffix}`,
    input: input.command`python3 ${symbolsScript} coverage ${symbolIndex} ${skeletonNodes} ${nodeBatches}`,
    output: coverageReport,
  });
}

/** One-word coverage verdict ("complete" | "incomplete:<n>") — the loop's exit condition reads this slot. */
export function coverageStatusStep(idSuffix: string): void {
  step.command(`Coverage status ${idSuffix}`, {
    id: `coverage-status-${idSuffix}`,
    input: input.command`python3 ${symbolsScript} status ${symbolIndex} ${skeletonNodes} ${nodeBatches}`,
    output: coverageStatus,
  });
}

/**
 * The deterministic render tail: python3 runs the package's render.py over
 * the skeleton, the described batches (flattened, last-id-wins), and the
 * derived output path. No model seat anywhere near the HTML.
 */
export function renderWalkthroughStep(): void {
  step.command("Render walkthrough", {
    id: "render-walkthrough",
    input: input.command`python3 ${renderScript} ${skeletonNodes} ${nodeBatches} ${htmlPath}`,
    output: renderLog,
  });
}
