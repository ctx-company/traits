// Identical across all three variants. useBehavior at shell level does not
// cascade into variant canonicals — each variant frame is independent — so
// every variant hook calls useBehavior(shared.behavior.family) itself.
import type { BehaviorFields } from "@ctx-traits/cdk";
import { method, tone, verbosity } from "@ctx-traits/cdk";

export const family: BehaviorFields = {
  tone: [tone.Direct, tone.Technical],
  method: method.EvidenceFirst,
  verbosity: verbosity.Brief,
};
