// Hook-based (defineVariant) variants only. The rest of the family
// (default, smart, strict, phase, gated, guarded) is still legacy
// object-style `variant({...})` and is bound directly in source/index.ts —
// migrated one at a time (0183).
import { default as quick } from "./quick/index.ts";

export default {
  quick,
};
