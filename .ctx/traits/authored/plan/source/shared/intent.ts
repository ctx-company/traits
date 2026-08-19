import type { Intent } from "@ctx-traits/cdk";
import { intent } from "@ctx-traits/cdk";

export default {
  focus: [intent.Specific, intent.Correctness],
  avoid: [intent.SpeculativeClaim, intent.ScopeCreep],
} as Intent;
