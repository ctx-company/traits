// In-package behavior/intent fragments (0109, mirroring implement's
// source/core.ts): the plain typed objects every research variant composes,
// so each behavior/intent fact exists in exactly one place instead of being
// hand-copied across quick/default/deep. Single-trait-package rule: these
// stay here until a second consuming package exists.
import type { Behavior, Intent } from "@ctx-traits/cdk";
import { intent, method, tone, verbosity } from "@ctx-traits/cdk";

/** Identical across all three variants. */
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
        intent.require.ReviewBeforeFinal,
    ],
    avoid: [
        intent.avoid.OverEngineering,
        intent.avoid.ScopeCreep,
        intent.avoid.RubberStampReview,
    ],
};

/** default/deep's superset intent (variants/default.ts, variants/deep.ts). */
export const FAMILY_INTENT: Intent = {
    require: [
        intent.focus.Correctness,
        intent.require.Robustness,
        intent.require.Leanness,
        intent.require.ReviewBeforeFinal,
    ],
    avoid: [
        intent.avoid.OverEngineering,
        intent.avoid.ScopeCreep,
        intent.avoid.UnboundedLoop,
        intent.avoid.RubberStampReview,
    ],
};
