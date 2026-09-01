import * as cdk from "@ctx-traits/cdk";

import { smart } from "../agent.ts";
import { criticVerdict, grounding, receipt, target } from "../data.ts";

export const openEndedness = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    Read-only adversarial review of ${target} and ${receipt} against ${grounding}. Inspect the actual task file — and, when the receipt records children, every child file too; edit nothing.
    Split shape (when children exist): block a child whose key is not the parent key plus a positional ordinal in receipt order, whose [relations] parent is not the parent key, or whose depends-on names a non-sibling without code-proven need; block the parent if it still carries an execution plan instead of charter intent and the umbrella stop condition; block a split that is packaging — children with no genuine shared umbrella. Every child is judged by the same Done-when/approach/checks bars as a standalone plan below. Block ONLY these: a Done-when goal that is missing, unobservable, hedged ("or", "either", "consider", "optionally", "TBD", "as appropriate" inside a goal statement), or lacking a runnable existing command (fabricated commands block); approach text presented as binding or restrictive rather than suggested — the plan must state in its own text that files, symbols, edit order, commands, and proof mechanics are the suggested route and that deviation in service of a goal is legitimate and reported, not escalated; pinned internal proof mechanics (exact fixtures, matchers, or literal expected output) that the grounding does not evidence as already occurring; status other than ready; changed filename, key, raised date, or relations without a code-proven correction. Also block unless receipt.key and receipt.file exactly match target, receipt records status-before draft and status-after ready, and relations-correction is present exactly when the task's relations changed and explains that code-proven correction. Do not block on taste, general quality, or hedge words inside approach suggestions.
  `,
  output: cdk.output.prompt`Return the typed review verdict: revise only for those blockers, approved when none remain: ${criticVerdict}`,
});
