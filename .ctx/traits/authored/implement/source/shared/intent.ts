// Two intent sets: quick's lean single-pass set, and the reviewed variants'
// superset (default and complex share it).
import type { IntentSpec } from "@ctx-traits/cdk";
import { intent } from "@ctx-traits/cdk";

export const QUICK_INTENT: IntentSpec = {
  require: [intent.focus.Correctness, intent.require.Leanness, intent.require.ReuseOverReimplement],
  avoid: [intent.avoid.OverEngineering, intent.avoid.ScopeCreep],
};

export const REVIEWED_INTENT: IntentSpec = {
  require: [
    intent.focus.Correctness,
    intent.require.Robustness,
    intent.require.Leanness,
    intent.require.ReuseOverReimplement,
    intent.require.ReviewBeforeFinal,
    intent.require.BoundedRefinement,
  ],
  avoid: [
    intent.avoid.OverEngineering,
    intent.avoid.Duplication,
    intent.avoid.ScopeCreep,
    intent.avoid.UnboundedLoop,
    intent.avoid.RubberStampReview,
  ],
};
