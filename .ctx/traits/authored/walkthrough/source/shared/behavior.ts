// One variant today, but the family behavior lives here (single-trait-package
// rule, pattern: research/source/shared/behavior.ts) so a future quick/deep
// split inherits it without a move.
import * as cdk from "@ctx-traits/cdk";

export const family: cdk.Behavior = {
  tone: [cdk.behavior.tone.Direct, cdk.behavior.tone.Technical],
  method: cdk.behavior.method.EvidenceFirst,
  verbosity: cdk.behavior.verbosity.Brief,
};
