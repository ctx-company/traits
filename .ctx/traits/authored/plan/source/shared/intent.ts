import type { Intent } from "@ctx-traits/cdk";
import { intent } from "@ctx-traits/cdk";

export default {
  focus: [intent.focus.Specific, intent.focus.Correctness],
  avoid: [intent.avoid.SpeculativeClaim, intent.avoid.ScopeCreep],
} as Intent;
