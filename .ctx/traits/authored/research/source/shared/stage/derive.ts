import type { SlotHandle } from "@ctx-traits/cdk";
import { input, step } from "@ctx-traits/cdk";

import { outputDir, reportPath, topic, topicSlug } from "../data.ts";

/**
 * Deterministic, shell-based slugify (0153): lowercases the topic, collapses
 * everything outside [a-z0-9] to a single hyphen, and trims leading/trailing
 * hyphens. A command step, never agent prose, so every later step and every
 * proof can predict the campaign directory name from the topic alone.
 */
export function deriveTopicSlugStep(): void {
  step.command("Derive topic slug", {
    id: "topic-slug",
    input: input.command`sh -c "printf \\"%s\\" \\"\\$1\\" | tr \\"A-Z\\" \\"a-z\\" | tr -cs \\"a-z0-9\\" \\"-\\" | sed \\"s/^-*//;s/-*\\$//\\"" _ ${topic}`,
    output: topicSlug,
  });
}

/**
 * Deterministic delivery-path derivation: `{output-dir}/<topic-slug>/<fileName>`
 * — a command step (`printf`, no shell needed: the format string alone
 * fully determines the layout), never agent prose, so proofs can assert the
 * exact file set a variant produces.
 */
export function deriveReportPathStep(fileName: string, target: SlotHandle<string> = reportPath): void {
  const idSlug = fileName.replace(/\.[^.]+$/, "").replace(/[^a-z0-9]+/gi, "-");
  const strings = [`printf %s/%s/${fileName} `, ` `, ``];
  const commandInput = input.command(
    Object.assign(strings, { raw: strings }) as unknown as TemplateStringsArray,
    outputDir,
    topicSlug,
  );
  step.command(`Derive ${fileName} path`, {
    id: `${idSlug}-path`,
    input: commandInput,
    output: target,
  });
}
