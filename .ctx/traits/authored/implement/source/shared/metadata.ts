import type { Behavior } from "@ctx-traits/cdk";
import { method, tone, verbosity } from "@ctx-traits/cdk";

/** Identical across every variant today. */
export const FAMILY_BEHAVIOR: Behavior = {
  tone: [tone.Direct, tone.Technical],
  method: method.EvidenceFirst,
  verbosity: verbosity.Brief,
};

/** Common tag prefix every variant's own metadata.tag extends. */
export const tag = ["dogfood", "implementation", "review"];
