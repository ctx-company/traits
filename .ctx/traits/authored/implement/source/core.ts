// In-package behavior/intent fragments (0109): the plain typed objects every
// implement variant composes, so each behavior/intent fact exists in exactly
// one place instead of being hand-copied across default/quick/smart/strict/
// phase. Single-trait-package rule: these stay here until a second
// consuming package exists, and then only import paths change.
import type { Behavior, Intent } from "@ctx-traits/cdk";
import { intent, method, tone, verbosity } from "@ctx-traits/cdk";

/** Identical across all five variants today. */
export const FAMILY_BEHAVIOR: Behavior = {
  tone: [tone.Direct, tone.Technical],
  method: method.EvidenceFirst,
  verbosity: verbosity.Brief,
};

/** quick's lean intent set (variants/quick.ts). */
export const QUICK_INTENT: Intent = {
  require: [
    intent.focus.Correctness,
    intent.require.Leanness,
    intent.require.ReuseOverReimplement,
    intent.require.ReviewBeforeFinal,
  ],
  avoid: [intent.avoid.OverEngineering, intent.avoid.ScopeCreep, intent.avoid.RubberStampReview],
};

/** gated's intent: quick's lean set plus role-attributed output for the owner-in-the-loop surfaces (variants/gated.ts). */
export const GATED_INTENT: Intent = {
  require: [...QUICK_INTENT.require!, intent.require.RoleAttributedOutput],
  avoid: [...QUICK_INTENT.avoid!],
};

/** default/smart/phase's superset intent (variants/default.ts). */
export const FAMILY_INTENT: Intent = {
  require: [
    intent.focus.Correctness,
    intent.require.Robustness,
    intent.require.Pragmatism,
    intent.require.Elegance,
    intent.require.Leanness,
    intent.require.ReuseOverReimplement,
    intent.require.ReviewBeforeFinal,
    intent.require.BoundedRefinement,
  ],
  avoid: [
    intent.avoid.Accretion,
    intent.avoid.OverEngineering,
    intent.avoid.GoldPlating,
    intent.avoid.Duplication,
    intent.avoid.ScopeCreep,
    intent.avoid.UnboundedLoop,
    intent.avoid.RubberStampReview,
  ],
};

/** strict's intent: the family superset plus verbatim-execution doctrine (variants/strict.ts). */
export const STRICT_INTENT: Intent = {
  require: [...FAMILY_INTENT.require!, intent.require.VerbatimExecution],
  avoid: [...FAMILY_INTENT.avoid!, intent.avoid.SilentDeviation],
};
