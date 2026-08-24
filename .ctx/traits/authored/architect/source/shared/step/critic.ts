import * as cdk from "@ctx-traits/cdk";

import { smart } from "../agent.ts";
import { criticVerdict, grounding, receipt, target } from "../data.ts";

export const openEndedness = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    Read-only adversarial review of ${target} and ${receipt} against ${grounding}. Inspect the actual task file; do not edit it. Block ONLY open-endedness or contract deviation: unnamed files; instructions without exact symbols and signatures; missing removed-symbol names or strict edit order; Done-whens without runnable existing commands or with fabricated commands; status other than ready; changed filename, key, raised date, or relations without a code-proven correction; and instruction hedges "or", "either", "consider", "optionally", "TBD", or "as appropriate". Also block unless receipt.key and receipt.file exactly match target, receipt records status-before draft and status-after ready, and relations-correction is present exactly when the task's relations changed and explains that code-proven correction. Do not block on taste or general quality.
  `,
  output: cdk.output.prompt`Return the typed review verdict: revise only for those blockers, approved when none remain: ${criticVerdict}`,
});
