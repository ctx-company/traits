// Identical across all three variants. useBehavior at shell level does not
// cascade into variant canonicals — each variant frame is independent — so
// every variant hook calls useBehavior(shared.behavior.family) itself.
import * as cdk from "@ctx-traits/cdk";

export const family: cdk.Behavior = {
  tone: [cdk.behavior.tone.Direct, cdk.behavior.tone.Technical],
  method: cdk.behavior.method.EvidenceFirst,
  verbosity: cdk.behavior.verbosity.Brief,
};
