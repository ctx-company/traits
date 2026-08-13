import type { IntentSpec } from "@ctx-traits/cdk";
import { intent } from "@ctx-traits/cdk";

export default {
  focus: [intent.focus.Specific, intent.focus.Correctness],
  avoid: [intent.avoid.SpeculativeClaim, intent.avoid.ScopeCreep],
} as IntentSpec;
