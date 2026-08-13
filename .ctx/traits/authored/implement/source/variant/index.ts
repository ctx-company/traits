// Hook-based (defineVariant) variants: the full family (0183).
import { default as defaultVariant } from "./default/index.ts";
import { default as gated } from "./gated/index.ts";
import { default as guarded } from "./guarded/index.ts";
import { default as phase } from "./phase/index.ts";
import { default as quick } from "./quick/index.ts";
import { default as smart } from "./smart/index.ts";
import { default as strict } from "./strict/index.ts";

export default {
  default: defaultVariant,
  gated,
  guarded,
  phase,
  quick,
  smart,
  strict,
};
