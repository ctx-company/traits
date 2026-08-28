import { input, step } from "@ctx-traits/cdk";

import { htmlPath, outputDir, topic, topicSlug } from "../data.ts";

/**
 * Deterministic, shell-based slugify (pattern: research/shared/step/derive.ts):
 * lowercases the topic, collapses everything outside [a-z0-9] to a single
 * hyphen, and trims leading/trailing hyphens. A command step, never agent
 * prose, so the output path is predictable from the topic alone.
 */
export function deriveTopicSlugStep(): void {
  step.command("Derive topic slug", {
    id: "topic-slug",
    input: input.command`sh -c "printf \\"%s\\" \\"\\$1\\" | tr \\"A-Z\\" \\"a-z\\" | tr -cs \\"a-z0-9\\" \\"-\\" | sed \\"s/^-*//;s/-*\\$//\\"" _ ${topic}`,
    output: topicSlug,
  });
}

/**
 * Deterministic delivery-path derivation: `{output-dir}/<topic-slug>.html` —
 * `printf` with no shell (the format string alone determines the layout), so
 * proofs can assert the exact artifact path a run produces.
 */
export function deriveHtmlPathStep(): void {
  const strings = [`printf %s/%s.html `, ` `, ``];
  const commandInput = input.command(
    Object.assign(strings, { raw: strings }) as unknown as TemplateStringsArray,
    outputDir,
    topicSlug,
  );
  step.command("Derive walkthrough path", {
    id: "walkthrough-path",
    input: commandInput,
    output: htmlPath,
  });
}
